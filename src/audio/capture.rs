use crate::audio::resample::{downmix_to_mono, Resampler};
use crate::messages::{Stage, StageState, UiMsg};
use crossbeam_channel::Sender;
use std::collections::VecDeque;
use std::time::Duration;
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode};

pub const TARGET_RATE: u32 = 16000;

fn werr<E: std::fmt::Display>(e: E) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// 一次采集会话；设备异常时返回 Err，由外层重试
fn run_once(tx: &Sender<Vec<f32>>, ui: &Sender<UiMsg>) -> anyhow::Result<()> {
    let enumerator = DeviceEnumerator::new().map_err(werr)?;
    let device = enumerator.get_default_device(&Direction::Render).map_err(werr)?;
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
    tracing::info!("loopback 采集已启动：{rate} Hz {channels} ch");
    let _ = ui.send(UiMsg::Status { stage: Stage::Capture, state: StageState::Ok });

    let mut resampler = Resampler::new(rate, TARGET_RATE);
    let mut q: VecDeque<u8> = VecDeque::new();
    let mut interleaved: Vec<f32> = Vec::new();
    loop {
        if h_event.wait_for_event(250).is_err() {
            continue; // 无应用放音时无事件，正常
        }
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

/// 采集线程：失败每 2 秒重试
pub fn spawn_capture(tx: Sender<Vec<f32>>, ui: Sender<UiMsg>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            wasapi::initialize_mta().ok().expect("COM 初始化失败");
            loop {
                match run_once(&tx, &ui) {
                    Ok(()) => return, // 下游已关闭
                    Err(e) => {
                        tracing::warn!("采集失败，2 秒后重试：{e}");
                        let _ = ui.send(UiMsg::Status {
                            stage: Stage::Capture,
                            state: StageState::Err(format!("音频设备重连中：{e}")),
                        });
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }
        })
        .expect("spawn capture")
}
