# live-caption 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 一个 Rust 单二进制的桌面悬浮字幕窗：内录系统声音 → 本机 Whisper（Vulkan）识别英文 → claude CLI 常驻子进程译成中文 → 双语字幕上屏并落盘。

**Architecture:** lib + bin 布局。四条工作线程（采集 / 切句 / 识别 / 翻译）用 crossbeam 有界通道串联，界面线程只消费 `UiMsg`。纯逻辑（重采样、切句、翻译协议、记录格式）都是无 I/O 的结构体或函数，先写测试再写实现；I/O 线程各自一个 `spawn_*` 入口，失败自动重试。

**Tech Stack:** Rust 1.98 (msvc) · eframe/egui 0.36 · wasapi 0.24 · whisper-rs 0.16 (`vulkan` 特性，需 Vulkan SDK + CMake + LLVM) · crossbeam-channel 0.5 · serde/toml · tracing + tracing-appender · global-hotkey 0.8 · claude CLI 2.1.x（`-p --input-format stream-json`）

**设计文档：** `docs/superpowers/specs/2026-09-03-live-caption-design.md`

**约定：**
- 项目根 `D:\My_data\Claude\live-caption\`，所有命令在项目根执行；Bash 工具下路径写 `D:/...`。
- 第三方软件全部装 D 盘（`D:\Tools\`、`D:\VulkanSDK\`），不占 C 盘。
- 每个 Task 结束 `git commit`；提交信息中文。
- 测试命令统一 `cargo test`；涉及模型的测试标 `#[ignore]`，用 `cargo test --release -- --ignored` 单独跑。

---

## 文件结构

| 文件 | 职责 |
|---|---|
| `Cargo.toml` | 依赖与 release 配置 |
| `config.example.toml` | 配置样例（拷成 `config.toml` 使用；`config.toml` 进 .gitignore） |
| `src/lib.rs` | 导出各模块，供 bin / examples / tests 复用 |
| `src/main.rs` | 启动：日志、配置、通道、线程、`eframe::run_native` |
| `src/config.rs` | `Config` 结构体，加载/保存 toml |
| `src/messages.rs` | 线程间消息类型 `UiMsg` / `Stage` / `StageState` |
| `src/audio/mod.rs` | `pub mod resample; pub mod capture;` |
| `src/audio/resample.rs` | 立体声混单声道、线性重采样（纯函数 + 状态重采样器） |
| `src/audio/capture.rs` | WASAPI loopback 采集线程，输出 16 kHz 单声道 f32 块 |
| `src/segmenter.rs` | 能量 VAD 切句器（纯逻辑）+ 切句线程 |
| `src/asr.rs` | whisper-rs 识别器 + 识别线程 |
| `src/translate.rs` | claude 子进程协议（纯函数）+ 翻译线程（超时重启、轮换、积压丢弃） |
| `src/transcript.rs` | 会议记录文件（行格式纯函数 + 写文件） |
| `src/ui.rs` | eframe 悬浮窗 |
| `examples/asr_bench.rs` | 第 0 步：Vulkan 识别基准 |
| `examples/capture_smoke.rs` | 第 0 步：loopback 采集冒烟，录 5 秒到 wav |
| `scripts/claude_probe.py` | 第 0 步：claude 子进程往返计时 |
| `scripts/make_fixture.ps1` | 用 Windows TTS 生成英文测试音频 `tests/fixtures/en-10s.wav` |
| `tests/fake_claude.py` | 冒充 claude 的假子进程（集成测试用） |
| `tests/translator.rs` | 翻译线程集成测试 |
| `tests/asr_e2e.rs` | 识别端到端测试（`#[ignore]`） |
| `CLAUDE.md` | 项目交接卡（硬规则 / 环境变量 / 启动方式） |

---

## Task 0a：安装编译工具链（CMake / LLVM / Vulkan SDK，全部 D 盘）

**Files:** 无代码；产出环境变量。

- [ ] **Step 1: 查最新版本号**

```bash
gh api repos/Kitware/CMake/releases/latest --jq .tag_name
gh api repos/llvm/llvm-project/releases/latest --jq .tag_name
```
Expected: 形如 `v4.x.y` 与 `llvmorg-2x.y.z`。下面命令里把版本号替换进去。

- [ ] **Step 2: 安装 CMake（便携 zip 到 D:\Tools\CMake）**

```bash
mkdir -p "D:/Tools" && cd "D:/Tools" && curl -L -o cmake.zip "https://github.com/Kitware/CMake/releases/download/v<版本>/cmake-<版本>-windows-x86_64.zip" && unzip -q cmake.zip && mv "cmake-<版本>-windows-x86_64" CMake && rm cmake.zip && "D:/Tools/CMake/bin/cmake.exe" --version
```
Expected: `cmake version <版本>`

- [ ] **Step 3: 安装 LLVM（bindgen 需要 libclang，NSIS 静默装到 D:\Tools\LLVM）**

```bash
cd "D:/Tools" && curl -L -o llvm.exe "https://github.com/llvm/llvm-project/releases/download/llvmorg-<版本>/LLVM-<版本>-win64.exe" && ./llvm.exe /S /D=D:\Tools\LLVM && rm llvm.exe && ls "D:/Tools/LLVM/bin/libclang.dll"
```
Expected: 列出 `D:/Tools/LLVM/bin/libclang.dll`

- [ ] **Step 4: 安装 Vulkan SDK（静默，根目录 D:\VulkanSDK）**

```bash
cd "D:/Tools" && curl -L -o vulkan-sdk.exe "https://sdk.lunarg.com/sdk/download/latest/windows/vulkan-sdk.exe" && ./vulkan-sdk.exe --root "D:\VulkanSDK" --accept-licenses --default-answer --confirm-command install && rm vulkan-sdk.exe && ls "D:/VulkanSDK/Lib/vulkan-1.lib" "D:/VulkanSDK/Bin/glslc.exe"
```
Expected: 两个文件都列出。若 `--root` 被装成 `D:\VulkanSDK\<版本>\`，把后面所有 `D:\VulkanSDK` 换成实际含 `Lib\` 的目录。

- [ ] **Step 5: 写用户级环境变量（PowerShell，避免 setx 截断 PATH）**

```powershell
[Environment]::SetEnvironmentVariable("VULKAN_SDK", "D:\VulkanSDK", "User")
[Environment]::SetEnvironmentVariable("LIBCLANG_PATH", "D:\Tools\LLVM\bin", "User")
$p = [Environment]::GetEnvironmentVariable("PATH", "User")
[Environment]::SetEnvironmentVariable("PATH", "$p;D:\Tools\CMake\bin;D:\Tools\LLVM\bin;D:\VulkanSDK\Bin", "User")
```
Expected: 无输出。新开一个终端后 `cmake --version`、`clang --version`、`glslc --version` 都能执行。

- [ ] **Step 6: 记录到项目交接卡**

在 `D:\My_data\Claude\live-caption\CLAUDE.md` 写入：

```markdown
# live-caption 交接卡

## 编译环境（缺一不可，全部在 D 盘）
- CMake：`D:\Tools\CMake\bin`（PATH）
- LLVM/libclang：`D:\Tools\LLVM\bin`（`LIBCLANG_PATH`，bindgen 用）
- Vulkan SDK：`D:\VulkanSDK`（`VULKAN_SDK`，whisper-rs-sys 链接 vulkan-1.lib 并用 glslc 编 shader）
- MSVC：VS2022 Community 14.44（已装）
- 三个环境变量是用户级，**新开终端才生效**。

