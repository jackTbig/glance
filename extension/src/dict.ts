/**
 * 取词词典卡：判定「这算不算查词」，以及解析模型回的行标记条目。
 *
 * 桌面端与插件端要给出**一样的取词卡片**，所以这两件事必须两边一致。
 * Rust 侧的权威实现在 `crates/ai-translate/src/scene.rs`（`looks_like_word`）
 * 与 `apps/desktop/src/dict.rs`（`parse`）。**改任一边都要同步改另一边**，
 * 否则同一个词在桌面端给词典卡、在网页里只给一句译文。
 */

/** 中日韩文字（含假名与谚文） */
function isCjk(ch: string): boolean {
  const c = ch.codePointAt(0) ?? 0;
  return (
    (c >= 0x3040 && c <= 0x30ff) || // 假名
    (c >= 0x3400 && c <= 0x4dbf) || // 扩展 A
    (c >= 0x4e00 && c <= 0x9fff) || // 基本汉字
    (c >= 0xac00 && c <= 0xd7af) || // 谚文
    (c >= 0xf900 && c <= 0xfaff) // 兼容汉字
  );
}

/** 首尾要剥掉的标点：划词很容易多带一个引号或句读 */
const EDGE_PUNCT = "「」『』（）《》，。；：！？、“”‘’";

function trimEdges(s: string): string {
  const isEdge = (ch: string) => /\s/.test(ch) || /[!-/:-@[-`{-~]/.test(ch) || EDGE_PUNCT.includes(ch);
  let a = 0;
  let b = s.length;
  while (a < b && isEdge(s[a])) a++;
  while (b > a && isEdge(s[b - 1])) b--;
  return s.slice(a, b);
}

/**
 * 这段取到的文字够不够格当「查词」：够格就给词典卡，不够格就当句子翻。
 *
 * 只认拉丁字母的词：目标语言本来就是中文，中文段会被跳过；
 * 日韩文的汉字与中文难以区分，一并排除，免得把中文段误判成查词。
 */
export function looksLikeWord(input: string): boolean {
  const t = trimEdges(input.trim());
  if (!t) return false;
  if ([...t].length > 32) return false;
  if (t.includes("\n")) return false;
  if (!/[a-zA-Z]/.test(t)) return false;
  if ([...t].some(isCjk)) return false;
  const words = t.split(/\s+/).filter(Boolean);
  if (words.length === 0 || words.length > 3) return false;
  // 多个词里还夹着句读，那是半句话，不是词组；单个词里的点允许（U.S、Ph.D）
  return words.length === 1 || !/[.,;:!?]/.test(t);
}

export interface Sense {
  /** 词性缩写，如 `v.`；模型没给就是空串 */
  pos: string;
  text: string;
}

export interface DictEntry {
  head: string;
  phonetic: string;
  senses: Sense[];
  forms: string;
  example: string;
  exampleZh: string;
}

/**
 * 常见词性缩写。用白名单而不是「点号前都算词性」：
 * 释义里出现 `e.g.` 之类会把半句话切成一个莫名其妙的标签。
 */
const POS = new Set([
  "n",
  "v",
  "vt",
  "vi",
  "adj",
  "adv",
  "prep",
  "conj",
  "pron",
  "int",
  "interj",
  "num",
  "art",
  "aux",
  "abbr",
  "det",
  "pl",
  "modal",
  "phr",
  "prefix",
  "suffix",
]);

/** 把 `v. 一瞥，看一眼` 拆成词性与释义；开头不是已知词性就整行当释义 */
function splitPos(body: string): Sense {
  const at = body.indexOf(".");
  if (at > 0) {
    const head = body.slice(0, at).trim();
    const tail = body.slice(at + 1).trim();
    if (POS.has(head.toLowerCase()) && tail) return { pos: head + ".", text: tail };
  }
  return { pos: "", text: body };
}

/**
 * 解析模型回的行标记词典条目。一条标记行都没有就返回 null，调用方退回普通译文渲染。
 *
 * 解析刻意宽容：认代码块围栏、认缩进、认模型多写的说明行。
 * 用行标记而不是 JSON，是因为卡片会流式上屏——行格式每来一行就能画一行，
 * 模型少给一行只是少一块；JSON 一旦截断则整块作废。
 */
export function parseDictEntry(raw: string): DictEntry | null {
  const e: DictEntry = { head: "", phonetic: "", senses: [], forms: "", example: "", exampleZh: "" };
  let hit = false;
  for (const rawLine of raw.split(/\r?\n/)) {
    const line = rawLine.trim().replace(/^`+/, "").trim();
    if (!line.startsWith("#")) continue;
    const tag = line.slice(1, 2);
    const body = line.slice(2).trim();
    if (!body) continue;
    if (tag === "词") e.head = body;
    else if (tag === "音") e.phonetic = body;
    else if (tag === "释") e.senses.push(splitPos(body));
    else if (tag === "形") e.forms = body;
    else if (tag === "例") e.example = body;
    else if (tag === "译") e.exampleZh = body;
    else continue;
    hit = true;
  }
  return hit ? e : null;
}

/** 卡片上「复制」拿走的纯文本 */
export function dictEntryToText(e: DictEntry): string {
  const lines: string[] = [];
  if (e.head) lines.push(e.head);
  if (e.phonetic) lines.push(e.phonetic);
  for (const s of e.senses) lines.push(s.pos ? s.pos + " " + s.text : s.text);
  if (e.forms) lines.push(e.forms);
  if (e.example) lines.push(e.example);
  if (e.exampleZh) lines.push(e.exampleZh);
  return lines.join("\n");
}
