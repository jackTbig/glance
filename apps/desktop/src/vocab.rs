//! 生词库：查过的单词存下来，随时随机抽一组出来考。
//!
//! - **存哪**：`%APPDATA%\translate-tool\words.json`（与 config.toml 同目录）。
//!   `path = None` 时只在内存里（headless 服务进程用，免得测试污染真词库）。
//! - **谁入库**：桌面端划词/悬停/主窗口查词、Chrome 插件划词查词——
//!   只要判成「查单词」且模型回了完整词条就入一份（见 `record_markers`）。
//!   插件的请求走本地服务，服务在同一个桌面进程里，所以两边共用同一个库。
//! - **怎么出题**：`build_quiz` 从库里随机抽 n 个词，混着出四种题
//!   （单选 / 配对 / 拼写 / 听音）。本文件只放纯逻辑，画界面在 `vocab_ui.rs`。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// 词库句柄：界面线程与本地服务线程共用一份
pub type SharedVocab = Arc<Mutex<Vocab>>;

/// 一个词性下的释义。
///
/// 自己定义一份而不复用 `dict::Sense`：那边是给卡片用的、不落盘，
/// 而这里要序列化进 json。字段含义一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sense {
    /// 词性缩写，如 `v.`；模型没给就是空串
    pub pos: String,
    pub text: String,
}

/// 生词库里的一条
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// 用户查的那个词（原样；去重时按小写比较）
    pub word: String,
    pub phonetic: String,
    pub senses: Vec<Sense>,
    pub example: String,
    pub example_zh: String,
    /// 第一次收录的时间
    pub first_seen: String,
    /// 最近一次收录的时间
    pub last_seen: String,
    /// 从哪来的：桌面端 / 插件
    pub source: String,
}

impl Word {
    /// 第一条中文释义。选择题、配对题都用它。
    ///
    /// 只取第一个义项：`一瞥，看一眼；浏览；掠过` → `一瞥，看一眼`。
    /// 一个义项塞四个选项已经够长，再多就不是「选项」了。
    pub fn meaning(&self) -> Option<&str> {
        let t = self.senses.iter().map(|s| s.text.trim()).find(|t| !t.is_empty())?;
        let first = t.split('；').next().unwrap_or(t).trim();
        (!first.is_empty()).then_some(first)
    }
}

/// 生词库的设置（存在词库文件里，不进 config.toml —— 这样设置窗的草稿机制碰不到它）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VocabSettings {
    /// 查到的单词要不要入库
    pub record: bool,
    /// 每天定时推送测验
    pub push_enabled: bool,
    /// 推送时刻（本地时间）
    pub push_hour: u8,
    pub push_minute: u8,
    /// 一次推几个词
    pub quiz_count: usize,
    /// 题型；空 = 全都要
    pub kinds: Vec<String>,
}

impl Default for VocabSettings {
    fn default() -> Self {
        Self {
            record: true,
            push_enabled: true,
            push_hour: 19,
            push_minute: 0,
            quiz_count: 10,
            kinds: Vec::new(),
        }
    }
}

impl VocabSettings {
    /// 要出哪几种题型
    pub fn kind_list(&self) -> Vec<Kind> {
        let picked: Vec<Kind> = self.kinds.iter().filter_map(|s| Kind::parse(s)).collect();
        if picked.is_empty() {
            Kind::all().to_vec()
        } else {
            picked
        }
    }
}

/// 词库文件的内容（路径不进文件）
#[derive(Debug, Default, Serialize, Deserialize)]
struct VocabFile {
    #[serde(default)]
    words: Vec<Word>,
    #[serde(default)]
    settings: VocabSettings,
    /// 上次推送测验的日期（`YYYY-MM-DD`），保证一天只推一次
    #[serde(default)]
    last_push: String,
}

/// 生词库
#[derive(Debug, Default)]
pub struct Vocab {
    words: Vec<Word>,
    settings: VocabSettings,
    last_push: String,
    /// 落盘路径；None = 只在内存里
    path: Option<PathBuf>,
}

