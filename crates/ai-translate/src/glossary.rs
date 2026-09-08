//! 术语表：命中的条目以「术语对照」形式附进 prompt，不做硬替换

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GlossaryEntry {
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Clone, Default)]
pub struct Glossary {
    entries: Vec<GlossaryEntry>,
}

impl Glossary {
    pub fn new(entries: Vec<GlossaryEntry>) -> Self {
        Self { entries: entries.into_iter().filter(|e| !e.src.trim().is_empty()).collect() }
    }

    pub fn entries(&self) -> &[GlossaryEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 大小写不敏感子串匹配
    pub fn hits(&self, text: &str) -> Vec<&GlossaryEntry> {
        let lower = text.to_lowercase();
        self.entries.iter().filter(|e| lower.contains(&e.src.to_lowercase())).collect()
    }

    /// 命中为空返回 None
    pub fn prompt_block(&self, text: &str) -> Option<String> {
        let hits = self.hits(text);
        if hits.is_empty() {
            return None;
        }
        let mut s = String::from("术语对照（必须遵守）：\n");
        for e in hits {
            s.push_str(&format!("{} → {}\n", e.src, e.dst));
        }
        Some(s.trim_end().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Glossary {
        Glossary::new(vec![
            GlossaryEntry { src: "Kubernetes".into(), dst: "K8s 集群".into() },
            GlossaryEntry { src: "pod".into(), dst: "Pod".into() },
            GlossaryEntry { src: "  ".into(), dst: "空的被过滤".into() },
        ])
    }

    #[test]
    fn hits_case_insensitive() {
        let gl = g();
        let h = gl.hits("Deploy a POD on kubernetes");
        assert_eq!(h.len(), 2);
        assert_eq!(gl.entries().len(), 2);
    }

    #[test]
    fn prompt_block_none_when_no_hit() {
        assert!(g().prompt_block("nothing here").is_none());
    }

    #[test]
    fn prompt_block_lists_hits() {
        assert_eq!(g().prompt_block("kubernetes").unwrap(), "术语对照（必须遵守）：\nKubernetes → K8s 集群");
    }
}
