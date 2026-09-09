//! 字体安装。两个应用（简译取词卡、实时字幕条）都要中文，且都踩过缺字形的坑。

use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// 中文字体候选，按优先级
const CJK: &[&str] = &[r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\msyh.ttf", r"C:\Windows\Fonts\simhei.ttf"];

/// 挂在中文字体后面的兜底。
///
/// 雅黑与 egui 自带字体都没有音标的长音符 `ː`(U+02D0) 与界面上的几个符号，
/// **缺字形不报错、只画成方块豆腐**，排查时很容易误判成字符串写错。
/// Segoe UI 覆盖 IPA 扩展与修饰字母，Segoe UI Symbol 覆盖杂项符号。
const FALLBACK: &[(&str, &str)] = &[("ipa", r"C:\Windows\Fonts\segoeui.ttf"), ("sym", r"C:\Windows\Fonts\seguisym.ttf")];

/// 装中文字体与兜底字体。找不到中文字体只打日志，界面照常起（中文会是方块）。
pub fn install(ctx: &egui::Context) {
    for p in CJK {
        let Ok(bytes) = std::fs::read(p) else { continue };
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert("cjk".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "cjk".into());
        fonts.families.get_mut(&FontFamily::Monospace).unwrap().push("cjk".into());
        // 顺序即回退顺序：中文字体在前，兜底依次排在它后面
        let mut at = 1;
        for (name, path) in FALLBACK {
            if let Ok(b) = std::fs::read(path) {
                fonts.font_data.insert((*name).into(), Arc::new(FontData::from_owned(b)));
                fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(at, (*name).into());
                at += 1;
            }
        }
        ctx.set_fonts(fonts);
        return;
    }
    tracing::warn!("没找到中文字体，界面里的中文会显示成方块");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_lists_are_sane() {
        assert!(CJK.iter().all(|p| p.ends_with(".ttc") || p.ends_with(".ttf")));
        // 名字进 font_data 的 key，重了会互相覆盖
        let mut names: Vec<&str> = FALLBACK.iter().map(|(n, _)| *n).collect();
        names.push("cjk");
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "字体名不能重复");
    }
}
