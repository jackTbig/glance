//! 场景：整页 / 划词 / 悬停 / 截屏

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scene {
    FullPage,
    Selection,
    Hover,
    Screenshot,
}

impl Scene {
    pub const ALL: [Scene; 4] = [Scene::FullPage, Scene::Selection, Scene::Hover, Scene::Screenshot];

    pub fn parse(s: &str) -> Option<Scene> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "full_page" | "fullpage" | "page" => Some(Scene::FullPage),
            "selection" | "select" => Some(Scene::Selection),
            "hover" => Some(Scene::Hover),
            "screenshot" | "ocr" => Some(Scene::Screenshot),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scene::FullPage => "full_page",
            Scene::Selection => "selection",
            Scene::Hover => "hover",
            Scene::Screenshot => "screenshot",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Scene::FullPage => "整页翻译",
            Scene::Selection => "划词翻译",
            Scene::Hover => "悬停翻译",
            Scene::Screenshot => "截屏翻译",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_parse_round_trip() {
        for s in Scene::ALL {
            assert_eq!(Scene::parse(s.as_str()), Some(s));
        }
        assert_eq!(Scene::parse("Full-Page"), Some(Scene::FullPage));
        assert_eq!(Scene::parse("nope"), None);
        assert_eq!(serde_json::to_string(&Scene::FullPage).unwrap(), "\"full_page\"");
    }
}
