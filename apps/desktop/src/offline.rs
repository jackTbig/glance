//! 离线词典的接入：词库放在哪、离线条目怎么变成卡片上的那几行。
//!
//! 背景：模型偶尔要等好几秒，看着像卡死（Jack 2026-09-20 报）。取到单个词时先用本地词库
//! 秒出一份释义，模型的完整词条回来再替换——**离线是先手不是备胎**，所以它出结果的那一刻
//! 卡片上就该有东西可看。
//!
//! 词库文件由 `offline-dict` 的 `build-dict` 从 ECDICT 的 CSV 生成（MIT 授权，
//! 许可声明随安装包分发，见 `docs/第三方组件授权.md`）。

use std::path::PathBuf;

/// 词库文件的位置，按优先级找：
/// 1. `TT_DICT` 环境变量（排障与测试用）；
/// 2. exe 旁的 `dict\ecdict.bin`（安装版就是这份）；
/// 3. `D:\DevCache\tt-dict\ecdict.bin`（开发机上 `build-dict` 的默认产物位置）。
///
/// **找不到不是错误**：没装词库时离线查词只是不工作，翻译照常走模型。
pub fn dict_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TT_DICT").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("dict").join("ecdict.bin");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let dev = PathBuf::from(r"D:\DevCache\tt-dict\ecdict.bin");
    dev.is_file().then_some(dev)
}

/// 把离线条目转成卡片认得的行标记格式（与模型输出同一套，见 `ai_translate::prompts`）。
///
/// 这样离线与在线两条路共用同一个解析器与同一张版式，**不会出现「离线卡片长得不一样」**。
/// `restored` 表示查的是变形、显示的是原型，要在词头上标出来，否则用户以为查错了。
pub fn to_markers(e: &offline_dict::Entry, restored: bool, queried: &str) -> String {
    let mut out = String::new();
    let head = if restored && !queried.trim().is_empty() {
        format!("{} （{} 的原形）", e.word, queried.trim())
    } else {
        e.word.clone()
    };
    out.push_str(&format!("#词 {head}\n"));
    if !e.phonetic.trim().is_empty() {
        out.push_str(&format!("#音 {}\n", e.phonetic.trim()));
    }
    for line in e.translation.lines() {
        let line = line.trim();
        // [网络]/[机] 这类机器翻译噪声行不进卡片：它们常常是乱的，混在正经释义里更碍眼
        if line.is_empty() || line.starts_with("[网络]") || line.starts_with("[机]") {
            continue;
        }
        out.push_str(&format!("#释 {line}\n"));
    }
    let forms = e.forms_zh();
    if !forms.is_empty() {
        out.push_str(&format!("#形 {forms}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> offline_dict::Entry {
        offline_dict::Entry {
            word: "glance".into(),
            phonetic: "'glɑ:ns".into(),
            translation: "n. 一瞥, 闪光\nvi. 扫视\n[网络] 格兰斯".into(),
            exchange: "p:glanced/d:glanced/i:glancing/3:glances/s:glances".into(),
        }
    }

    /// 离线条目要能被现有的卡片解析器吃下去：标记格式一变，卡片就退化成一坨原文
    #[test]
    fn markers_parse_into_the_same_card() {
        let m = to_markers(&entry(), false, "glance");
        let card = crate::dict::parse(&m).expect("离线条目应当解析成词典卡");
        assert_eq!(card.head, "glance");
        assert_eq!(card.phonetic, "'glɑ:ns");
        assert_eq!(card.senses.len(), 2, "两条正经释义，网络义项要被滤掉：{card:?}");
        assert!(card.forms.contains("过去式 glanced"));
    }

    #[test]
    fn machine_translation_noise_is_dropped() {
        let m = to_markers(&entry(), false, "glance");
        assert!(!m.contains("[网络]"), "{m}");
    }

    /// 查的是变形、显示的是原型时要在词头讲清楚，否则用户以为查错了词
    #[test]
    fn restored_lemma_says_so_in_the_headword() {
        let m = to_markers(&entry(), true, "glancing");
        assert!(m.contains("glance （glancing 的原形）"), "{m}");
        let card = crate::dict::parse(&m).unwrap();
        assert!(card.head.contains("原形"));
    }

    /// 没音标、没词形的条目也要能出卡片，不能留下空的 #音 / #形 行
    #[test]
    fn sparse_entry_omits_empty_lines() {
        let e = offline_dict::Entry {
            word: "Beijing".into(),
            translation: "n. 北京".into(),
            ..Default::default()
        };
        let m = to_markers(&e, false, "beijing");
        assert!(!m.contains("#音") && !m.contains("#形"), "{m}");
        assert_eq!(crate::dict::parse(&m).unwrap().senses.len(), 1);
    }

    /// 环境变量指到不存在的文件时要当成「没有词库」，而不是拿着坏路径去开
    #[test]
    fn missing_dict_is_not_an_error() {
        let old = std::env::var_os("TT_DICT");
        std::env::set_var("TT_DICT", r"Z:\nowhere\ecdict.bin");
        let p = dict_path();
        assert!(p.as_deref() != Some(std::path::Path::new(r"Z:\nowhere\ecdict.bin")));
        match old {
            Some(v) => std::env::set_var("TT_DICT", v),
            None => std::env::remove_var("TT_DICT"),
        }
    }
}
