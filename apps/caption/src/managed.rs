//! managed 模式：字幕作为简译拉起的子进程运行。
//!
//! 单独运行时字幕什么都自己管（托盘、热键、退出）。被简译托管时这些交给父进程，
//! 好处是用户只看到一个托盘图标、一套热键；代价是**多了一条依赖链，断了要能自救**。
//!
//! 自救这件事不是可选项。`Ctrl+Alt+P` 打开的鼠标穿透会让字幕窗对鼠标完全无感，
//! 回到可点击状态的路只有热键和托盘两条。托管模式下这两条都在父进程手上——
//! 父进程一旦崩了、或者它的热键注册失败而自己没发现，用户屏幕上就会留下一个
//! **永远置顶、永远穿透、无法交互也无法关闭**的窗口，只能去任务管理器杀。
//!
//! 所以子进程对父进程做心跳：连不上就立刻关穿透、把自己的托盘图标显示出来。
//! 这一条同时覆盖了「父进程崩溃」与「父进程热键没注册上」两种情况。

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 心跳间隔。父进程活着时这是纯粹的空转，别设太密。
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(2);
/// 连续失败几次算父进程没了。3 次 × 2 秒 = 6 秒，够扛住父进程一次卡顿或重启。
pub const HEARTBEAT_TOLERANCE: u8 = 3;

/// 子进程退出码，父进程用它区分「正常退出」「请求重启」「崩了」。
/// 不区分的话，字幕改完识别参数主动重启会被父进程当成崩溃，进重试风暴。
pub mod exit_code {
    /// 用户主动退出，父进程不要再拉起
    pub const NORMAL: i32 = 0;
    /// 改了识别/切句参数，请求父进程重新拉起
    pub const RESTART: i32 = 17;
}

/// 命令行参数。托管时由简译传入。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Args {
    /// 是否被托管
    pub managed: bool,
    /// 父进程（简译）的本地服务端口，用来做心跳
    pub parent_port: u16,
    /// 自己的控制端点端口，父进程用它查状态、下命令
    pub control_port: u16,
}

impl Args {
    /// 解析 `--managed --parent-port 42731 --control-port 42732`。
    /// 少了任何一个端口就当没托管——宁可退回「自己管自己」，也不要半托管状态。
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Args {
        let v: Vec<String> = args.into_iter().collect();
        let get = |name: &str| -> Option<u16> {
            let i = v.iter().position(|a| a == name)?;
            v.get(i + 1)?.parse().ok()
        };
        let managed = v.iter().any(|a| a == "--managed");
        let parent_port = get("--parent-port").unwrap_or(0);
        let control_port = get("--control-port").unwrap_or(0);
        if managed && parent_port != 0 && control_port != 0 {
            Args { managed: true, parent_port, control_port }
        } else {
            if managed {
                tracing::warn!("--managed 缺少 --parent-port / --control-port，退回独立运行");
            }
            Args::default()
        }
    }
}

/// 进程退出码。`eframe::run_native` 返回后由 main 读出来交给 `std::process::exit`。
///
/// 用全局而不是把值一路传回 main：想设退出码的地方在 UI 回调深处，
/// 一路加返回值要改十几处签名，收益为零。
static EXIT_CODE: AtomicI32 = AtomicI32::new(exit_code::NORMAL);

pub fn set_exit_code(code: i32) {
    EXIT_CODE.store(code, Ordering::Relaxed);
}

pub fn exit_code_now() -> i32 {
    EXIT_CODE.load(Ordering::Relaxed)
}

/// 托管运行时的共享状态。UI 线程读它决定要不要自救，心跳线程写它。
#[derive(Debug)]
pub struct Link {
    pub args: Args,
    /// 与父进程失联了：UI 看到这个就关穿透、把托盘显示出来
    lost: AtomicBool,
    /// 连续失败次数
    misses: AtomicU8,
}

impl Link {
    pub fn new(args: Args) -> Arc<Link> {
        Arc::new(Link { args, lost: AtomicBool::new(false), misses: AtomicU8::new(0) })
    }

    pub fn is_managed(&self) -> bool {
        self.args.managed
    }

    /// 失联了吗。没托管永远是 false。
    pub fn is_lost(&self) -> bool {
        self.args.managed && self.lost.load(Ordering::Relaxed)
    }

    /// 记一次心跳结果。返回 true 表示**这一次刚刚**判定失联（只在跳变的那一次返回 true，
    /// 免得 UI 每帧都重复执行自救动作）。
    pub fn record(&self, ok: bool) -> bool {
        if ok {
            self.misses.store(0, Ordering::Relaxed);
            return false;
        }
        let n = self.misses.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= HEARTBEAT_TOLERANCE && !self.lost.swap(true, Ordering::Relaxed) {
            tracing::error!("与简译失联（连续 {n} 次心跳失败），恢复自己的托盘并关闭鼠标穿透");
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Args {
        Args::parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_full_managed_args() {
        assert_eq!(
            args(&["live-caption.exe", "--managed", "--parent-port", "42731", "--control-port", "42732"]),
            Args { managed: true, parent_port: 42731, control_port: 42732 }
        );
    }

    #[test]
    fn half_managed_falls_back_to_standalone() {
        // 半托管是最危险的状态：托盘和热键都没了，父进程却管不到它
        for v in [
            vec!["x", "--managed"],
            vec!["x", "--managed", "--parent-port", "42731"],
            vec!["x", "--managed", "--control-port", "42732"],
            vec!["x", "--managed", "--parent-port", "0", "--control-port", "42732"],
            vec!["x", "--managed", "--parent-port", "abc", "--control-port", "42732"],
        ] {
            assert!(!args(&v).managed, "{v:?} 不该判成托管");
        }
        assert_eq!(args(&["x"]), Args::default());
    }

    #[test]
    fn loss_is_reported_once_and_needs_consecutive_misses() {
        let link = Link::new(Args { managed: true, parent_port: 1, control_port: 2 });
        for _ in 0..HEARTBEAT_TOLERANCE - 1 {
            assert!(!link.record(false), "还没到容忍次数不能判失联");
            assert!(!link.is_lost());
        }
        assert!(link.record(false), "达到次数的那一下要返回 true");
        assert!(link.is_lost());
        assert!(!link.record(false), "已经失联过就不再返回 true，避免每帧重复自救");
    }

    #[test]
    fn a_success_resets_the_streak() {
        let link = Link::new(Args { managed: true, parent_port: 1, control_port: 2 });
        link.record(false);
        link.record(false);
        link.record(true); // 父进程只是卡了一下
        for _ in 0..HEARTBEAT_TOLERANCE - 1 {
            assert!(!link.record(false));
        }
        assert!(link.record(false));
    }

    #[test]
    fn exit_code_defaults_to_normal_and_can_be_set() {
        assert_eq!(exit_code_now(), exit_code::NORMAL);
        set_exit_code(exit_code::RESTART);
        assert_eq!(exit_code_now(), exit_code::RESTART);
        // 两个码必须不同，父进程靠它区分「用户退出」和「请求重启」
        assert_ne!(exit_code::NORMAL, exit_code::RESTART);
        set_exit_code(exit_code::NORMAL);
    }

    #[test]
    fn standalone_link_never_reports_loss() {
        let link = Link::new(Args::default());
        for _ in 0..10 {
            link.record(false);
        }
        assert!(!link.is_lost(), "没托管就没有父进程，不该有失联概念");
    }
}
