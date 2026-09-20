//! 「关于」页要展示的那几条事实。
//!
//! 放在这里而不是直接写进界面：这些条目是**排障时最先要问的信息**（版本、构建时间、
//! 配置与日志在哪、词库装没装、插件连没连），要能被测试盯住格式，也要能一键复制给别人。

use std::path::PathBuf;

/// 一行「名称 → 值」。`path` 有值时界面给一个「打开」按钮
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    pub name: &'static str,
    pub value: String,
    /// 可以在资源管理器里打开的目录或文件
    pub path: Option<PathBuf>,
}

impl Fact {
    fn new(name: &'static str, value: impl Into<String>) -> Fact {
        Fact { name, value: value.into(), path: None }
    }

    fn at(name: &'static str, value: impl Into<String>, path: PathBuf) -> Fact {
        Fact { name, value: value.into(), path: Some(path) }
    }
}

/// 版本行：`1.0.6（2026-09-20 构建 · a1b2c3d）`。
///
/// 构建时间与 commit 由 `build.rs` 注入；没注入（比如别人直接 `cargo build` 没跑到那段）
/// 就只显示版本号，**不显示「未知」**——一行括号里写着「未知」比不写更让人心里没底。
pub fn version_line() -> String {
    let ver = env!("CARGO_PKG_VERSION");
    match (option_env!("TT_BUILD_DATE"), option_env!("TT_BUILD_COMMIT")) {
        (Some(d), Some(c)) if !d.is_empty() && !c.is_empty() => format!("{ver}（{d} 构建 · {c}）"),
        (Some(d), _) if !d.is_empty() => format!("{ver}（{d} 构建）"),
        _ => ver.to_string(),
    }
}

/// 关于页的全部条目。参数都是运行时才知道的东西，由界面传进来。
pub fn facts(
    data_dir: PathBuf,
    dict: Option<(PathBuf, usize)>,
    caption_exe: Option<PathBuf>,
    extension_dir: Option<PathBuf>,
    server_port: Option<u16>,
    plugin_seen: bool,
) -> Vec<Fact> {
    let mut v = vec![
        Fact::new("版本", version_line()),
        Fact::at("配置文件", data_dir.join("config.toml").display().to_string(), data_dir.join("config.toml")),
        Fact::at("日志目录", data_dir.join("logs").display().to_string(), data_dir.join("logs")),
    ];
    v.push(match dict {
        Some((p, n)) => Fact::at("离线词典", format!("{} 条（{}）", n, p.display()), p),
        None => Fact::new("离线词典", "未安装——模型慢的时候没有本地释义兜底"),
    });
    v.push(match caption_exe {
        Some(p) => Fact::at("实时字幕", p.display().to_string(), p),
        None => Fact::new("实时字幕", "未安装（安装包里可勾选）"),
    });
    v.push(match extension_dir {
        Some(p) => Fact::at("浏览器插件", p.display().to_string(), p),
        None => Fact::new("浏览器插件", "没找到插件目录"),
    });
    v.push(Fact::new(
        "本地服务",
        match (server_port, plugin_seen) {
            (Some(p), true) => format!("127.0.0.1:{p} · 插件已连上"),
            (Some(p), false) => format!("127.0.0.1:{p} · 还没有插件连过"),
            (None, _) => "没有启动（设置里可开）".into(),
        },
    ));
    v
}

/// 「复制」按钮拷走的纯文本：一行一条，可以直接贴进聊天窗口发给别人排障
pub fn to_plain(facts: &[Fact]) -> String {
    let mut s = format!("简译 Glance {}\n", version_line());
    for f in facts {
        s.push_str(&format!("{}：{}\n", f.name, f.value));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Fact> {
        facts(
            PathBuf::from(r"C:\Users\x\AppData\Roaming\translate-tool"),
            Some((PathBuf::from(r"D:\app\dict\ecdict.bin"), 768757)),
            Some(PathBuf::from(r"D:\app\live-caption.exe")),
            Some(PathBuf::from(r"D:\app\extension")),
            Some(42731),
            true,
        )
    }

    #[test]
    fn version_line_never_says_unknown() {
        let v = version_line();
        assert!(v.starts_with(env!("CARGO_PKG_VERSION")), "{v}");
        assert!(!v.contains("未知"), "没有构建信息时就只写版本号：{v}");
    }

    /// 排障要问的东西必须都在：配置、日志、词典、字幕、插件、服务
    #[test]
    fn facts_cover_what_we_always_ask_when_troubleshooting() {
        let f = sample();
        for name in ["版本", "配置文件", "日志目录", "离线词典", "实时字幕", "浏览器插件", "本地服务"] {
            assert!(f.iter().any(|x| x.name == name), "少了「{name}」");
        }
    }

    /// 缺组件时要说清「没有会怎样」，不能只写「未安装」让用户自己猜
    #[test]
    fn missing_pieces_explain_themselves() {
        let f = facts(PathBuf::from("."), None, None, None, None, false);
        let dict = f.iter().find(|x| x.name == "离线词典").unwrap();
        assert!(dict.value.contains("兜底"), "{}", dict.value);
        assert!(dict.path.is_none(), "没装就不该给「打开」按钮");
        let server = f.iter().find(|x| x.name == "本地服务").unwrap();
        assert!(server.value.contains("没有启动"));
    }

    #[test]
    fn plugin_connection_state_is_visible() {
        assert!(sample().iter().any(|f| f.value.contains("插件已连上")));
        let never = facts(PathBuf::from("."), None, None, None, Some(1), false);
        assert!(never.iter().any(|f| f.value.contains("还没有插件连过")));
    }

    /// 复制出去的文本要能直接发给别人看
    #[test]
    fn plain_text_is_one_line_per_fact() {
        let t = to_plain(&sample());
        assert!(t.starts_with("简译 Glance "));
        assert_eq!(t.lines().count(), sample().len() + 1);
        assert!(t.contains("离线词典：768757 条"));
    }
}