impl Vocab {
    /// 打开词库。文件不存在或坏了都当空的（词库坏了不该让程序起不来）
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut v = Vocab { words: Vec::new(), settings: VocabSettings::default(), last_push: String::new(), path: path.clone() };
        let Some(p) = path else { return v };
        let Ok(s) = std::fs::read_to_string(&p) else { return v };
        match serde_json::from_str::<VocabFile>(&s) {
            Ok(f) => {
                v.words = f.words;
                v.settings = f.settings;
                v.last_push = f.last_push;
                tracing::info!("生词库已加载：{} 条（{}）", v.words.len(), p.display());
            }
            Err(e) => tracing::warn!("生词库读不出来（{e}），当空的用：{}", p.display()),
        }
        v
    }

    /// 词库文件的位置。`None` = 只在内存里
    pub fn default_path() -> PathBuf {
        crate::config::Config::dir().join("words.json")
    }

    /// 写回文件。`path` 为 None 时什么都不做
    ///
    /// **先写临时文件再改名**：写到一半崩了/断电了，原来的词库还在，
    /// 而不是留下一个半截 json（那样读回来当空库，用户攒的词全没了）。
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        let f = VocabFile { words: self.words.clone(), settings: self.settings.clone(), last_push: self.last_push.clone() };
        let s = serde_json::to_string_pretty(&f)?;
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, p)?;
        Ok(())
    }

    pub fn words(&self) -> &[Word] {
        &self.words
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn settings(&self) -> &VocabSettings {
        &self.settings
    }

    pub fn set_settings(&mut self, s: VocabSettings) {
        self.settings = s;
    }

    pub fn last_push(&self) -> &str {
        &self.last_push
    }

    pub fn set_last_push(&mut self, day: &str) {
        self.last_push = day.to_string();
    }

    /// 收录一条。同一个词再次收录只补齐字段，不新增条目
    pub fn upsert(&mut self, w: Word) {
        let key = w.word.trim().to_lowercase();
        if key.is_empty() {
            return;
        }
        if let Some(existing) = self.words.iter_mut().find(|x| x.word.trim().to_lowercase() == key) {
            if !w.phonetic.trim().is_empty() {
                existing.phonetic = w.phonetic;
            }
            if !w.senses.is_empty() {
                existing.senses = w.senses;
            }
            if !w.example.trim().is_empty() {
                existing.example = w.example;
            }
            if !w.example_zh.trim().is_empty() {
                existing.example_zh = w.example_zh;
            }
            existing.last_seen = w.last_seen;
            return;
        }
        self.words.insert(0, w);
    }

    /// 删一条（界面上的「×」）
    pub fn remove(&mut self, word: &str) {
        let key = word.trim().to_lowercase();
        self.words.retain(|x| x.word.trim().to_lowercase() != key);
    }

    /// 能出题的词：有中文释义
    pub fn eligible(&self) -> Vec<&Word> {
        self.words.iter().filter(|w| w.meaning().is_some()).collect()
    }
}

/// 把模型返回的行标记文本收进词库。
///
/// `query` 是用户查的那个词（行标记里的词头有时是原形，不一定等于用户查的）。
/// 只收**拉丁字母**的查询：中译英模式下查的是中文词，那不是要背的生词。
pub fn record_markers(store: &SharedVocab, query: &str, markers: &str, source: &str) {
    let query = query.trim();
    if query.is_empty() || !query.chars().all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '-' || c == '\'') {
        return;
    }
    let Some(entry) = crate::dict::parse(markers) else { return };
    if entry.senses.is_empty() {
        return;
    }
    let now = now_str();
    let word = Word {
        word: query.to_string(),
        phonetic: entry.phonetic,
        senses: entry.senses.into_iter().map(|s| Sense { pos: s.pos, text: s.text }).collect(),
        example: entry.example,
        example_zh: entry.example_zh,
        first_seen: now.clone(),
        last_seen: now,
        source: source.to_string(),
    };
    let mut g = store.lock().unwrap();
    if !g.settings.record {
        return;
    }
    let before = g.len();
    g.upsert(word);
    if g.len() != before {
        tracing::info!("生词库收录：{query}（共 {} 条）", g.len());
    }
    if let Err(e) = g.save() {
        tracing::warn!("生词库写盘失败：{e}");
    }
}

