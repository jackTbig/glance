use crate::audio::resample::{downmix_to_mono, Resampler};
use crate::messages::{Stage, StageState, UiMsg};
use crossbeam_channel::Sender;
use std::collections::VecDeque;
use std::time::Duration;
use wasapi::{Device, DeviceEnumerator, DeviceState, Direction, SampleType, StreamMode};

pub const TARGET_RATE: u32 = 16000;

fn werr<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// 列出当前可用的播放设备（友好名），设置窗用；需先 initialize_mta
pub fn list_render_devices() -> Vec<String> {
    let mut v = Vec::new();
    let Ok(enumerator) = DeviceEnumerator::new() else { return v };
    let Ok(coll) = enumerator.get_device_collection(&Direction::Render) else { return v };
    let n = coll.get_nbr_devices().unwrap_or(0);
    for i in 0..n {
        if let Ok(d) = coll.get_device_at_index(i) {
            if matches!(d.get_state(), Ok(DeviceState::Active)) {
                if let Ok(name) = d.get_friendlyname() {
                    v.push(name);
                }
            }
        }
    }
    v
}

/// 按友好名找播放设备；空串 = 系统默认播放设备
fn pick_device(enumerator: &DeviceEnumerator, preferred: &str) -> anyhow::Result<Device> {
    if !preferred.trim().is_empty() {
        let coll = enumerator.get_device_collection(&Direction::Render).map_err(werr)?;
        let n = coll.get_nbr_devices().map_err(werr)?;
        for i in 0..n {
            if let Ok(d) = coll.get_device_at_index(i) {
                if d.get_friendlyname().map_or(false, |n| n == preferred.trim()) {
                    return Ok(d);
                }
            }
        }
        tracing::warn!("找不到指定的播放设备 {preferred:?}，改用系统默认");
    }
    enumerator.get_default_device(&Direction::Render).map_err(werr)
}

/// 一次采集会话；设备异常或默认设备切换时返回 Err，由外层重试
fn run_once(tx: &Sender<Vec<f32>>, ui: &Sender<UiMsg>, preferred: &str) -> anyhow::Result<()> {
    let enumerator = DeviceEnumerator::new().map_err(werr)?;
    let device = pick_device(&enumerator, preferred)?;
    let device_id = device.get_id().map_err(werr)?;
    let device_name = device.get_friendlyname().unwrap_or_else(|_| "?".into());
    let follow_default = preferred.trim().is_empty();
    let mut client = device.get_iaudioclient().map_err(werr)?;
    let fmt = client.get_mixformat().map_err(werr)?;
    let rate = fmt.get_samplespersec();
    let channels = fmt.get_nchannels() as usize;
    if !matches!(fmt.get_subformat(), Ok(SampleType::Float)) || fmt.get_bitspersample() != 32 {
        anyhow::bail!("混音格式不是 32 位浮点（{} bit），第一版不支持", fmt.get_bitspersample());
    }
    let (_def, min) = client.get_device_period().map_err(werr)?;
    client
        .initialize_client(
            &fmt,
            &Direction::Capture,
            &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: min },
        )
        .map_err(werr)?;
    let h_event = client.set_get_eventhandle().map_err(werr)?;
    let cap = client.get_audiocaptureclient().map_err(werr)?;
    client.start_stream().map_err(werr)?;
    tracing::info!("loopback 采集已启动：{device_name}（{rate} Hz {channels} ch，{}）", if follow_default { "跟随系统默认" } else { "指定设备" });
    let _ = ui.send(UiMsg::Status { stage: Stage::Capture, state: StageState::Ok });

    let mut resampler = Resampler::new(rate, TARGET_RATE);
    let mut q: VecDeque<u8> = VecDeque::new();
    let mut interleaved: Vec<f32> = Vec::new();
    let mut idle_ticks: u32 = 0;
    loop {
        if h_event.wait_for_event(250).is_err() {
            // 无应用放音时无事件，正常；趁空闲每 2 秒查一次默认播放设备有没有被切走
            // （切默认设备不会让旧设备报错，之前就一直听着没声音的那个）
            idle_ticks += 1;
            if follow_default && idle_ticks % 8 == 0 {
                if let Ok(now) = enumerator.get_default_device(&Direction::Render) {
                    if now.get_id().map_or(false, |id| id != device_id) {
                        let name = now.get_friendlyname().unwrap_or_else(|_| "?".into());
                        client.stop_stream().ok();
                        anyhow::bail!("默认播放设备已切换为 {name}，重新采集");
                    }
                }
            }
            continue;
        }
        idle_ticks = 0;
        cap.read_from_device_to_deque(&mut q).map_err(werr)?;
        interleaved.clear();
        while q.len() >= 4 {
            let b = [
                q.pop_front().unwrap(),
                q.pop_front().unwrap(),
                q.pop_front().unwrap(),
                q.pop_front().unwrap(),
            ];
            interleaved.push(f32::from_le_bytes(b));
        }
        if interleaved.is_empty() {
            continue;
        }
        let mono = downmix_to_mono(&interleaved, channels);
        let out = resampler.process(&mono);
        if tx.send(out).is_err() {
            client.stop_stream().ok();
            return Ok(());
        }
    }
}

/// 采集线程：失败每 2 秒重试；device 为空 = 跟随系统默认播放设备
pub fn spawn_capture(tx: Sender<Vec<f32>>, ui: Sender<UiMsg>, device: String) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            wasapi::initialize_mta().ok().expect("COM 初始化失败");
            loop {
                match run_once(&tx, &ui, &device) {
                    Ok(()) => return, // 下游已关闭
                    Err(e) => {
                        let switched = e.to_string().contains("已切换");
                        if switched {
                            tracing::info!("{e}");
                        } else {
                            tracing::warn!("采集失败，2 秒后重试：{e}");
                            let _ = ui.send(UiMsg::Status {
                                stage: Stage::Capture,
                                state: StageState::Err(format!("音频设备重连中：{e}")),
                            });
                            std::thread::sleep(Duration::from_secs(2));
                        }
                    }
                }
            }
        })
        .expect("spawn capture")
}
