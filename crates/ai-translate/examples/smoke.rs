//! 真实后端冒烟：`cargo run -p ai-translate --example smoke -- [openai|claude|codex]`
//!
//! Key 与 Base URL 从环境变量取，没有就退回读 live-caption 的 config.toml：
//! `OPENAI_API_KEY` / `OPENAI_BASE_URL` / `OPENAI_MODEL`。
use ai_translate::translator::{Item, Translator, TranslatorConfig};
use ai_translate::{Backend, Scene, SceneRoute};
use std::time::Instant;

fn from_live_caption_config() -> Option<(String, String, String)> {
    let p = r"D:\My_data\Claude\live-caption\config.toml";
    let s = std::fs::read_to_string(p).ok()?;
    let get = |k: &str| -> Option<String> {
        s.lines()
            .find(|l| l.trim_start().starts_with(k))?
            .split_once('=')
            .map(|(_, v)| v.trim().trim_matches('"').trim_matches('\'').to_string())
    };
    Some((get("openai_base_url")?, get("openai_api_key")?, get("openai_model")?))
}

fn main() -> anyhow::Result<()> {
    let which = std::env::args().nth(1).unwrap_or_else(|| "openai".into());
    let backend = Backend::parse(&which);
    let mut cfg = TranslatorConfig::default();
    if backend == Backend::OpenAi {
        let (url, key, model) = match (std::env::var("OPENAI_API_KEY"), from_live_caption_config()) {
            (Ok(k), _) => (
                std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.deepseek.com".into()),
                k,
                std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "deepseek-flash".into()),
            ),
            (_, Some(t)) => t,
            _ => anyhow::bail!("没有 OPENAI_API_KEY，也读不到 live-caption 的 config.toml"),
        };
        println!("后端 openai  base_url={url}  model={model}  key=***{}", &key[key.len().saturating_sub(4)..]);
        cfg.base.openai_base_url = url;
        cfg.base.openai_api_key = key;
        cfg.base.openai_model = model;
    } else {
        println!("后端 {}", backend.as_str());
        cfg.base.timeout = std::time::Duration::from_secs(60);
    }
    for s in Scene::ALL {
        cfg.routes.insert(s, SceneRoute { backend, model: String::new() });
    }
    let t = Translator::new(cfg)?;

    let t0 = Instant::now();
    let one = t.translate(Scene::Selection, "The quick brown fox jumps over the lazy dog.", None, &mut |_| {})?;
    println!("[划词] {:?}ms  {one}", t0.elapsed().as_millis());

    let t0 = Instant::now();
    let w = t.translate(Scene::Word, "glance", None, &mut |_| {})?;
    println!("[取词] {:?}ms\n{w}", t0.elapsed().as_millis());
    anyhow::ensure!(w.contains("#词") && w.contains("#释"), "取词没按行标记格式返回");

    let t0 = Instant::now();
    let items = vec![
        Item { id: "1".into(), text: "Rust is a systems programming language.".into(), is_html: false },
        Item { id: "2".into(), text: "Click <a href=\"https://example.com\">here</a> to learn <b>more</b>.".into(), is_html: true },
        Item { id: "3".into(), text: "这一段已经是中文，应当原样返回。".into(), is_html: false },
    ];
    let rs = t.translate_batch(Scene::FullPage, items);
    println!("[整页] {:?}ms", t0.elapsed().as_millis());
    for r in &rs {
        match &r.result {
            Ok(s) => println!("  {} → {s}", r.id),
            Err(e) => println!("  {} → 失败：{e}", r.id),
        }
    }
    anyhow::ensure!(rs.iter().all(|r| r.result.is_ok()), "有段落翻译失败");
    anyhow::ensure!(rs[1].result.as_ref().unwrap().contains("<a href="), "第 2 段的链接标签没还原");
    println!("冒烟通过");
    Ok(())
}