/// 当前时间（本地），`YYYY-MM-DD HH:MM`
pub fn now_str() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// 今天（本地），`YYYY-MM-DD`
pub fn today_str() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 现在这个时刻该不该推送测验（纯函数，便于测）。
///
/// 三个条件同时成立才推：开了开关 · 到了设定的分钟 · 今天还没推过。
/// 「今天推过」看的是 `last_push` 记的日期，跨零点自然就又能推了。
pub fn push_due(settings: &VocabSettings, last_push: &str, hour: u8, minute: u8, today: &str) -> bool {
    settings.push_enabled
        && last_push != today
        && settings.push_hour == hour
        && settings.push_minute == minute
}

/// 题型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// 单选：给英文选中文，或给中文选英文
    Choice,
    /// 配对：左边英文词，右边中文释义，拖到对应位置
    Pair,
    /// 拼写：给中文释义与例句，补全英文单词
    Spell,
    /// 听音：读英文单词，选中文释义
    Listen,
}

impl Kind {
    pub fn all() -> [Kind; 4] {
        [Kind::Choice, Kind::Pair, Kind::Spell, Kind::Listen]
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "choice" => Some(Kind::Choice),
            "pair" => Some(Kind::Pair),
            "spell" => Some(Kind::Spell),
            "listen" => Some(Kind::Listen),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Choice => "choice",
            Kind::Pair => "pair",
            Kind::Spell => "spell",
            Kind::Listen => "listen",
        }
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Kind::Choice => "单选",
            Kind::Pair => "拖曳配对",
            Kind::Spell => "拼写",
            Kind::Listen => "听音选义",
        }
    }
}

/// 一道题
#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    /// 单选。`to_english` = true 时题干是中文、选项是英文词；否则题干是英文、选项是中文释义
    Choice {
        word: String,
        prompt: String,
        options: Vec<String>,
        answer: usize,
        to_english: bool,
    },
    /// 配对。`left` 是英文词，`right` 是打乱后的中文释义，
    /// `correct[j]` = 右边第 j 个释义该配左边第几个词
    Pair {
        left: Vec<String>,
        right: Vec<String>,
        correct: Vec<usize>,
    },
    /// 拼写。`sentence` 是挖空后的例句（没有例句就是空串）
    Spell {
        word: String,
        meaning: String,
        sentence: String,
    },
    /// 听音选义。选项是中文释义
    Listen {
        word: String,
        options: Vec<String>,
        answer: usize,
    }
}

/// 用户的作答
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Answer {
    #[default]
    None,
    /// 选了第几个选项
    Choice(usize),
    /// 拼写题输入的词
    Text(String),
    /// 配对题：右边第 j 个槽放的是左边第几个词
    Pair(Vec<Option<usize>>),
}

/// 判对错
pub fn check(q: &Question, a: &Answer) -> bool {
    match (q, a) {
        (Question::Choice { options, answer, .. }, Answer::Choice(i))
        | (Question::Listen { options, answer, .. }, Answer::Choice(i)) => *i < options.len() && *i == *answer,
        (Question::Spell { word, .. }, Answer::Text(t)) => t.trim().eq_ignore_ascii_case(word.trim()),
        (Question::Pair { correct, .. }, Answer::Pair(placed)) => {
            correct.len() == placed.len() && correct.iter().zip(placed).all(|(c, p)| p.map_or(false, |p| p == *c))
        }
        _ => false,
    }
}