## 启动
- 开发：`cargo run --release`（第一次编 whisper.cpp 约 5–10 分钟）
- 配置：`config.toml`（拷自 `config.example.toml`），改完重启生效
```

```bash
cd "D:/My_data/Claude/live-caption" && git add CLAUDE.md && git commit -m "chore: 交接卡——编译环境与环境变量"
```

---

## Task 0b：工程骨架 + whisper-rs Vulkan 编译 + 识别基准

**Files:**
- Create: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `.gitignore`, `examples/asr_bench.rs`, `scripts/make_fixture.ps1`

- [ ] **Step 1: 生成英文测试音频（Windows 自带 TTS，16 kHz 单声道）**

`scripts/make_fixture.ps1`：
```powershell
Add-Type -AssemblyName System.Speech
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000, [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
New-Item -ItemType Directory -Force "$PSScriptRoot\..\tests\fixtures" | Out-Null
$s.SetOutputToWaveFile("$PSScriptRoot\..\tests\fixtures\en-10s.wav", $fmt)
$s.Speak("Good morning everyone. Today we will review the quarterly roadmap, discuss the migration to the new data center, and confirm the budget for GPU servers before the end of this month.")
$s.Dispose()
```
运行：
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "D:\My_data\Claude\live-caption\scripts\make_fixture.ps1"; (Get-Item "D:\My_data\Claude\live-caption\tests\fixtures\en-10s.wav").Length
```
Expected: 文件大小约 250–400 KB（8–12 秒）。

- [ ] **Step 2: 写 Cargo.toml**

```toml
[package]
name = "live-caption"
version = "0.1.0"
edition = "2021"

[lib]
name = "live_caption"
path = "src/lib.rs"

[[bin]]
name = "live-caption"
path = "src/main.rs"

[dependencies]
eframe = "0.36"
egui = "0.36"
wasapi = "0.24"
whisper-rs = { version = "0.16", features = ["vulkan"] }
crossbeam-channel = "0.5"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
global-hotkey = "0.8"
chrono = "0.4"
hound = "3.5"

[profile.release]
opt-level = 3
```

`.gitignore`：
```
/target
/config.toml
/transcripts
/logs
```

`src/lib.rs`（先只放空模块声明，后续 Task 逐个补）：
```rust
pub mod audio;
pub mod asr;
pub mod config;
pub mod messages;
pub mod segmenter;
pub mod transcript;
pub mod translate;
pub mod ui;
```
本 Task 先让它编译通过：把上面除 `asr` 外的行暂时注释掉，`src/asr.rs` 按 Task 5 的 Step 3 内容先写（基准要用；其中 `use crate::messages::…` 与 `spawn_asr` 依赖 messages 模块，本 Task 先把 `pub mod messages;` 也打开并按 Task 2 Step 1 写好 `src/messages.rs`），其余模块在各自 Task 取消注释。

`src/main.rs`（临时）：
```rust
fn main() {
    println!("live-caption skeleton");
}
```

- [ ] **Step 3: 写基准 example**

`examples/asr_bench.rs`：
```rust
//! 用法：cargo run --release --example asr_bench -- <模型.bin> <wav 16k mono>
use live_caption::asr::{Asr, AsrConfig};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let model = args.get(1).expect("模型路径").clone();
    let wav = args.get(2).expect("wav 路径").clone();

    let mut reader = hound::WavReader::open(&wav)?;
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16000, "基准音频必须 16 kHz");
    assert_eq!(spec.channels, 1, "基准音频必须单声道");
    let pcm: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let audio_sec = pcm.len() as f32 / 16000.0;

    let t0 = Instant::now();
    let mut asr = Asr::load(&AsrConfig { model_path: model, language: "en".into(), threads: 4 })?;
    let load_ms = t0.elapsed().as_millis();

    // 预热一次
    let _ = asr.transcribe(&pcm)?;

    let t1 = Instant::now();
    let text = asr.transcribe(&pcm)?;
    let infer_ms = t1.elapsed().as_millis();

    println!("load_ms={load_ms} audio_sec={audio_sec:.1} infer_ms={infer_ms} rtf={:.2}", infer_ms as f32 / 1000.0 / audio_sec);
    println!("text={text}");
    Ok(())
}
```

- [ ] **Step 4: 首次编译（编 whisper.cpp，需新终端使 Task 0a 环境变量生效）**

```bash
cd "D:/My_data/Claude/live-caption" && cargo build --release --example asr_bench 2>&1 | tail -5
```
Expected: `Finished release ...`。若报 `VULKAN_SDK`、`libclang` 找不到，回 Task 0a Step 5 检查变量并重开终端。

- [ ] **Step 5: 跑 Vulkan 基准**

```bash
cd "D:/My_data/Claude/live-caption" && cargo run --release --example asr_bench -- "D:/My_data/Claude/workspace/local-asr/ggml-large-v3-turbo-q5_0.bin" "tests/fixtures/en-10s.wav"
```
Expected: 打印 `load_ms=... infer_ms=... rtf=...` 与识别文本；文本应含 "quarterly roadmap" 与 "GPU servers"。把整行记下来，Task 0f 用。

- [ ] **Step 6: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: 工程骨架 + whisper-rs Vulkan 基准 example + 英文测试音频"
```

---

## Task 0c：OpenVINO 对照基准

**Files:** 无代码。

- [ ] **Step 1: 起 local-asr 服务（只读使用，不改它任何文件）**

```powershell
Start-Process -FilePath "powershell" -ArgumentList "-NoProfile","-ExecutionPolicy","Bypass","-File","D:\My_data\Claude\workspace\local-asr\start-asr-server.ps1" -WindowStyle Minimized
```
等 20 秒后：
```bash
curl -s -m 5 http://127.0.0.1:8080/healthz
```
Expected: `{"ok":true}`

- [ ] **Step 2: 同一音频计时（跑两次，取第二次）**

```bash
cd "D:/My_data/Claude/live-caption" && for i in 1 2; do curl -s -o /dev/null -w "wall=%{time_total}s\n" -F "file=@tests/fixtures/en-10s.wav" -F "language=en" http://127.0.0.1:8080/v1/audio/transcriptions; done; curl -s -F "file=@tests/fixtures/en-10s.wav" -F "language=en" http://127.0.0.1:8080/v1/audio/transcriptions
```
Expected: 两行墙钟秒数 + 一行 JSON 文本。服务自己的日志 `D:\My_data\Claude\workspace\local-asr\logs\cc-asr.err.log` 末尾有 `asr=...ms rtf=...`，以它为准。

- [ ] **Step 3: 停掉服务（是临时拉起的才停；若它本来就在跑，跳过）**

```powershell
Get-NetTCPConnection -LocalPort 8080 -State Listen | ForEach-Object { Stop-Process -Id $_.OwningProcess -Force }
```

---

## Task 0d：claude 子进程往返计时 + 参数核实

**Files:**
- Create: `scripts/claude_probe.py`

- [ ] **Step 1: 找 claude 可执行文件的完整路径**

```powershell
(Get-Command claude).Source
```
Expected: 形如 `C:\Users\13601\AppData\Roaming\npm\claude.cmd` 或 `...\claude.exe`。这个完整路径写进 `config.toml` 的 `translate.claude_bin`。

- [ ] **Step 2: 写探针脚本**

`scripts/claude_probe.py`：
```python
"""探针：常驻 claude 子进程逐句翻译，打印每句往返毫秒。
用法：python scripts/claude_probe.py "<claude 完整路径>" [模型名]
"""
import json, subprocess, sys, time

SYSTEM = ("你是英译中会议同传。用户每条消息是一句英文会议发言，你只输出对应的简体中文译文，"
          "不加解释、不加引号、不加前缀。专有名词与产品名保留原文。")
claude = sys.argv[1]
model = sys.argv[2] if len(sys.argv) > 2 else "sonnet"
args = [claude, "-p", "--input-format", "stream-json", "--output-format", "stream-json",
        "--verbose", "--model", model, "--system-prompt", SYSTEM, "--disallowedTools", "*"]
p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, encoding="utf-8")

sentences = [
    "Good morning everyone, let's get started.",
    "We need to finalize the GPU server budget before the end of this month.",
    "The migration to the new data center is scheduled for the second week of October.",
]
for s in sentences:
    line = json.dumps({"type": "user", "message": {"role": "user", "content": s}, "parent_tool_use_id": None})
    t0 = time.perf_counter()
    p.stdin.write(line + "\n"); p.stdin.flush()
    while True:
        out = p.stdout.readline()
        if not out:
            print("子进程退出"); sys.exit(1)
        ev = json.loads(out)
        if ev.get("type") == "result":
            ms = (time.perf_counter() - t0) * 1000
            print(f"{ms:.0f}ms  {ev.get('subtype')}  {ev.get('result','')!r}")
            break
p.stdin.close(); p.wait(timeout=10)
```

- [ ] **Step 3: 跑探针（sonnet 与 haiku 各一次）**

```bash
cd "D:/My_data/Claude/live-caption" && PYTHONUTF8=1 python scripts/claude_probe.py "<Step 1 的完整路径>" sonnet && PYTHONUTF8=1 python scripts/claude_probe.py "<Step 1 的完整路径>" haiku
```
Expected: 每个模型 3 行，形如 `1234ms  success  '大家早上好，我们开始吧。'`。译文里不应出现工具调用痕迹或解释。若报 `--disallowedTools` 不识别，改用 `--tools ""` 再试并把可用的写进 Task 6 的 `FIXED_ARGS`。

- [ ] **Step 4: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add scripts/claude_probe.py && git commit -m "chore: claude 子进程往返探针"
```

---

## Task 0e：WASAPI loopback 采集冒烟

**Files:**
- Create: `examples/capture_smoke.rs`

- [ ] **Step 1: 写冒烟 example（录 5 秒系统声音到 wav，原始混音格式）**

