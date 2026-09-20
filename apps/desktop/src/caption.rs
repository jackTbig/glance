//! 托管实时字幕子进程。
//!
//! 简译是主进程：持唯一的托盘图标与全局热键，按需把字幕拉起来当子进程。
//! **不做成一个 exe 是硬性的**——字幕的 `vulkan-1.dll` 是加载期导入（无 delay-load），
//! 合进同一个可执行文件后，任何没有 Vulkan ICD 的机器（干净虚拟机、部分 RDP 会话、
//! 老显卡驱动）连划词取词都起不来，Windows 加载器直接 `0xC0000135`，进不到 main。
//!
//! 这里只依赖 `desktop_shell::caption_ipc` 里的协议，**绝不依赖 live-caption crate**：
//! 那会把 whisper-rs 拉进简译的依赖图，于是 `cargo build -p translate-tool` 又得要
//! CMake、Ninja 和 Vulkan SDK。

use desktop_shell::caption_ipc::{cmd_body, Command, State, PATH_CMD, PATH_STATE};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

/// 与字幕控制端点通信的超时。子进程在本机、走回环，慢不到哪去；
/// 设长了会让托盘菜单卡住，因为弹菜单前要同步读一次勾选态。
const IPC_TIMEOUT: Duration = Duration::from_millis(800);

/// 优雅退出的等待上限。**必须优雅退出，不能直接 kill**：
/// 字幕的 claude 后端会起常驻子进程，强杀父进程不回收它们，
/// 用户开关几次字幕就积一堆 claude.exe，每个约 300 MB。
const QUIT_GRACE: Duration = Duration::from_secs(4);

/// 子进程请求重启时的退出码，与 `live_caption::managed::exit_code::RESTART` 对齐。
/// 这里写死数字而不是引用那个常量，同样是为了不依赖 live-caption crate；
/// 两边不一致的后果是「改完识别参数不会重启」，所以两处都注明了对方位置。
const EXIT_RESTART: i32 = 17;