/// 从库里随机抽 n 个词出题。
///
/// `kinds` 是本次要混的题型，循环取用；`seed` 决定随机结果（同 seed 出同样的题，便于复现与测试）。
pub fn build_quiz(words: &[&Word], n: usize, kinds: &[Kind], seed: u64) -> Vec<Question> {
    let mut rng = seed | 1;
    let mut pool: Vec<&Word> = words.to_vec();
    shuffle(&mut pool, &mut rng);
    let mut out: Vec<Question> = Vec::new();
    if kinds.is_empty() {
        return out;
    }
    let mut k = 0usize;
    while out.len() < n && k < n * 8 + 16 {
        let kind = kinds[k % kinds.len()];
        k += 1;
        match kind {
            Kind::Choice | Kind::Listen => {
                let Some(w) = pool.first().copied() else { break };
                let Some(meaning) = w.meaning().map(|s| s.to_string()) else {
                    pool.remove(0);
                    continue;
                };
                pool.remove(0);
                // 中译英需要别的英文词当干扰项；词太少就只出英译中
                let to_english = kind == Kind::Choice && words.len() >= 2 && next_rand(&mut rng) % 2 == 0;
                let (prompt, options, answer) = if to_english {
                    let mut opts = vec![w.word.clone()];
                    for o in words.iter() {
                        if opts.len() >= 4 {
                            break;
                        }
                        if !opts.iter().any(|x| x.eq_ignore_ascii_case(&o.word)) {
                            opts.push(o.word.clone());
                        }
                    }
                    let mut all = opts;
                    shuffle(&mut all, &mut rng);
                    let answer = all.iter().position(|x| x.eq_ignore_ascii_case(&w.word)).unwrap_or(0);
                    (meaning.clone(), all, answer)
                } else {
                    let mut opts = vec![meaning.clone()];
                    for o in words.iter() {
                        if opts.len() >= 4 {
                            break;
                        }
                        if let Some(m) = o.meaning() {
                            if !opts.iter().any(|x| x == m) {
                                opts.push(m.to_string());
                            }
                        }
                    }
                    let mut all = opts;
                    shuffle(&mut all, &mut rng);
                    let answer = all.iter().position(|x| *x == meaning).unwrap_or(0);
                    (w.word.clone(), all, answer)
                };
                if kind == Kind::Choice {
                    out.push(Question::Choice { word: w.word.clone(), prompt, options, answer, to_english });
                } else {
                    out.push(Question::Listen { word: w.word.clone(), options, answer });
                }
            }
            Kind::Spell => {
                let Some(w) = pool.first().copied() else { break };
                let Some(meaning) = w.meaning().map(|s| s.to_string()) else {
                    pool.remove(0);
                    continue;
                };
                pool.remove(0);
                out.push(Question::Spell {
                    word: w.word.clone(),
                    meaning,
                    sentence: blank_out(&w.example, &w.word),
                });
            }
            Kind::Pair => {
                // 配对题一次要四个词，凑不齐就跳过这题
                if pool.len() < 4 {
                    continue;
                }
                let picked: Vec<&Word> = pool.drain(..4).collect();
                let mut pairs: Vec<(String, String)> = Vec::new();
                for w in picked {
                    if let Some(m) = w.meaning() {
                        pairs.push((w.word.clone(), m.to_string()));
                    }
                }
                if pairs.len() < 2 {
                    continue;
                }
                // 左右两边都打乱：不然用户照着位置抄就是「配对」了
                let mut lp: Vec<usize> = (0..pairs.len()).collect();
                shuffle(&mut lp, &mut rng);
                let mut rp: Vec<usize> = (0..pairs.len()).collect();
                shuffle(&mut rp, &mut rng);
                let left: Vec<String> = lp.iter().map(|i| pairs[*i].0.clone()).collect();
                let right: Vec<String> = rp.iter().map(|i| pairs[*i].1.clone()).collect();
                // 右边第 j 个槽该放左边第几个词：找 right[j] 的词在 left 里的位置
                let correct: Vec<usize> = rp
                    .iter()
                    .map(|i| lp.iter().position(|x| x == i).unwrap_or(0))
                    .collect();
                out.push(Question::Pair { left, right, correct });
            }
        }
    }
    out
}

