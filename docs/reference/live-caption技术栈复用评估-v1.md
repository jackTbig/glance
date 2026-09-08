# live-caption 技术栈复用评估 v1

- 日期：2026-09-08
- 决策：Jack 确认新项目复用 live-caption 技术栈（Rust + eframe/egui + 三后端翻译封装 + Inno Setup）
- 源项目：`D:\My_data\Claude\live-caption`，版本 1.0.2（2026-09-07），源码 3,051 行

## 1. 技术栈事实
- Rust 2021，本机 rustc 1.98.0，无 rust-toolchain.toml
- GUI：eframe 0.36 + egui 0.36，渲染器固定 glow（OpenGL）。原因 `Cargo.toml:15-16`：wgpu 在本机 Arc 核显上不给带 alpha 的合成模式，透明窗变黑底
- 无 tokio / 无 async，全部 std::thread + crossbeam-channel
- Windows 依赖：windows-sys 0.61、tray-icon 0.24（不挂 muda，自己调 Win32 弹菜单）、global-hotkey 0.8、raw-window-handle 0.6
- 悬浮窗：`src/main.rs:89-99` ViewportBuilder 无边框+透明+置顶+不上任务栏；`src/ui.rs:1248` clear_color 全零；圆角背景自绘；鼠标穿透用 `ViewportCommand::MousePassthrough`；子窗口用 `show_viewport_immediate`（`ui.rs:1496-1515`）
- 配置：TOML，`src/config.rs`，开发目录 vs `%APPDATA%\live-caption\config.toml` 判据在 `config.rs:252-258`；有版本迁移 `CONFIG_VERSION=3` / `migrate()`
- 日志：tracing + 按天滚动保留 7 天，`src/main.rs:14-29`
- 打包：Inno Setup 7，`installer/live-caption.iss` + `scripts/build_installer.sh`；无 cargo-dist / CI / 签名 / 自动更新；版本号手改 Cargo.toml 与 .iss 两处

## 2. 翻译后端封装（`src/translate.rs`，814 行）
- 后端 enum：claude（常驻 claude CLI 子进程，stream-json，流式；`CLAUDE_FIXED_ARGS` 把单句 4–7s 压到 1.5s）、codex（每句 fork `codex exec`，非流式，最慢 3s+）、openai（`{base_url}/chat/completions`，reqwest blocking + 手工 SSE，覆盖 DeepSeek/通义/网关/Ollama）
- 不是 trait，是 `enum Engine` 分发；签名 `translate(&mut self, cfg, text, on_delta: &mut dyn FnMut(&str)) -> Result<String>`，回调回传累计全文
- 热切换：配置通过 channel 投给翻译线程，下一句生效
- 有：背压 max_backlog、claude 会话轮换 rotate_every、最近 N 句上下文
- 没有：重试、QPS 限流、并发池、批量请求、HTML 标签保护、术语表
- 测试夹具：`tests/translator.rs`（14 用例）+ `tests/fake_claude.py` + `tests/fake_openai.py`，不烧 token

## 3. 复用三档
### 可直接复用（改动 <10%）
- `translate.rs` 1–501 行：常量、Backend、TranslatorConfig、提示词构造、协议解析、三后端连接、Engine
- 假后端测试夹具三件
- `config.rs:249-339` 配置框架（目录判据 + 版本迁移）
- `main.rs:14-29` 日志初始化
- 透明置顶悬浮窗骨架约 30 行 + 多视口子窗口写法
- Win32 小工具：`show_native_tray_menu`、`open_path`（ShellExecuteW）、`main_hwnd`
- Inno Setup 打包链（去掉模型组件）
- live-caption `CLAUDE.md` 的踩坑清单：glow vs wgpu 透明、tray-icon 不用 muda、`windows_subsystem="windows"` 下不能 `cmd /C start`、PowerShell 脚本必须 ASCII

### 需改造
- `spawn_translator` / `spawn_partial_translator` 与字幕 UiMsg 硬耦合，不搬，新写并发池
- `TranslatorConfig` 的 `From<&TranslateCfg>` 绑死宿主配置，抽 crate 时依赖倒置
- `SYSTEM_PROMPT` 写死英译中同传，要参数化为源/目标语言 × 整页/悬停/划词三场景
- 同步阻塞单线程模型 → 整页翻译需要批量接口 + 并发度控制 + 指数退避重试（当前一个都没有）
- 划词弹窗定位需新写：跟随鼠标/选区坐标、多显示器边界收敛、`WS_EX_NOACTIVATE` 不抢焦点（现项目没做）
- 配置字段全换

### 不适用
- `asr.rs`、`audio/*`、`segmenter.rs`、`transcript.rs`，whisper-rs / wasapi / hound 依赖
- 全部 examples、`tests/asr_e2e.rs`、语音相关脚本
- Vulkan SDK / CMake / LLVM / Ninja 编译环境与 `scripts/env.sh`（去掉 whisper 后首次编译从 5–10 分钟降到 1 分钟内）
- 字幕专属 UI：Line 结构、打字机、迷你条、识别/切句设置页
- eframe/egui 路线对浏览器扩展部分完全不适用，浏览器侧必须另起 TS/JS 扩展工程

## 4. 两个关键判断
- **抽共享 crate `crates/ai-translate/`：能，推荐，约 1–2 天**。搬 translate.rs 1–501 行，公开 `Translator::new(cfg)` / `.translate(text, on_delta)`；live-caption 反向依赖它，只留薄胶水。新项目自加并发池、重试、批处理、标签保护。
- **整页翻译实际只有 openai 兼容后端可用**：claude/codex 是常驻 CLI 子进程，每并发一个约 300 MB 进程，不适合几十并发；留给划词/悬停低频单句。
- **悬浮窗复用的是模式不是代码**：透明置顶 + glow + clear_color 全零那套约 30 行值一天调试；交互逻辑（瞬时出现、失焦即关、不抢焦点、跟随光标）与字幕条正交，主循环重写。
- 若桌面端要渲染富文本/网页，egui 吃力，届时换 tauri，Rust 翻译 crate 仍可复用，这是抽 crate 的最大价值。