/// 找字幕可执行文件。安装版在 exe 旁边，开发时在字幕自己的 target 目录
/// （它有独立的 `CARGO_TARGET_DIR`，见 `apps/caption/scripts/env.sh`——
/// 共用一个 target 会让字幕一次 5–10 分钟的全量编译把简译的 6 秒增量堵在目录锁上）。
pub fn find_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let base = exe.parent()?;
    let candidates = [
        base.join("live-caption.exe"),
        base.join("caption").join("live-caption.exe"),
        PathBuf::from(r"D:\DevCache\lc-target\release\live-caption.exe"),
        PathBuf::from(r"D:\DevCache\lc-target\debug\live-caption.exe"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// 字幕子进程的持有者。
pub struct CaptionHost {
    exe: Option<PathBuf>,
    child: Option<Child>,
    /// 自己的本地服务端口，传给子进程做心跳目标
    parent_port: u16,
    /// 子进程控制端点端口
    control_port: u16,
    client: reqwest::blocking::Client,
    /// 用户是不是主动关掉的。主动关的不再自动拉起，崩溃的才重启。
    stopped_by_user: bool,
}

impl CaptionHost {
    pub fn new(parent_port: u16) -> CaptionHost {
        let client = reqwest::blocking::Client::builder().timeout(IPC_TIMEOUT).build().unwrap_or_default();
        CaptionHost {
            exe: find_exe(),
            child: None,
            parent_port,
            // 控制端点用本地服务端口 +1，冲突时 start 里再往后挪
            control_port: parent_port.wrapping_add(1),
            client,
            stopped_by_user: false,
        }
    }

    /// 装没装字幕。没装时菜单里那一栏置灰。
    pub fn installed(&self) -> bool {
        self.exe.is_some()
    }

    /// 字幕 exe 在哪（关于页展示用）
    pub fn exe_path(&self) -> Option<std::path::PathBuf> {
        self.exe.clone()
    }

    pub fn running(&mut self) -> bool {
        self.reap();
        self.child.is_some()
    }

    /// 回收已经退出的子进程，并按退出码决定要不要重新拉起。
    /// 每帧调一次，别用 `wait()`（那会阻塞 UI 线程）。
    pub fn reap(&mut self) {
        let Some(child) = self.child.as_mut() else { return };
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(-1);
                self.child = None;
                if code == EXIT_RESTART {
                    tracing::info!("字幕请求重启（退出码 {code}），重新拉起");
                    if let Err(e) = self.start() {
                        tracing::error!("字幕重启失败：{e}");
                    }
                } else if code == 0 || self.stopped_by_user {
                    tracing::info!("字幕已退出（退出码 {code}）");
                } else {
                    // 崩溃不自动重启：起不来的原因多半是缺 Vulkan / 模型，
                    // 反复拉起只会刷屏，还会一直弹窗口。
                    tracing::error!("字幕异常退出（退出码 {code}），不自动重启");
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("查子进程状态失败：{e}");
                self.child = None;
            }
        }
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.child.is_some() {
            return Ok(());
        }
        let exe = self.exe.clone().ok_or_else(|| anyhow::anyhow!("没找到实时字幕程序，安装时可能没有勾选这个组件"))?;
        self.control_port = pick_free_port(self.parent_port.wrapping_add(1))
            .ok_or_else(|| anyhow::anyhow!("没有可用的本地端口给字幕控制端点"))?;
        // **工作目录必须显式指定**：字幕的数据目录判据是「当前目录有没有 config.toml」，
        // 不指定的话它会继承简译的工作目录，在「读仓库里的配置」和「读 %APPDATA%」之间静默横跳。
        let cwd = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let child = std::process::Command::new(&exe)
            .current_dir(&cwd)
            .args([
                "--managed",
                "--parent-port",
                &self.parent_port.to_string(),
                "--control-port",
                &self.control_port.to_string(),
            ])
            .spawn()
            .map_err(|e| anyhow::anyhow!("拉起实时字幕失败：{e}"))?;
        tracing::info!("字幕已拉起：{} 工作目录 {} 控制端口 {}", exe.display(), cwd.display(), self.control_port);
        self.child = Some(child);
        self.stopped_by_user = false;
        Ok(())
    }

    /// 优雅停止：先下 quit 命令让它自己收尾，等不到再杀。
    pub fn stop(&mut self) {
        self.stopped_by_user = true;
        let Some(mut child) = self.child.take() else { return };
        let _ = self.send(Command::Quit);
        let deadline = std::time::Instant::now() + QUIT_GRACE;
        while std::time::Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => {
                    tracing::info!("字幕已优雅退出");
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
        tracing::warn!("字幕没在 {QUIT_GRACE:?} 内退出，强制结束（它的 claude 子进程可能残留）");
        let _ = child.kill();
        let _ = child.wait();
    }

    /// 读勾选态，给托盘菜单画勾用。读不到就返回 None，菜单退化成不带勾。
    pub fn state(&self) -> Option<State> {
        let url = format!("http://127.0.0.1:{}{PATH_STATE}", self.control_port);
        let body = self.client.get(&url).send().ok()?.text().ok()?;
        State::from_json(&body)
    }

    pub fn send(&self, cmd: Command) -> anyhow::Result<()> {
        let url = format!("http://127.0.0.1:{}{PATH_CMD}", self.control_port);
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(cmd_body(cmd))
            .send()
            .map_err(|e| anyhow::anyhow!("字幕没有响应：{e}"))?;
        anyhow::ensure!(resp.status().is_success(), "字幕拒绝了命令 {}", cmd.as_str());
        Ok(())
    }
}

impl Drop for CaptionHost {
    /// 简译退出时别把字幕留成孤儿进程——它没有托盘、没有热键，
    /// 用户只能去任务管理器杀。
    fn drop(&mut self) {
        if self.child.is_some() {
            self.stop();
        }
    }
}

/// 从 `from` 开始找一个能绑上的回环端口
fn pick_free_port(from: u16) -> Option<u16> {
    (from..from.saturating_add(20)).find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_port_that_is_actually_free() {
        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = taken.local_addr().unwrap().port();
        let got = pick_free_port(busy).expect("总该找得到空端口");
        assert_ne!(got, busy, "占用的端口不能选");
        assert!(got > busy && got < busy + 20);
        // 选出来的确实能绑
        assert!(std::net::TcpListener::bind(("127.0.0.1", got)).is_ok());
    }

    #[test]
    fn host_without_exe_reports_not_installed() {
        let mut h = CaptionHost::new(42731);
        h.exe = None;
        assert!(!h.installed());
        assert!(!h.running());
        assert!(h.start().is_err(), "没装就该报错，不能静默什么都不做");
    }

    #[test]
    fn restart_exit_code_matches_the_caption_side() {
        // 对面在 apps/caption/src/managed.rs 的 exit_code::RESTART。
        // 这里写死数字是为了不依赖 live-caption crate（会把 whisper 拉进依赖图），
        // 代价就是这条断言——两边改了任一处，测试立刻红。
        let caption_src = include_str!("../../caption/src/managed.rs");
        let line = caption_src.lines().find(|l| l.contains("pub const RESTART")).expect("对面的 RESTART 常量不见了");
        assert!(line.contains(&EXIT_RESTART.to_string()), "退出码对不上：{line}");
    }
}