/// 把例句里的单词挖成下划线。
///
/// 只按 ASCII 小写匹配（`to_ascii_lowercase` 不改长度，字节下标才对得上），
/// 且**只挖整词**：`glance` 不该把 `glanced` 挖成 `______d`。
pub fn blank_out(sentence: &str, word: &str) -> String {
    let w = word.trim();
    if sentence.trim().is_empty() || w.is_empty() {
        return String::new();
    }
    let s_chars: Vec<char> = sentence.chars().collect();
    let w_chars: Vec<char> = w.chars().map(|c| c.to_ascii_lowercase()).collect();
    let s_lower: Vec<char> = s_chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mask = "_".repeat(w_chars.len());
    let mut out = String::new();
    let mut i = 0usize;
    while i < s_chars.len() {
        let hit = i + w_chars.len() <= s_chars.len()
            && s_lower[i..i + w_chars.len()] == w_chars[..]
            && (i == 0 || !s_chars[i - 1].is_alphabetic())
            && (i + w_chars.len() == s_chars.len() || !s_chars[i + w_chars.len()].is_alphabetic());
        if hit {
            out.push_str(&mask);
            i += w_chars.len();
        } else {
            out.push(s_chars[i]);
            i += 1;
        }
    }
    if out == sentence {
        // 例句里没有这个词，挖不出来
        return String::new();
    }
    out
}