`examples/capture_smoke.rs`：
```rust
//! 用法：先在电脑里放一段英文视频，再 cargo run --release --example capture_smoke
//! 产出 target/smoke.wav（原始采样率、声道数）
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode};

fn main() -> anyhow::Result<()> {
    wasapi::initialize_mta().ok().expect("COM 初始化失败");
    let enumerator = DeviceEnumerator::new().map_err(|e| anyhow::anyhow!("{e}"))?;
    let device = enumerator.get_default_device(&Direction::Render).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut client = device.get_iaudioclient().map_err(|e| anyhow::anyhow!("{e}"))?;
    let fmt = client.get_mixformat().map_err(|e| anyhow::anyhow!("{e}"))?;
    let rate = fmt.get_samplespersec();
    let channels = fmt.get_nchannels();
    let bits = fmt.get_bitspersample();
    println!("mixformat: {rate} Hz, {channels} ch, {bits} bit, {:?}", fmt.get_subformat());
    assert!(matches!(fmt.get_subformat(), Ok(SampleType::Float)) && bits == 32, "第一版只支持 32 位浮点混音格式");

    let (_def, min) = client.get_device_period().map_err(|e| anyhow::anyhow!("{e}"))?;
    client
        .initialize_client(&fmt, &Direction::Capture, &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: min })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let h_event = client.set_get_eventhandle().map_err(|e| anyhow::anyhow!("{e}"))?;
    let cap = client.get_audiocaptureclient().map_err(|e| anyhow::anyhow!("{e}"))?;
    client.start_stream().map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut q: VecDeque<u8> = VecDeque::new();
    let mut samples: Vec<f32> = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if h_event.wait_for_event(250).is_err() {
            continue; // 没有应用在放声音时不会来事件，属正常
        }
        cap.read_from_device_to_deque(&mut q).map_err(|e| anyhow::anyhow!("{e}"))?;
        while q.len() >= 4 {
            let b = [q.pop_front().unwrap(), q.pop_front().unwrap(), q.pop_front().unwrap(), q.pop_front().unwrap()];
            samples.push(f32::from_le_bytes(b));
        }
    }
    client.stop_stream().ok();

    let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create("target/smoke.wav", spec)?;
    for s in &samples { w.write_sample(*s)?; }
    w.finalize()?;
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!("captured {} samples ({:.1}s), peak={peak:.3} -> target/smoke.wav", samples.len(), samples.len() as f32 / rate as f32 / channels as f32);
    Ok(())
}
```

- [ ] **Step 2: 放一段英文视频，跑冒烟**

```bash
cd "D:/My_data/Claude/live-caption" && cargo run --release --example capture_smoke
```
Expected: `mixformat: 48000 Hz, 2 ch, 32 bit, Ok(Float)`（数值可能不同），`captured ... (5.0s), peak=0.3xx`；peak 明显大于 0 说明拿到了系统声音。用播放器打开 `target/smoke.wav` 听一下确认是视频里的声音。

- [ ] **Step 3: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add examples/capture_smoke.rs && git commit -m "chore: WASAPI loopback 采集冒烟 example"
```

---

## Task 0f：基准结论写入设计文档附录 A，定案识别后端

**Files:**
- Modify: `docs/superpowers/specs/2026-09-03-live-caption-design.md`（附录 A）

- [ ] **Step 1: 填附录 A**

把"（空）"替换为：
```markdown
| 项目 | 结果 |
|---|---|
| Vulkan large-v3-turbo q5_0 | load ___ ms · infer ___ ms · rtf ___ · 文本：___ |
| OpenVINO large-v3 int8（8080） | asr ___ ms · rtf ___ · 文本：___ |
| claude sonnet 单句往返 | ___ / ___ / ___ ms |
| claude haiku 单句往返 | ___ / ___ / ___ ms |
| loopback 冒烟 | mixformat ___ · peak ___ |
| 工具关闭参数 | `--disallowedTools "*"`（或实测可用的那个） |

**判定**：Vulkan infer ≤ 2 × OpenVINO asr → 进程内 whisper-rs（Task 5）；否则 OpenVINO 侧车（Task 5-alt）。
本次结论：____
```

- [ ] **Step 2: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add docs && git commit -m "docs: 附录 A 基准结果与识别后端定案"
```

---

## Task 1：配置模块

**Files:**
- Create: `src/config.rs`, `config.example.toml`
- Modify: `src/lib.rs`（取消 `pub mod config;` 注释）

- [ ] **Step 1: 写失败测试（放在 `src/config.rs` 底部）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip() {
        let cfg = Config::default();
        let s = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.audio.silence_ms, 500);
        assert_eq!(back.asr.language, "en");
        assert_eq!(back.translate.rotate_every, 60);
        assert!((back.ui.opacity - 0.75).abs() < 1e-6);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[ui]\nfont_size = 30.0\n").unwrap();
        assert_eq!(cfg.ui.font_size, 30.0);
        assert_eq!(cfg.audio.max_segment_ms, 8000);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test config 2>&1 | tail -3