/// xorshift64*：够随机、可复现、不引依赖
fn next_rand(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// 原地洗牌
pub fn shuffle<T>(v: &mut [T], state: &mut u64) {
    if v.len() < 2 {
        return;
    }
    for i in (1..v.len()).rev() {
        let j = (next_rand(state) % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

/// 一次测验的会话：题目 + 作答 + 进度
#[derive(Debug, Clone, Default)]
pub struct QuizSession {
    pub questions: Vec<Question>,
    /// 当前第几题
    pub idx: usize,
    /// 每题的选择
    pub answers: Vec<Answer>,
}

impl QuizSession {
    /// 出卷。词不够时出不满 n 题就是不满，不硬凑
    pub fn new(words: &[&Word], n: usize, kinds: &[Kind], seed: u64) -> Self {
        let questions = build_quiz(words, n, kinds, seed);
        let answers = vec![Answer::None; questions.len()];
        Self { questions, idx: 0, answers }
    }

    pub fn current(&self) -> Option<&Question> {
        self.questions.get(self.idx)
    }

    pub fn current_answer(&self) -> &Answer {
        self.answers.get(self.idx).unwrap_or(&Answer::None)
    }

    /// 当前题是否已作答
    pub fn is_answered(&self) -> bool {
        !matches!(self.current_answer(), Answer::None)
    }

    /// 提交当前题的作答，返回对错
    pub fn submit(&mut self, a: Answer) -> bool {
        let Some(q) = self.questions.get(self.idx) else { return false };
        let ok = check(q, &a);
        if let Some(slot) = self.answers.get_mut(self.idx) {
            *slot = a;
        }
        ok
    }

    /// 下一题。最后一题之后不再动
    pub fn next(&mut self) {
        if self.idx + 1 < self.questions.len() {
            self.idx += 1;
        }
    }

    /// 是否已经到最后一题
    pub fn at_last(&self) -> bool {
        self.idx + 1 >= self.questions.len()
    }

    /// 得分（对, 总）
    pub fn score(&self) -> (usize, usize) {
        let total = self.questions.len();
        let right = self
            .questions
            .iter()
            .zip(self.answers.iter())
            .filter(|(q, a)| check(q, a))
            .count();
        (right, total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(w: &str, zh: &str) -> Word {
        Word {
            word: w.into(),
            phonetic: String::new(),
            senses: vec![Sense { pos: "n.".into(), text: zh.into() }],
            example: String::new(),
            example_zh: String::new(),
            first_seen: "2026-10-27 10:00".into(),
            last_seen: "2026-10-27 10:00".into(),
            source: "桌面端".into(),
        }
    }

    fn words() -> Vec<Word> {
        vec![
            word("glance", "一瞥"),
            word("robust", "稳健的"),
            word("resilient", "有韧性的"),
            word("scrutinize", "仔细检查"),
            word("verify", "核实"),
            word("ubiquitous", "无处不在的"),
        ]
    }

    fn refs(v: &[Word]) -> Vec<&Word> {
        v.iter().collect()
    }

    #[test]
    fn upsert_keeps_one_entry_per_word() {
        let mut v = Vocab::default();
        v.upsert(word("glance", "一瞥"));
        v.upsert(word("glance", "扫一眼"));
        assert_eq!(v.len(), 1);
        assert_eq!(v.words()[0].meaning(), Some("扫一眼"), "再次收录要更新释义");
    }

    #[test]
    fn upsert_ignores_case_differences() {
        let mut v = Vocab::default();
        v.upsert(word("Glance", "一瞥"));
        v.upsert(word("glance", "扫一眼"));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn remove_drops_the_word() {
        let mut v = Vocab::default();
        v.upsert(word("glance", "一瞥"));
        v.remove("glance");
        assert!(v.is_empty());
    }

    #[test]
    fn meaning_takes_only_the_first_sense() {
        let mut w = word("glance", "一瞥");
        w.senses[0].text = "一瞥，看一眼；浏览；掠过".into();
        assert_eq!(w.meaning(), Some("一瞥，看一眼"));
    }

    #[test]
    fn only_latin_queries_are_recorded() {
        let store: SharedVocab = Arc::new(Mutex::new(Vocab::default()));
        record_markers(&store, "你好", "#词 你好\n#释 n. hello", "桌面端");
        record_markers(&store, "glance", "#词 glance\n#释 n. 一瞥", "桌面端");
        let g = store.lock().unwrap();
        assert_eq!(g.len(), 1, "中文查询不进生词库");
        assert_eq!(g.words()[0].word, "glance");
    }

    #[test]
    fn recording_can_be_switched_off() {
        let store: SharedVocab = Arc::new(Mutex::new(Vocab::default()));
        let mut s = VocabSettings::default();
        s.record = false;
        store.lock().unwrap().set_settings(s);
        record_markers(&store, "glance", "#词 glance\n#释 n. 一瞥", "桌面端");
        assert!(store.lock().unwrap().is_empty());
    }

    #[test]
    fn record_keeps_phonetic_and_example() {
        let store: SharedVocab = Arc::new(Mutex::new(Vocab::default()));
        record_markers(
            &store,
            "glance",
            "#词 glance\n#音 /ɡlɑːns/\n#释 n. 一瞥\n#例 She glanced at her watch.\n#译 她瞥了一眼手表。",
            "插件",
        );
        let g = store.lock().unwrap();
        let w = &g.words()[0];
        assert_eq!(w.phonetic, "/ɡlɑːns/");
        assert_eq!(w.example, "She glanced at her watch.");
        assert_eq!(w.source, "插件");
    }

    #[test]
    fn quiz_gives_the_requested_number_of_questions() {
        let q = build_quiz(&refs(&words()), 5, &[Kind::Choice, Kind::Spell, Kind::Listen], 42);
        assert_eq!(q.len(), 5, "{q:?}");
    }

    #[test]
    fn quiz_options_contain_the_right_answer() {
        let q = build_quiz(&refs(&words()), 6, &[Kind::Choice], 7);
        for item in &q {
            let Question::Choice { options, answer, .. } = item else { panic!("题型不对：{item:?}") };
            assert!(*answer < options.len());
            assert!(!options.is_empty());
        }
    }

    #[test]
    fn same_seed_gives_the_same_quiz() {
        let a = build_quiz(&refs(&words()), 4, &Kind::all(), 99);
        let b = build_quiz(&refs(&words()), 4, &Kind::all(), 99);
        assert_eq!(a, b);
    }

    #[test]
    fn pair_question_matches_left_and_right() {
        let q = build_quiz(&refs(&words()), 4, &[Kind::Pair], 5);
        let Some(Question::Pair { left, right, correct }) = q.first() else { panic!("没出配对题") };
        assert_eq!(left.len(), right.len());
        assert_eq!(correct.len(), right.len());
        // 正确映射：right[j] 对应 left[correct[j]]
        let all = words();
        for (j, r) in right.iter().enumerate() {
            let l = &left[correct[j]];
            let w = all.iter().find(|w| &w.word == l).unwrap();
            assert_eq!(w.meaning(), Some(r.as_str()));
        }
    }

    #[test]
    fn checking_answers() {
        let q = Question::Choice {
            word: "glance".into(),
            prompt: "glance".into(),
            options: vec!["一瞥".into(), "稳健的".into()],
            answer: 1,
            to_english: false,
        };
        assert!(check(&q, &Answer::Choice(1)));
        assert!(!check(&q, &Answer::Choice(0)));
        assert!(!check(&q, &Answer::None));

        let s = Question::Spell { word: "Glance".into(), meaning: "一瞥".into(), sentence: String::new() };
        assert!(check(&s, &Answer::Text(" glance ".into())), "拼写忽略大小写与空格");
        assert!(!check(&s, &Answer::Text("glances".into())));

        let p = Question::Pair { left: vec!["a".into(), "b".into()], right: vec!["x".into(), "y".into()], correct: vec![1, 0] };
        assert!(check(&p, &Answer::Pair(vec![Some(1), Some(0)])));
        assert!(!check(&p, &Answer::Pair(vec![Some(0), Some(1)])));
        assert!(!check(&p, &Answer::Pair(vec![Some(1), None])));
    }

    #[test]
    fn blank_out_masks_the_word() {
        assert_eq!(blank_out("She glanced at her watch.", "glanced"), "She _______ at her watch.");
        assert_eq!(blank_out("She glanced at her watch.", "glance"), "", "例句里没有这个词就别挖");
        assert_eq!(blank_out("", "glance"), "");
    }

    #[test]
    fn session_tracks_score_and_progress() {        let mut s = QuizSession::new(&refs(&words()), 3, &[Kind::Choice], 3);
        assert_eq!(s.questions.len(), 3);
        assert!(!s.is_answered());
        let q = s.questions[0].clone();
        let Question::Choice { answer, .. } = &q else { panic!() };
        assert!(s.submit(Answer::Choice(*answer)));
        assert!(s.is_answered());
        assert_eq!(s.score().0, 1);
        s.next();
        assert_eq!(s.idx, 1);
        s.next();
        s.next();
        assert_eq!(s.idx, 2, "到最后一题就不再往前了");
        assert!(s.at_last());
    }

    #[test]
    fn push_only_fires_at_the_set_minute_and_once_a_day() {
        let s = VocabSettings::default();
        assert!(push_due(&s, "", 19, 0, "2026-10-27"), "到点该推");
        assert!(!push_due(&s, "", 19, 1, "2026-10-27"), "不是那个分钟不推");
        assert!(!push_due(&s, "2026-10-27", 19, 0, "2026-10-27"), "今天推过就不再推");
        let mut off = s.clone();
        off.push_enabled = false;
        assert!(!push_due(&off, "", 19, 0, "2026-10-27"), "关了就不推");
    }

    #[test]
    fn vocab_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("tt-vocab-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("words.json");
        let mut v = Vocab::open(Some(path.clone()));
        v.upsert(word("glance", "一瞥"));
        v.set_last_push("2026-10-27");
        v.save().unwrap();

        let again = Vocab::open(Some(path.clone()));
        assert_eq!(again.len(), 1);
        assert_eq!(again.words()[0].word, "glance");
        assert_eq!(again.last_push(), "2026-10-27");
        std::fs::remove_dir_all(&dir).ok();
    }
}