```
Expected: 编译错误 `cannot find type Config`。

- [ ] **Step 3: 实现**

`src/config.rs`：
```rust
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub audio: AudioCfg,
    pub asr: AsrCfg,
    pub translate: TranslateCfg,
    pub ui: UiCfg,
    pub paths: PathsCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioCfg {
    /// RMS 能量门限（0–1）
    pub energy_threshold: f32,
    pub silence_ms: u32,
    pub min_segment_ms: u32,
    pub max_segment_ms: u32,
}
impl Default for AudioCfg {
    fn default() -> Self {
        Self { energy_threshold: 0.01, silence_ms: 500, min_segment_ms: 1000, max_segment_ms: 8000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrCfg {
    pub model_path: String,
    pub language: String,
    pub threads: i32,
}
impl Default for AsrCfg {
    fn default() -> Self {
        Self {
            model_path: r"D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin".into(),
            language: "en".into(),
            threads: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslateCfg {
    /// claude 可执行文件完整路径（Task 0d Step 1 得到）
    pub claude_bin: String,
    pub model: String,
    pub timeout_secs: u64,
    pub rotate_every: u32,
    pub max_backlog: usize,
}
impl Default for TranslateCfg {
    fn default() -> Self {
        Self { claude_bin: "claude".into(), model: "sonnet".into(), timeout_secs: 8, rotate_every: 60, max_backlog: 10 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiCfg {
    pub font_size: f32,
    pub opacity: f32,
    pub font_path: String,
    /// 窗口外框位置与大小（屏幕像素）；x/y 为 -1 表示首次启动用系统默认位置
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Default for UiCfg {
    fn default() -> Self {
        Self { font_size: 22.0, opacity: 0.75, font_path: r"C:\Windows\Fonts\msyh.ttc".into(), x: -1, y: -1, width: 1100, height: 170 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsCfg {
    pub transcripts_dir: String,
    pub logs_dir: String,
}
impl Default for PathsCfg {
    fn default() -> Self {
        Self { transcripts_dir: "transcripts".into(), logs_dir: "logs".into() }
    }
}

impl Config {
    /// 工作目录下的 config.toml 优先（cargo run 时），否则 exe 同目录
    pub fn path() -> PathBuf {
        let cwd = PathBuf::from("config.toml");
        if cwd.exists() { return cwd; }
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."));
        exe_dir.join("config.toml")
    }

    pub fn load() -> anyhow::Result<Config> {
        let p = Self::path();
        if p.exists() {
            let s = std::fs::read_to_string(&p)?;
            Ok(toml::from_str(&s)?)
        } else {
            let cfg = Config::default();
            cfg.save()?;
            Ok(cfg)
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::write(Self::path(), toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
```

`config.example.toml`（内容与默认值一致，供人看）：
```toml
[audio]
energy_threshold = 0.01   # RMS 门限，越小越灵敏
silence_ms = 500          # 静音多久算一句结束
min_segment_ms = 1000     # 短于此丢弃
max_segment_ms = 8000     # 长于此强制切

[asr]
model_path = 'D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin'
language = "en"
threads = 4

[translate]
claude_bin = 'C:\Users\13601\AppData\Roaming\npm\claude.cmd'   # 用 (Get-Command claude).Source 的结果
model = "sonnet"          # 或 haiku
timeout_secs = 8
rotate_every = 60
max_backlog = 10

[ui]
font_size = 22.0
opacity = 0.75
font_path = 'C:\Windows\Fonts\msyh.ttc'
x = -1
y = -1
width = 1100
height = 170

[paths]
transcripts_dir = "transcripts"
logs_dir = "logs"
```

- [ ] **Step 4: 跑测试**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test config 2>&1 | tail -3
```
Expected: `test result: ok. 2 passed`

- [ ] **Step 5: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: 配置模块（toml，缺省填充）"
```

---

## Task 2：消息类型 + 重采样

**Files:**
- Create: `src/messages.rs`, `src/audio/mod.rs`, `src/audio/resample.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: 消息类型（无测试，纯数据）**

`src/messages.rs`：
```rust
use chrono::{DateTime, Local};

pub type SegId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage { Capture, Asr, Translate }

#[derive(Debug, Clone, PartialEq)]
pub enum StageState { Ok, Warn(String), Err(String) }

#[derive(Debug, Clone)]
pub enum UiMsg {
    English { id: SegId, at: DateTime<Local>, text: String },
    Chinese { id: SegId, text: String },
    Status { stage: Stage, state: StageState },
}
```

`src/audio/mod.rs`：
```rust
pub mod capture;
pub mod resample;
```
（`capture` 在 Task 4 才有文件，本 Task 先注释掉 `pub mod capture;`。）

- [ ] **Step 2: 写重采样失败测试（`src/audio/resample.rs` 底部）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        let stereo = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(downmix_to_mono(&stereo, 2), vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn resample_48k_to_16k_length() {
        let mut r = Resampler::new(48000, 16000);
        let out = r.process(&vec![0.25; 4800]);
        assert!((1599..=1601).contains(&out.len()), "len={}", out.len());
        assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-6));
    }

    #[test]
    fn resample_is_continuous_across_chunks() {
        // 两块各 48 个样本 vs 一块 96 个，结果应一致
        let ramp: Vec<f32> = (0..96).map(|i| i as f32).collect();
        let mut a = Resampler::new(48000, 16000);
        let whole = a.process(&ramp);
        let mut b = Resampler::new(48000, 16000);
        let mut parts = b.process(&ramp[..48]);
        parts.extend(b.process(&ramp[48..]));
        assert_eq!(whole.len(), parts.len());
        for (x, y) in whole.iter().zip(parts.iter()) {
            assert!((x - y).abs() < 1e-4, "{x} vs {y}");
        }
    }

    #[test]
    fn same_rate_passthrough() {
        let mut r = Resampler::new(16000, 16000);
        assert_eq!(r.process(&[1.0, 2.0, 3.0]), vec![1.0, 2.0, 3.0]);
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test resample 2>&1 | tail -3
```
Expected: 编译错误（`Resampler` 未定义）。

- [ ] **Step 4: 实现**

`src/audio/resample.rs`：
```rust
/// 交错多声道 → 单声道（各声道平均）
pub fn downmix_to_mono(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// 线性插值重采样，跨块保持相位连续。
/// 坐标系：索引 0 = 上一块最后一个样本（若有），1.. = 本块样本；
/// `pos` 是下一个输出点在该坐标系里的位置，处理完一块后把坐标系平移到本块末样本。
pub struct Resampler {
    step: f64,
    pos: f64,
    last: Option<f32>,
}

impl Resampler {
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        Self { step: from_hz as f64 / to_hz as f64, pos: 0.0, last: None }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.step - 1.0).abs() < 1e-12 {
            return input.to_vec();
        }
        if input.is_empty() {
            return Vec::new();
        }
        let offset = if self.last.is_some() { 1 } else { 0 };
        let total = input.len() + offset;
        let sample = |i: usize| -> f32 {
            if offset == 1 && i == 0 { self.last.unwrap() } else { input[i - offset] }
        };
        let mut out = Vec::with_capacity(input.len() / self.step as usize + 2);
        // 只在能做插值（i+1 存在）时产出；余下位置留到下一块
        while self.pos < (total - 1) as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = sample(i);
            let b = sample(i + 1);
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= (total - 1) as f64;
        self.last = Some(*input.last().unwrap());
        out
    }
}
```

- [ ] **Step 5: 跑测试**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test resample 2>&1 | tail -3
```
Expected: `4 passed`

- [ ] **Step 6: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: 消息类型 + 混音/线性重采样（相位连续）"
```

---

## Task 3：能量 VAD 切句器 + 切句线程

**Files:**
- Create: `src/segmenter.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: 写失败测试（`src/segmenter.rs` 底部）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SegmenterConfig {
        SegmenterConfig { sample_rate: 16000, frame_ms: 20, energy_threshold: 0.01, silence_ms: 500, min_segment_ms: 1000, max_segment_ms: 8000 }
    }
    fn tone(ms: u32) -> Vec<f32> {
        let n = (16000 * ms / 1000) as usize;
        (0..n).map(|i| (i as f32 * 0.2).sin() * 0.3).collect()
    }
    fn silence(ms: u32) -> Vec<f32> { vec![0.0; (16000 * ms / 1000) as usize] }

    #[test]
    fn silence_only_emits_nothing() {
        let mut s = Segmenter::new(cfg());
        assert!(s.push(&silence(5000)).is_empty());
        assert!(s.idle(2000).is_none());
    }

    #[test]
    fn speech_then_silence_emits_one_segment() {
        let mut s = Segmenter::new(cfg());
        assert!(s.push(&tone(3000)).is_empty());
        let segs = s.push(&silence(600));
        assert_eq!(segs.len(), 1);
        let len_ms = segs[0].len() as u32 * 1000 / 16000;
        assert!((3400..=3600).contains(&len_ms), "len_ms={len_ms}");
    }

    #[test]
    fn short_burst_is_dropped() {
        let mut s = Segmenter::new(cfg());
        s.push(&tone(400));
        assert!(s.push(&silence(600)).is_empty());
    }

    #[test]
    fn long_speech_is_force_cut_at_max() {
        let mut s = Segmenter::new(cfg());
        let segs = s.push(&tone(10000));
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].len(), 16000 * 8);
    }

    #[test]
    fn idle_finishes_pending_speech() {
        let mut s = Segmenter::new(cfg());
        s.push(&tone(2000));
        assert!(s.idle(300).is_none());
        let seg = s.idle(300).expect("累计 600ms 静音应出段");
        assert!(seg.len() >= 16000 * 2);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test segmenter 2>&1 | tail -3
```
Expected: 编译错误。

- [ ] **Step 3: 实现**

`src/segmenter.rs`：
```rust
use crate::messages::{Stage, StageState, UiMsg};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    pub sample_rate: u32,
    pub frame_ms: u32,
    pub energy_threshold: f32,
    pub silence_ms: u32,
    pub min_segment_ms: u32,
    pub max_segment_ms: u32,
}

pub struct Segmenter {
    cfg: SegmenterConfig,
    frame_len: usize,
    pending: Vec<f32>, // 不足一帧的余数
    buf: Vec<f32>,     // 当前段
    in_speech: bool,
    silence_acc_ms: u32,
}

impl Segmenter {
    pub fn new(cfg: SegmenterConfig) -> Self {
        let frame_len = (cfg.sample_rate * cfg.frame_ms / 1000) as usize;
        Self { cfg, frame_len, pending: Vec::new(), buf: Vec::new(), in_speech: false, silence_acc_ms: 0 }
    }

    fn ms_to_samples(&self, ms: u32) -> usize { (self.cfg.sample_rate * ms / 1000) as usize }

    /// 喂入任意长度样本，返回本次完成的段
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(samples);
        let mut start = 0;
        while start + self.frame_len <= self.pending.len() {
            let frame: Vec<f32> = self.pending[start..start + self.frame_len].to_vec();
            let rms = (frame.iter().map(|v| v * v).sum::<f32>() / frame.len() as f32).sqrt();
            let voiced = rms >= self.cfg.energy_threshold;
            if voiced {
                if !self.in_speech {
                    self.in_speech = true;
                    self.buf.clear();
                }
                self.silence_acc_ms = 0;
                self.buf.extend_from_slice(&frame);
                if self.buf.len() >= self.ms_to_samples(self.cfg.max_segment_ms) {
                    out.push(std::mem::take(&mut self.buf));
                    self.in_speech = false;
                }
            } else if self.in_speech {
                self.buf.extend_from_slice(&frame);
                self.silence_acc_ms += self.cfg.frame_ms;
                if self.silence_acc_ms >= self.cfg.silence_ms {
                    if let Some(seg) = self.finish() { out.push(seg); }
                }
            }
            start += self.frame_len;
        }
        self.pending.drain(..start);
        out
    }

    /// 没有音频到达时调用（等价于 elapsed_ms 的静音）
    pub fn idle(&mut self, elapsed_ms: u32) -> Option<Vec<f32>> {
        if !self.in_speech { return None; }
        self.silence_acc_ms += elapsed_ms;
        if self.silence_acc_ms >= self.cfg.silence_ms { self.finish() } else { None }
    }

    fn finish(&mut self) -> Option<Vec<f32>> {
        self.in_speech = false;
        self.silence_acc_ms = 0;
        let seg = std::mem::take(&mut self.buf);
        if seg.len() >= self.ms_to_samples(self.cfg.min_segment_ms) { Some(seg) } else { None }
    }
}

/// 切句线程：rx_audio 收 16 kHz 单声道块；tx_seg 发完成的段；paused 为真时丢弃音频
pub fn spawn_segmenter(
    cfg: SegmenterConfig,
    rx_audio: Receiver<Vec<f32>>,
    tx_seg: Sender<Vec<f32>>,
    paused: Arc<AtomicBool>,
    ui: Sender<UiMsg>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new().name("segmenter".into()).spawn(move || {
        let mut seg = Segmenter::new(cfg);
        loop {
            match rx_audio.recv_timeout(Duration::from_millis(100)) {
                Ok(chunk) => {
                    if paused.load(Ordering::Relaxed) { continue; }
                    for s in seg.push(&chunk) {
                        if tx_seg.send(s).is_err() { return; }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(s) = seg.idle(100) {
                        if tx_seg.send(s).is_err() { return; }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = ui.send(UiMsg::Status { stage: Stage::Capture, state: StageState::Err("采集线程已退出".into()) });
                    return;
                }
            }
        }
    }).expect("spawn segmenter")
}
```

- [ ] **Step 4: 跑测试**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test segmenter 2>&1 | tail -3
```
Expected: `5 passed`

- [ ] **Step 5: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: 能量 VAD 切句器 + 切句线程（含 idle 静音推进）"
```

---

## Task 4：WASAPI loopback 采集线程

**Files:**
- Create: `src/audio/capture.rs`
- Modify: `src/audio/mod.rs`（取消注释）、`examples/capture_smoke.rs`（改为调用模块）

- [ ] **Step 1: 实现（把 Task 0e 的冒烟代码收进模块，加重试与重采样）**

`src/audio/capture.rs`：
```rust
use crate::audio::resample::{downmix_to_mono, Resampler};
use crate::messages::{Stage, StageState, UiMsg};
use crossbeam_channel::Sender;
use std::collections::VecDeque;
use std::time::Duration;
use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode};

pub const TARGET_RATE: u32 = 16000;

fn werr<E: std::fmt::Display>(e: E) -> anyhow::Error { anyhow::anyhow!("{e}") }

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
        .initialize_client(&fmt, &Direction::Capture, &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: min })
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
            let b = [q.pop_front().unwrap(), q.pop_front().unwrap(), q.pop_front().unwrap(), q.pop_front().unwrap()];
            interleaved.push(f32::from_le_bytes(b));
        }
        if interleaved.is_empty() { continue; }
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
    std::thread::Builder::new().name("capture".into()).spawn(move || {
        wasapi::initialize_mta().ok().expect("COM 初始化失败");
        loop {
            match run_once(&tx, &ui) {
                Ok(()) => return, // 下游已关闭
                Err(e) => {
                    tracing::warn!("采集失败，2 秒后重试：{e}");
                    let _ = ui.send(UiMsg::Status { stage: Stage::Capture, state: StageState::Err(format!("音频设备重连中：{e}")) });
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
    }).expect("spawn capture")
}
```

- [ ] **Step 2: 冒烟 example 改成走模块（验证 16 kHz 单声道输出）**

`examples/capture_smoke.rs` 整体替换为：
```rust
//! 放一段英文视频，然后 cargo run --release --example capture_smoke → target/smoke16k.wav
use crossbeam_channel::bounded;
use live_caption::audio::capture::{spawn_capture, TARGET_RATE};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let (tx, rx) = bounded::<Vec<f32>>(64);
    let (ui_tx, ui_rx) = bounded(64);
    let _h = spawn_capture(tx, ui_tx);
    let mut samples = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) { samples.extend(chunk); }
        while let Ok(m) = ui_rx.try_recv() { println!("{m:?}"); }
    }
    let spec = hound::WavSpec { channels: 1, sample_rate: TARGET_RATE, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
    let mut w = hound::WavWriter::create("target/smoke16k.wav", spec)?;
    for s in &samples { w.write_sample(*s)?; }
    w.finalize()?;
    let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
    println!("captured {:.1}s @16k mono, peak={peak:.3} -> target/smoke16k.wav", samples.len() as f32 / TARGET_RATE as f32);
    Ok(())
}
```

- [ ] **Step 3: 编译 + 冒烟**

```bash
cd "D:/My_data/Claude/live-caption" && cargo build --release 2>&1 | tail -2 && cargo run --release --example capture_smoke
```
Expected: `Status { stage: Capture, state: Ok }`，`captured 5.0s @16k mono, peak=0.x`；用播放器听 `target/smoke16k.wav` 是清晰人声（不能变调）。

- [ ] **Step 4: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: WASAPI loopback 采集线程（自动重试，输出 16k 单声道）"
```

---

## Task 5：whisper-rs 识别器 + 识别线程（Task 0f 判定进程内时用）

**Files:**
- Create/Modify: `src/asr.rs`, `tests/asr_e2e.rs`

- [ ] **Step 1: 写端到端测试（有模型才跑）**

`tests/asr_e2e.rs`：
```rust
use live_caption::asr::{Asr, AsrConfig};

const MODEL: &str = r"D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin";

#[test]
#[ignore = "需要模型与 GPU：cargo test --release -- --ignored"]
fn transcribes_english_fixture() {
    let mut reader = hound::WavReader::open("tests/fixtures/en-10s.wav").unwrap();
    let pcm: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    let mut asr = Asr::load(&AsrConfig { model_path: MODEL.into(), language: "en".into(), threads: 4 }).unwrap();
    let text = asr.transcribe(&pcm).unwrap().to_lowercase();
    assert!(text.contains("roadmap"), "text={text}");
    assert!(text.contains("gpu"), "text={text}");
}

#[test]
#[ignore = "需要模型与 GPU"]
fn short_audio_is_padded_not_error() {
    let mut asr = Asr::load(&AsrConfig { model_path: MODEL.into(), language: "en".into(), threads: 4 }).unwrap();
    let half_sec = vec![0.0f32; 8000];
    assert!(asr.transcribe(&half_sec).is_ok());
}
```

- [ ] **Step 2: 跑确认失败**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test --release --test asr_e2e -- --ignored 2>&1 | tail -3
```
Expected: 编译错误（`Asr` 未定义）或断言失败。

- [ ] **Step 3: 实现**

`src/asr.rs`：
```rust
use crate::messages::{SegId, Stage, StageState, UiMsg};
use chrono::Local;
use crossbeam_channel::{Receiver, Sender};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

#[derive(Debug, Clone)]
pub struct AsrConfig {
    pub model_path: String,
    pub language: String,
    pub threads: i32,
}

pub struct Asr {
    _ctx: WhisperContext,
    state: WhisperState,
    cfg: AsrConfig,
    prev_text: String,
}

const MIN_SAMPLES: usize = 16000 * 12 / 10; // whisper.cpp 要求 ≥1 s，补到 1.2 s

impl Asr {
    pub fn load(cfg: &AsrConfig) -> anyhow::Result<Self> {
        let ctx = WhisperContext::new_with_params(&cfg.model_path, WhisperContextParameters::default())
            .map_err(|e| anyhow::anyhow!("加载模型失败 {}: {e:?}", cfg.model_path))?;
        let state = ctx.create_state().map_err(|e| anyhow::anyhow!("create_state: {e:?}"))?;
        Ok(Self { _ctx: ctx, state, cfg: cfg.clone(), prev_text: String::new() })
    }

    pub fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        let mut audio = pcm.to_vec();
        if audio.len() < MIN_SAMPLES { audio.resize(MIN_SAMPLES, 0.0); }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.cfg.language));
        params.set_translate(false);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_n_threads(self.cfg.threads);
        if !self.prev_text.is_empty() { params.set_initial_prompt(&self.prev_text); }

        self.state.full(params, &audio).map_err(|e| anyhow::anyhow!("full: {e:?}"))?;
        let n = self.state.full_n_segments();
        let mut parts = Vec::new();
        for i in 0..n {
            if let Some(seg) = self.state.get_segment(i) {
                parts.push(seg.to_str_lossy().map_err(|e| anyhow::anyhow!("{e:?}"))?.trim().to_string());
            }
        }
        let text = parts.join(" ").trim().to_string();
        if text.chars().any(|c| c.is_alphanumeric()) {
            // 只留最后约 200 个字符做下一段的提示词
            let tail: String = text.chars().rev().take(200).collect::<Vec<_>>().into_iter().rev().collect();
            self.prev_text = tail;
        }
        Ok(text)
    }
}

/// 识别线程：rx_seg 收音频段；ui 收英文；tx_tr 送翻译队列
pub fn spawn_asr(cfg: AsrConfig, rx_seg: Receiver<Vec<f32>>, ui: Sender<UiMsg>, tx_tr: Sender<(SegId, String)>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new().name("asr".into()).spawn(move || {
        let mut asr = match Asr::load(&cfg) {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("{e}");
                let _ = ui.send(UiMsg::Status { stage: Stage::Asr, state: StageState::Err(e.to_string()) });
                return;
            }
        };
        let _ = ui.send(UiMsg::Status { stage: Stage::Asr, state: StageState::Ok });
        let mut next_id: SegId = 1;
        while let Ok(seg) = rx_seg.recv() {
            let at = Local::now();
            let t0 = std::time::Instant::now();
            match asr.transcribe(&seg) {
                Ok(text) if text.chars().any(|c| c.is_alphanumeric()) => {
                    tracing::info!("asr {}ms audio={:.1}s text={text:?}", t0.elapsed().as_millis(), seg.len() as f32 / 16000.0);
                    let id = next_id; next_id += 1;
                    let _ = ui.send(UiMsg::English { id, at, text: text.clone() });
                    let _ = tx_tr.try_send((id, text)); // 满了由翻译线程按积压规则处理；这里不阻塞
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("识别失败，跳过：{e}");
                }
            }
        }
    }).expect("spawn asr")
}
```

- [ ] **Step 4: 跑测试**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test --release --test asr_e2e -- --ignored 2>&1 | tail -3
```
Expected: `2 passed`

- [ ] **Step 5: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: whisper-rs 识别器（Vulkan）+ 识别线程"
```

---

## Task 5-alt：OpenVINO 侧车识别器（仅当 Task 0f 判定侧车时执行，替代 Task 5）

**Files:**
- Modify: `Cargo.toml`（去掉 whisper-rs，加 `reqwest = { version = "0.12", features = ["blocking", "multipart"] }`）、`src/asr.rs`

- [ ] **Step 1: `src/asr.rs` 改为 HTTP 实现，保持 `AsrConfig` / `Asr::load` / `Asr::transcribe` / `spawn_asr` 签名不变**

```rust
use crate::messages::{SegId, Stage, StageState, UiMsg};
use chrono::Local;
use crossbeam_channel::{Receiver, Sender};
use std::io::Cursor;

#[derive(Debug, Clone)]
pub struct AsrConfig {
    /// 侧车地址，如 http://127.0.0.1:8080
    pub model_path: String,
    pub language: String,
    pub threads: i32,
}

pub struct Asr { client: reqwest::blocking::Client, cfg: AsrConfig }

impl Asr {
    pub fn load(cfg: &AsrConfig) -> anyhow::Result<Self> {
        let client = reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(15)).build()?;
        let ok: serde_json::Value = client.get(format!("{}/healthz", cfg.model_path)).send()?.json()?;
        anyhow::ensure!(ok["ok"] == true, "侧车未就绪：{ok}");
        Ok(Self { client, cfg: cfg.clone() })
    }

    pub fn transcribe(&mut self, pcm: &[f32]) -> anyhow::Result<String> {
        let spec = hound::WavSpec { channels: 1, sample_rate: 16000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = hound::WavWriter::new(&mut buf, spec)?;
            for s in pcm { w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?; }
            w.finalize()?;
        }
        let part = reqwest::blocking::multipart::Part::bytes(buf.into_inner()).file_name("seg.wav").mime_str("audio/wav")?;
        let form = reqwest::blocking::multipart::Form::new().part("file", part).text("language", self.cfg.language.clone());
        let v: serde_json::Value = self.client.post(format!("{}/v1/audio/transcriptions", self.cfg.model_path)).multipart(form).send()?.json()?;
        Ok(v["text"].as_str().unwrap_or("").trim().to_string())
    }
}
```
`spawn_asr` 与 Task 5 Step 3 的完全相同（原样复制）。`config.example.toml` 里 `asr.model_path` 改成 `"http://127.0.0.1:8080"`，并在 `CLAUDE.md` 注明"启动前先跑 `local-asr\start-asr-server.ps1`"。

- [ ] **Step 2: 跑 Task 5 的 `tests/asr_e2e.rs`（MODEL 常量改成 `"http://127.0.0.1:8080"`，服务先起）**

Expected: `2 passed`

- [ ] **Step 3: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: OpenVINO 侧车识别器（HTTP）"
```

---

## Task 6：翻译协议 + claude 常驻子进程线程

**Files:**
- Create: `src/translate.rs`, `tests/fake_claude.py`, `tests/translator.rs`

- [ ] **Step 1: 协议纯函数的单元测试（`src/translate.rs` 底部）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_line_is_single_line_json() {
        let l = user_line("hello \"world\"\nnext");
        assert!(!l.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&l).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], "hello \"world\"\nnext");
    }

    #[test]
    fn parse_result_success() {
        let r = parse_result_line(r#"{"type":"result","subtype":"success","result":"你好"}"#).unwrap();
        assert_eq!(r.text, "你好");
        assert!(!r.is_error);
    }

    #[test]
    fn parse_result_error_subtype() {
        let r = parse_result_line(r#"{"type":"result","subtype":"error_during_execution","result":"","is_error":true}"#).unwrap();
        assert!(r.is_error);
    }

    #[test]
    fn non_result_lines_are_none() {
        assert!(parse_result_line(r#"{"type":"system","subtype":"init"}"#).is_none());
        assert!(parse_result_line("not json").is_none());
    }
}
```

- [ ] **Step 2: 跑确认失败**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test translate 2>&1 | tail -3
```
Expected: 编译错误。

- [ ] **Step 3: 实现**

`src/translate.rs`：
```rust
use crate::messages::{SegId, Stage, StageState, UiMsg};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub const SYSTEM_PROMPT: &str = "你是英译中会议同传。用户每条消息是一句英文会议发言，你只输出对应的简体中文译文，不加解释、不加引号、不加前缀。专有名词与产品名保留原文。";

/// Task 0d 实测可用的固定参数（若实测是 --tools ""，改这里）
pub const FIXED_ARGS: &[&str] = &["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--disallowedTools", "*"];

#[derive(Debug, Clone)]
pub struct TranslatorConfig {
    /// 命令与前置参数，如 ["C:\\...\\claude.cmd"] 或测试用 ["python", "tests/fake_claude.py"]
    pub command: Vec<String>,
    pub model: String,
    pub timeout: Duration,
    pub rotate_every: u32,
    pub max_backlog: usize,
}

pub fn user_line(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
        "parent_tool_use_id": null
    })
    .to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResultEvent { pub text: String, pub is_error: bool }

pub fn parse_result_line(line: &str) -> Option<ResultEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "result" { return None; }
    let subtype = v.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
    let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false) || subtype.starts_with("error");
    let text = v.get("result").and_then(|s| s.as_str()).unwrap_or("").trim().to_string();
    Some(ResultEvent { text, is_error })
}

struct Session {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: Receiver<String>,
    turns: u32,
}

impl Session {
    fn spawn(cfg: &TranslatorConfig) -> anyhow::Result<Session> {
        let (prog, pre) = cfg.command.split_first().ok_or_else(|| anyhow::anyhow!("command 为空"))?;
        let mut cmd = Command::new(prog);
        cmd.args(pre).args(FIXED_ARGS).args(["--model", &cfg.model, "--system-prompt", SYSTEM_PROMPT])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("启动 {prog} 失败：{e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = bounded::<String>(256);
        std::thread::Builder::new().name("claude-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() { break; }
            }
        })?;
        Ok(Session { child, stdin, lines: rx, turns: 0 })
    }

    /// 送一句，等 result；超时或子进程退出返回 Err
    fn translate(&mut self, text: &str, timeout: Duration) -> anyhow::Result<String> {
        writeln!(self.stdin, "{}", user_line(text))?;
        self.stdin.flush()?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() { anyhow::bail!("超时"); }
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Some(r) = parse_result_line(&line) {
                        self.turns += 1;
                        if r.is_error { anyhow::bail!("claude 返回错误：{}", r.text); }
                        return Ok(r.text);
                    }
                }
                Err(RecvTimeoutError::Timeout) => anyhow::bail!("超时"),
                Err(RecvTimeoutError::Disconnected) => anyhow::bail!("子进程已退出"),
            }
        }
    }

    fn kill(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

impl Drop for Session { fn drop(&mut self) { self.kill(); } }

/// 翻译线程
pub fn spawn_translator(cfg: TranslatorConfig, rx: Receiver<(SegId, String)>, ui: Sender<UiMsg>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new().name("translator".into()).spawn(move || {
        let mut session: Option<Session> = None;
        loop {
            // 取下一句；合并积压，超过 max_backlog 的旧句直接标记跳过
            let first = match rx.recv() { Ok(x) => x, Err(_) => return };
            let mut backlog = vec![first];
            while let Ok(x) = rx.try_recv() { backlog.push(x); }
            if backlog.len() > cfg.max_backlog {
                let drop_n = backlog.len() - cfg.max_backlog;
                for (id, _) in backlog.drain(..drop_n) {
                    let _ = ui.send(UiMsg::Chinese { id, text: "（积压过多，已跳过）".into() });
                }
            }
            let state = if backlog.len() > 3 { StageState::Warn(format!("翻译积压 {} 句", backlog.len())) } else { StageState::Ok };
            let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state });

            for (id, en) in backlog {
                if session.as_ref().map_or(true, |s| s.turns >= cfg.rotate_every) {
                    session = None;
                    match Session::spawn(&cfg) {
                        Ok(s) => session = Some(s),
                        Err(e) => {
                            tracing::error!("{e}");
                            let _ = ui.send(UiMsg::Status { stage: Stage::Translate, state: StageState::Err(e.to_string()) });
                            let _ = ui.send(UiMsg::Chinese { id, text: "（翻译不可用）".into() });
                            continue;
                        }
                    }
                }
                let t0 = std::time::Instant::now();
                match session.as_mut().unwrap().translate(&en, cfg.timeout) {
                    Ok(zh) => {
                        tracing::info!("translate {}ms {zh:?}", t0.elapsed().as_millis());
                        let _ = ui.send(UiMsg::Chinese { id, text: zh });
                    }
                    Err(e) => {
                        tracing::warn!("翻译失败，重启会话：{e}");
                        session = None;
                        let _ = ui.send(UiMsg::Chinese { id, text: "（翻译超时）".into() });
                    }
                }
            }
        }
    }).expect("spawn translator")
}
```

- [ ] **Step 4: 单元测试通过**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test translate 2>&1 | tail -3
```
Expected: `4 passed`

- [ ] **Step 5: 假子进程 + 集成测试**

`tests/fake_claude.py`：
```python
"""冒充 claude -p stream-json：每收一行 user 消息，回一行 result。
文本含 SLEEP 时睡 20 秒（测超时）；译文带 pid 便于验证会话轮换。"""
import json, os, sys, time
sys.stdin.reconfigure(encoding="utf-8"); sys.stdout.reconfigure(encoding="utf-8")
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    text = msg["message"]["content"]
    if "SLEEP" in text:
        time.sleep(20)
    out = {"type": "result", "subtype": "success", "result": f"pid={os.getpid()} zh={text}"}
    sys.stdout.write(json.dumps(out, ensure_ascii=False) + "\n"); sys.stdout.flush()
```

`tests/translator.rs`：
```rust
use crossbeam_channel::bounded;
use live_caption::messages::UiMsg;
use live_caption::translate::{spawn_translator, TranslatorConfig};
use std::time::Duration;

fn cfg(rotate_every: u32, timeout_secs: u64) -> TranslatorConfig {
    TranslatorConfig {
        command: vec!["python".into(), "tests/fake_claude.py".into()],
        model: "fake".into(),
        timeout: Duration::from_secs(timeout_secs),
        rotate_every,
        max_backlog: 10,
    }
}

fn next_chinese(rx: &crossbeam_channel::Receiver<UiMsg>) -> (u64, String) {
    loop {
        match rx.recv_timeout(Duration::from_secs(15)).expect("等译文超时") {
            UiMsg::Chinese { id, text } => return (id, text),
            _ => continue,
        }
    }
}

fn pid_of(s: &str) -> &str { s.split_whitespace().next().unwrap() }

#[test]
fn translates_in_order_and_rotates_session() {
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(64);
    let _h = spawn_translator(cfg(2, 5), rx, ui_tx);
    tx.send((1, "one".into())).unwrap();
    tx.send((2, "two".into())).unwrap();
    tx.send((3, "three".into())).unwrap();
    let (id1, t1) = next_chinese(&ui_rx);
    let (id2, t2) = next_chinese(&ui_rx);
    let (id3, t3) = next_chinese(&ui_rx);
    assert_eq!((id1, id2, id3), (1, 2, 3));
    assert!(t1.ends_with("zh=one") && t2.ends_with("zh=two") && t3.ends_with("zh=three"));
    assert_eq!(pid_of(&t1), pid_of(&t2), "前两句同一会话");
    assert_ne!(pid_of(&t2), pid_of(&t3), "第三句应已轮换新会话");
}

#[test]
fn timeout_restarts_session_and_continues() {
    let (tx, rx) = bounded(16);
    let (ui_tx, ui_rx) = bounded(64);
    let _h = spawn_translator(cfg(100, 1), rx, ui_tx);
    tx.send((1, "SLEEP please".into())).unwrap();
    let (id1, t1) = next_chinese(&ui_rx);
    assert_eq!(id1, 1);
    assert_eq!(t1, "（翻译超时）");
    tx.send((2, "after".into())).unwrap();
    let (id2, t2) = next_chinese(&ui_rx);
    assert_eq!(id2, 2);
    assert!(t2.ends_with("zh=after"));
}
```

- [ ] **Step 6: 跑集成测试**

```bash
cd "D:/My_data/Claude/live-caption" && PYTHONUTF8=1 cargo test --test translator 2>&1 | tail -3
```
Expected: `2 passed`

- [ ] **Step 7: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: claude 常驻子进程翻译线程（超时重启/会话轮换/积压丢弃）+ 假子进程集成测试"
```

---

## Task 7：会议记录文件

**Files:**
- Create: `src/transcript.rs`

- [ ] **Step 1: 失败测试（文件底部）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn line_format() {
        let at = chrono::Local.with_ymd_and_hms(2026, 9, 3, 20, 55, 12).unwrap();
        let l = format_line(at, "Hello team.", "大家好。");
        assert_eq!(l, "[20:55:12] EN: Hello team.\n           ZH: 大家好。\n");
    }

    #[test]
    fn pairs_english_with_later_chinese() {
        let at = chrono::Local::now();
        let mut t = Transcript::default();
        assert!(t.push_en(1, at, "a").is_none());
        assert!(t.push_en(2, at, "b").is_none());
        assert!(t.push_zh(2, "乙").unwrap().contains("EN: b"));
        assert!(t.push_zh(1, "甲").unwrap().contains("ZH: 甲"));
        assert!(t.push_zh(9, "无").is_none(), "未知段号忽略");
    }
}
```

- [ ] **Step 2: 跑确认失败**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test transcript 2>&1 | tail -3
```
Expected: 编译错误。

- [ ] **Step 3: 实现**

`src/transcript.rs`：
```rust
use crate::messages::SegId;
use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

pub fn format_line(at: DateTime<Local>, en: &str, zh: &str) -> String {
    format!("[{}] EN: {en}\n           ZH: {zh}\n", at.format("%H:%M:%S"))
}

/// 内存配对：英文先到，中文后到时产出一行
#[derive(Default)]
pub struct Transcript {
    pending: HashMap<SegId, (DateTime<Local>, String)>,
}

impl Transcript {
    pub fn push_en(&mut self, id: SegId, at: DateTime<Local>, en: &str) -> Option<String> {
        self.pending.insert(id, (at, en.to_string()));
        None
    }
    pub fn push_zh(&mut self, id: SegId, zh: &str) -> Option<String> {
        let (at, en) = self.pending.remove(&id)?;
        Some(format_line(at, &en, zh))
    }
}

/// 落盘：transcripts/2026-09-03_2055.md
pub struct TranscriptFile { pub path: PathBuf, file: std::fs::File }

impl TranscriptFile {
    pub fn create(dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = PathBuf::from(dir).join(format!("{}.md", Local::now().format("%Y-%m-%d_%H%M")));
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "# 会议字幕 {}\n", Local::now().format("%Y-%m-%d %H:%M"))?;
        Ok(Self { path, file })
    }
    pub fn append(&mut self, line: &str) {
        if let Err(e) = self.file.write_all(line.as_bytes()) { tracing::warn!("写记录失败：{e}"); }
    }
}
```

- [ ] **Step 4: 跑测试**

```bash
cd "D:/My_data/Claude/live-caption" && cargo test transcript 2>&1 | tail -3
```
Expected: `2 passed`

- [ ] **Step 5: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: 会议记录配对与落盘"
```

---

## Task 8：悬浮窗 UI + main 接线

**Files:**
- Create: `src/ui.rs`
- Modify: `src/main.rs`, `src/lib.rs`

- [ ] **Step 1: UI**

`src/ui.rs`：
```rust
use crate::config::Config;
use crate::messages::{SegId, Stage, StageState, UiMsg};
use crate::transcript::{Transcript, TranscriptFile};
use chrono::{DateTime, Local};
use crossbeam_channel::Receiver;
use egui::{Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Id, Layout, RichText, Sense, ViewportCommand};
use global_hotkey::{hotkey::{Code, HotKey, Modifiers}, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Line { id: SegId, #[allow(dead_code)] at: DateTime<Local>, en: String, zh: Option<String> }

pub struct CaptionApp {
    rx: Receiver<UiMsg>,
    cfg: Config,
    lines: VecDeque<Line>,
    status: HashMap<Stage, StageState>,
    paused: Arc<AtomicBool>,
    passthrough: bool,
    transcript: Transcript,
    file: Option<TranscriptFile>,
    _hotkeys: Option<GlobalHotKeyManager>,
    last_cfg_save: Instant,
}

impl CaptionApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rx: Receiver<UiMsg>, cfg: Config, paused: Arc<AtomicBool>) -> Self {
        if let Err(e) = install_fonts(&cc.egui_ctx, &cfg.ui.font_path) { tracing::warn!("加载中文字体失败：{e}"); }
        let hotkeys = GlobalHotKeyManager::new().ok().and_then(|m| {
            m.register(HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyC)).ok().map(|_| m)
        });
        if hotkeys.is_none() { tracing::warn!("全局热键 Ctrl+Alt+C 注册失败"); }
        let file = match TranscriptFile::create(&cfg.paths.transcripts_dir) {
            Ok(f) => Some(f),
            Err(e) => { tracing::warn!("建记录文件失败：{e}"); None }
        };
        Self { rx, cfg, lines: VecDeque::new(), status: HashMap::new(), paused, passthrough: false, transcript: Transcript::default(), file, _hotkeys: hotkeys, last_cfg_save: Instant::now() }
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::English { id, at, text } => {
                    self.transcript.push_en(id, at, &text);
                    self.lines.push_back(Line { id, at, en: text, zh: None });
                    while self.lines.len() > 2 { self.lines.pop_front(); }
                }
                UiMsg::Chinese { id, text } => {
                    if let Some(l) = self.lines.iter_mut().find(|l| l.id == id) { l.zh = Some(text.clone()); }
                    if let Some(line) = self.transcript.push_zh(id, &text) {
                        if let Some(f) = self.file.as_mut() { f.append(&line); }
                    }
                }
                UiMsg::Status { stage, state } => { self.status.insert(stage, state); }
            }
        }
    }

    fn status_color_and_text(&self) -> (Color32, String) {
        let mut worst = (Color32::from_rgb(70, 200, 90), String::new());
        for (stage, st) in &self.status {
            let name = match stage { Stage::Capture => "采集", Stage::Asr => "识别", Stage::Translate => "翻译" };
            match st {
                StageState::Err(m) => return (Color32::from_rgb(230, 70, 70), format!("{name}：{m}")),
                StageState::Warn(m) => worst = (Color32::from_rgb(240, 190, 60), format!("{name}：{m}")),
                StageState::Ok => {}
            }
        }
        worst
    }

    fn maybe_save_window_rect(&mut self, ctx: &egui::Context) {
        if self.last_cfg_save.elapsed() < Duration::from_secs(1) { return; }
        self.last_cfg_save = Instant::now();
        let Some(r) = ctx.input(|i| i.viewport().outer_rect) else { return };
        let (x, y, w, h) = (r.min.x as i32, r.min.y as i32, r.width() as u32, r.height() as u32);
        if (x, y, w, h) != (self.cfg.ui.x, self.cfg.ui.y, self.cfg.ui.width, self.cfg.ui.height) {
            self.cfg.ui.x = x; self.cfg.ui.y = y; self.cfg.ui.width = w; self.cfg.ui.height = h;
            if let Err(e) = self.cfg.save() { tracing::warn!("保存配置失败：{e}"); }
        }
    }
}

fn install_fonts(ctx: &egui::Context, path: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
    fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "cjk".into());
    ctx.set_fonts(fonts);
    Ok(())
}

impl eframe::App for CaptionApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] { [0.0, 0.0, 0.0, 0.0] }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain();

        // 全局热键：切回可点击
        if let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state == HotKeyState::Pressed && self.passthrough {
                self.passthrough = false;
                ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(false));
            }
        }

        let rect = ui.max_rect();
        let alpha = (self.cfg.ui.opacity.clamp(0.0, 1.0) * 255.0) as u8;
        ui.painter().rect_filled(rect, CornerRadius::same(14), Color32::from_rgba_unmultiplied(12, 12, 16, alpha));

        // 拖动整窗
        let drag = ui.interact(rect, Id::new("window-drag"), Sense::drag());
        if drag.drag_started() { ctx.send_viewport_cmd(ViewportCommand::StartDrag); }

        let hovered = ui.rect_contains_pointer(rect);
        egui::Frame::NONE.inner_margin(14.0).show(ui, |ui| {
            // 顶部：状态点 + 提示 + 悬停工具条
            ui.horizontal(|ui| {
                let (color, text) = self.status_color_and_text();
                let dot = ui.allocate_response(egui::vec2(12.0, 12.0), Sense::hover());
                ui.painter().circle_filled(dot.rect.center(), 5.0, color);
                if !text.is_empty() { ui.label(RichText::new(text).size(12.0).color(Color32::from_gray(200))); }
                if hovered {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("✕").on_hover_text("退出").clicked() { ctx.send_viewport_cmd(ViewportCommand::Close); }
                        if ui.small_button("穿透").on_hover_text("鼠标穿透，Ctrl+Alt+C 切回").clicked() {
                            self.passthrough = true;
                            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));
                        }
                        if ui.small_button("记录").on_hover_text("打开记录文件").clicked() {
                            if let Some(f) = &self.file {
                                let _ = std::process::Command::new("cmd").args(["/C", "start", "", &f.path.to_string_lossy()]).spawn();
                            }
                        }
                        if ui.small_button("透-").clicked() { self.cfg.ui.opacity = (self.cfg.ui.opacity - 0.1).max(0.1); }
                        if ui.small_button("透+").clicked() { self.cfg.ui.opacity = (self.cfg.ui.opacity + 0.1).min(1.0); }
                        if ui.small_button("A-").clicked() { self.cfg.ui.font_size = (self.cfg.ui.font_size - 2.0).max(12.0); }
                        if ui.small_button("A+").clicked() { self.cfg.ui.font_size = (self.cfg.ui.font_size + 2.0).min(60.0); }
                        let paused = self.paused.load(Ordering::Relaxed);
                        if ui.small_button(if paused { "▶ 继续" } else { "⏸ 暂停" }).clicked() { self.paused.store(!paused, Ordering::Relaxed); }
                    });
                }
            });
            ui.add_space(4.0);
            let fs = self.cfg.ui.font_size;
            let n = self.lines.len();
            for (i, l) in self.lines.iter().enumerate() {
                let dim = if i + 1 < n { 150 } else { 255 };
                ui.label(RichText::new(&l.en).size(fs * 0.85).color(Color32::from_rgba_unmultiplied(200, 210, 230, dim)));
                match &l.zh {
                    Some(zh) => { ui.label(RichText::new(zh).size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, dim))); }
                    None => { ui.label(RichText::new("…").size(fs).color(Color32::from_rgba_unmultiplied(255, 255, 255, 90))); }
                }
                ui.add_space(6.0);
            }
        });

        self.maybe_save_window_rect(&ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
```

- [ ] **Step 2: main 接线**

`src/main.rs`：
```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crossbeam_channel::bounded;
use live_caption::asr::{spawn_asr, AsrConfig};
use live_caption::audio::capture::spawn_capture;
use live_caption::config::Config;
use live_caption::segmenter::{spawn_segmenter, SegmenterConfig};
use live_caption::translate::{spawn_translator, TranslatorConfig};
use live_caption::ui::CaptionApp;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

fn init_logging(dir: &str) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(dir)?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(7)
        .filename_prefix("live-caption")
        .filename_suffix("log")
        .build(dir)?;
    let (nb, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt().with_writer(nb).with_ansi(false).with_env_filter(tracing_subscriber::EnvFilter::new("info")).init();
    Ok(guard)
}

fn main() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    let _guard = init_logging(&cfg.paths.logs_dir)?;
    tracing::info!("启动，配置文件 {}", Config::path().display());

    let (tx_audio, rx_audio) = bounded::<Vec<f32>>(256);
    let (tx_seg, rx_seg) = bounded::<Vec<f32>>(8);
    let (tx_tr, rx_tr) = bounded(64);
    let (tx_ui, rx_ui) = bounded(256);
    let paused = Arc::new(AtomicBool::new(false));

    spawn_capture(tx_audio, tx_ui.clone());
    spawn_segmenter(
        SegmenterConfig { sample_rate: 16000, frame_ms: 20, energy_threshold: cfg.audio.energy_threshold, silence_ms: cfg.audio.silence_ms, min_segment_ms: cfg.audio.min_segment_ms, max_segment_ms: cfg.audio.max_segment_ms },
        rx_audio, tx_seg, paused.clone(), tx_ui.clone(),
    );
    spawn_asr(AsrConfig { model_path: cfg.asr.model_path.clone(), language: cfg.asr.language.clone(), threads: cfg.asr.threads }, rx_seg, tx_ui.clone(), tx_tr);
    spawn_translator(
        TranslatorConfig { command: vec![cfg.translate.claude_bin.clone()], model: cfg.translate.model.clone(), timeout: Duration::from_secs(cfg.translate.timeout_secs), rotate_every: cfg.translate.rotate_every, max_backlog: cfg.translate.max_backlog },
        rx_tr, tx_ui.clone(),
    );

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("live-caption")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false)
        .with_inner_size([cfg.ui.width as f32, cfg.ui.height as f32]);
    if cfg.ui.x >= 0 && cfg.ui.y >= 0 {
        viewport = viewport.with_position([cfg.ui.x as f32, cfg.ui.y as f32]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let paused_ui = paused.clone();
    eframe::run_native("live-caption", options, Box::new(move |cc| Ok(Box::new(CaptionApp::new(cc, rx_ui, cfg, paused_ui)))))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}
```

- [ ] **Step 3: 编译并首次运行（放英文视频）**

```bash
cd "D:/My_data/Claude/live-caption" && cargo build --release 2>&1 | grep -E "^(warning|error)" | head; cargo run --release
```
Expected: 屏幕上出现半透明深色悬浮窗；视频里每句英文说完 1–2 秒出英文、随后中文补上；`transcripts/` 下生成当天文件并逐句追加；`logs/live-caption.<日期>.log` 里有 `asr ...ms` 与 `translate ...ms`。首次启动 x/y 为 -1 时窗口出现在系统默认位置，拖到屏幕下沿后位置会记进 config.toml。

- [ ] **Step 4: 逐项人工验收（写在交接卡的"验收记录"里）**

- 拖动窗口 → 关闭再启动位置复原
- 悬停出现按钮；A+/A-、透+/透- 即时生效；暂停后不再出新字幕，继续后恢复
- 点"穿透"后能点到下面的窗口；Ctrl+Alt+C 后按钮又能点
- 拔掉/切换默认播放设备 → 红点提示，恢复后自动继续
- 断网 → 中文行显示"（翻译超时）"，英文照常；恢复后新句正常

- [ ] **Step 5: 提交**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "feat: eframe 悬浮字幕窗 + 主程序接线"
```

---

## Task 9：收尾——交接卡、启动脚本、版本标记

**Files:**
- Modify: `CLAUDE.md`
- Create: `start.ps1`

- [ ] **Step 1: 启动脚本**

`start.ps1`：
```powershell
Set-Location $PSScriptRoot
if (-not (Test-Path ".\config.toml")) { Copy-Item ".\config.example.toml" ".\config.toml" }
Start-Process -FilePath ".\target\release\live-caption.exe" -WorkingDirectory $PSScriptRoot
```

- [ ] **Step 2: 交接卡补全**

在 `CLAUDE.md` 追加：
```markdown
## 运行
- `start.ps1`（或 `cargo run --release`）；`config.toml` 里 `translate.claude_bin` 必须是完整路径，`(Get-Command claude).Source` 取。
- 模型文件只读引用 `D:\My_data\Claude\workspace\local-asr\ggml-large-v3-turbo-q5_0.bin`，**不要改动 local-asr 目录任何内容**。
- 记录在 `transcripts\`，日志在 `logs\`（保留 7 天）。

## 硬规则
- 会议音频不出本机；只有识别出的英文文本送 claude。
- 识别后端定案见设计文档附录 A；换后端只改 `src/asr.rs`，签名不变。
- 翻译子进程参数 `FIXED_ARGS` 是 Task 0d 实测过的，升级 claude CLI 后先跑 `scripts/claude_probe.py` 再改。

## 已知限制（第一版）
- 混音格式必须 32 位浮点（Windows 默认如此）；不支持麦克风。
- 能量 VAD，背景音乐大时会切不开；调 `audio.energy_threshold`。

## 验收记录
（Task 8 Step 4 的结果）
```

- [ ] **Step 3: 提交并打标签**

```bash
cd "D:/My_data/Claude/live-caption" && git add -A && git commit -m "chore: 启动脚本 + 交接卡" && git tag v0.1.0
```
