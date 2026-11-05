/**
 * 直连第三方模型接口（OpenAI 兼容的 /v1/chat/completions）。
 *
 * 桌面端那条路（background.ts 的 callLocal）一个字没动；这里是给「只装插件、
 * 没装桌面端」的场景准备的第二条路，比如上架 Chrome 商店后单独用。
 *
 * 提示词、打包格式、HTML 保护三样都抄自 crates/ai-translate/src
 * （prompts.rs / batch.rs / html_guard.rs），tests/direct.test.mjs 直接从 Rust
 * 源码抠原文逐条对跑——改一边必须改另一边，抄一份迟早漂移。
 *
 * 方向固定「译成简体中文」：插件这边本来就跳过中文段（worthTranslating），
 * 不存在中译英的场景，桌面端的 direction 是配置项、这里没有对应物。
 */
import type { Scene, TranslateItem, TranslateResult } from "./shared";

/* ---------------- 提示词（抄自 prompts.rs） ---------------- */

const COMMON =
  "你是专业翻译。把用户给的内容翻译成简体中文；已经是简体中文的内容原样输出。专有名词、产品名、代码、URL、数字与单位保留原文。只输出译文，不加解释、不加引号、不加前缀。";

const FULL_PAGE =
  "输入是网页里的多个段落，每段一行，行首有编号 ⟨n⟩。输出时每段同样一行、保留相同编号 ⟨n⟩ 与顺序，不合并、不拆分、不遗漏任何一段。文中形如 ⟦1⟧、⟦/1⟧、⟦2/⟧ 的记号是格式占位符：必须原样保留、数量不变、相对顺序不变，只翻译它们之间和周围的文字。";

const SELECTION =
  "输入是用户在阅读时选中的一小段文字。如果只是一个单词或短语，第一行给译文，第二行用一句话给出简短释义（词性与常见含义）；如果是句子或段落，只给译文。";

const HOVER = "输入是用户鼠标悬停的一个段落。输出整段译文，不加解释。";

const SCREENSHOT =
  "输入来自屏幕截图的文字识别结果，可能有错字、断行与多余空格。先在心里纠正明显的识别错误并合并被断开的句子，再翻译；只输出译文。";

const WORD =
  "你是英汉词典。用户取到一个英文词或短语，请给出词典条目。严格按下面的行格式输出，一行一条，行首标记不能改；不知道的整行省略，不要写「无」或「暂缺」；除这些行外不要输出任何说明、标题或代码块围栏。\n#词 词条本身，保持原样拼写\n#音 英 /国际音标/ 美 /国际音标/\n#释 词性缩写. 该词性下的常见中文释义，多个义项用「；」隔开\n#形 词形变化，如：复数 xxx；过去式 xxx；比较级 xxx\n#例 一个英文短例句\n#译 上面那句例句的中文翻译\n其中 #释 按常用程度排序，一个词性一行，最多 4 行；人名、地名、缩写、专有名词也用 #释 给出（词性写 n. 或 abbr.）。短语没有音标与词形变化时，省掉 #音 与 #形 两行。";

/** 场景对应的系统提示词 */
export function systemPrompt(scene: Scene): string {
  if (scene === "word") return WORD;
  const part =
    scene === "full_page" ? FULL_PAGE : scene === "selection" ? SELECTION : scene === "screenshot" ? SCREENSHOT : HOVER;
  return `${COMMON}\n${part}`;
}

/** 用户消息：可选上下文 + 待译文本 */
export function userMessage(scene: Scene, text: string, context?: string): string {
  let m = "";
  const c = (context ?? "").trim();
  if (c) m += `上下文（仅供参考，不要翻译）：\n${c}\n\n`;
  m += scene === "full_page" ? "请翻译以下各段：\n" : scene === "word" ? "请给出这个词的词典条目：\n" : "请翻译：\n";
  return m + text;
}

/* ---------------- 打包 / 拆包（抄自 batch.rs） ---------------- */

/** 每段一行 "⟨n⟩ 文本"，段内换行折叠成空格 */
export function pack(items: string[]): string {
  return items.map((t, i) => `⟨${i + 1}⟩ ${t.trim().split(/\s+/).filter(Boolean).join(" ")}`).join("\n");
}

/** 按编号拆回；缺号返回 null，重复编号取第一次，无编号的行忽略 */
export function unpack(reply: string, n: number): (string | null)[] {
  const out: (string | null)[] = new Array(n).fill(null);
  for (const line of reply.split(/\r?\n/)) {
    const at = line.indexOf("⟨");
    if (at < 0) continue;
    const m = /^⟨\s*(\d+)\s*⟩\s*([\s\S]*)$/.exec(line.slice(at));
    if (!m) continue;
    const k = Number(m[1]);
    if (k >= 1 && k <= n && out[k - 1] === null) out[k - 1] = m[2].trim();
  }
  return out;
}

/* ---------------- 行内 HTML 保护（抄自 html_guard.rs） ---------------- */

const VOID_TAGS = new Set([
  "br", "img", "wbr", "hr", "input", "source", "embed", "area", "col", "base", "link", "meta", "param", "track",
]);

export interface Protected {
  text: string;
  /** tags[i] = 第 i+1 号占位符对应的原始标签串 */
  tags: string[];
}

function tagName(tag: string): string {
  const s = tag.replace(/^<+/, "").replace(/^\/+/, "").replace(/[^A-Za-z0-9-][\s\S]*$/, "");
  return s.toLowerCase();
}

/** 把标签换成 ⟦n⟧ / ⟦/n⟧ / ⟦n/⟧ 占位符 */
export function protectHtml(html: string): Protected {
  let text = "";
  const tags: string[] = [];
  const open: { num: number; name: string }[] = [];
  let rest = html;
  for (;;) {
    const start = rest.indexOf("<");
    if (start < 0) break;
    const endRel = rest.indexOf(">", start);
    if (endRel < 0) break;
    const end = endRel + 1;
    const tag = rest.slice(start, end);
    text += rest.slice(0, start);
    const name = tagName(tag);
    if (name === "") {
      text += tag; // 不是标签（如 "a < b"），原样保留
    } else if (tag.startsWith("</")) {
      let pos = -1;
      for (let i = open.length - 1; i >= 0; i--) {
        if (open[i].name === name) {
          pos = i;
          break;
        }
      }
      if (pos >= 0) {
        const num = open[pos].num;
        open.splice(pos, 1);
        text += `⟦/${num}⟧`;
      } else {
        tags.push(tag);
        text += `⟦${tags.length}/⟧`;
      }
    } else if (tag.endsWith("/>") || VOID_TAGS.has(name)) {
      tags.push(tag);
      text += `⟦${tags.length}/⟧`;
    } else {
      tags.push(tag);
      const num = tags.length;
      open.push({ num, name });
      text += `⟦${num}⟧`;
    }
    rest = rest.slice(end);
  }
  text += rest;
  return { text, tags };
}

/** 占位符换回原标签；对不上（缺号、多号、编号越界）返回 null */
export function restoreHtml(translated: string, tags: string[]): string | null {
  let out = "";
  const seen = new Array(tags.length).fill(0);
  let rest = translated;
  for (;;) {
    const start = rest.indexOf("⟦");
    if (start < 0) break;
    const endRel = rest.indexOf("⟧", start);
    if (endRel < 0) return null; // 占位符未闭合
    const end = endRel + 1;
    const inner = rest.slice(start + 1, end - 1);
    out += rest.slice(0, start);
    let numStr: string;
    let kind: "open" | "close" | "void";
    if (inner.startsWith("/")) {
      kind = "close";
      numStr = inner.slice(1);
    } else if (inner.endsWith("/")) {
      kind = "void";
      numStr = inner.slice(0, -1);
    } else {
      kind = "open";
      numStr = inner;
    }
    const num = Number(numStr.trim());
    if (!Number.isInteger(num) || num < 1 || num > tags.length) return null;
    const tag = tags[num - 1];
    if (kind === "open") {
      seen[num - 1] += 1;
      out += tag;
    } else if (kind === "close") {
      seen[num - 1] += 1;
      out += `</${tagName(tag)}>`;
    } else {
      seen[num - 1] += 2;
      out += tag;
    }
    rest = rest.slice(end);
  }
  out += rest;
  return seen.every((c) => c === 2) ? out : null;
}

/* ---------------- 接口调用 ---------------- */

export interface DirectConfig {
  /** 接口地址，填到 /v1 这一层，如 https://api.deepseek.com/v1 */
  base: string;
  key: string;
  model: string;
}

/** 去掉尾部斜杠与多余的 /chat/completions */
export function normalizeBase(base: string): string {
  return base.trim().replace(/\/+$/, "").replace(/\/chat\/completions$/, "");
}

export function chatUrl(base: string): string {
  return `${normalizeBase(base)}/chat/completions`;
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * 发一次对话请求，拿回文本。
 * 跨域由 host_permissions 放行（options 页保存时会申请），不让 CORS 决定成败。
 */
export async function chat(cfg: DirectConfig, system: string, user: string, timeoutMs = 60000): Promise<string> {
  if (!cfg.base) throw new Error("没填接口地址");
  if (!cfg.key) throw new Error("没填 API Key");
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), timeoutMs);
  try {
    const resp = await fetch(chatUrl(cfg.base), {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: `Bearer ${cfg.key}` },
      body: JSON.stringify({
        model: cfg.model,
        messages: [
          { role: "system", content: system },
          { role: "user", content: user },
        ],
        stream: false,
        temperature: 0.2,
      }),
      signal: ctrl.signal,
    });
    if (!resp.ok) {
      const detail = (await resp.text().catch(() => "")).slice(0, 300);
      throw new Error(`接口返回 ${resp.status}${detail ? `：${detail}` : ""}`);
    }
    const j = await resp.json();
    const text = j?.choices?.[0]?.message?.content;
    if (typeof text !== "string") throw new Error("接口没返回译文（回包形状不对）");
    return text;
  } catch (e) {
    if (e instanceof Error && e.name === "AbortError") throw new Error("接口超时");
    if (e instanceof TypeError) {
      throw new Error(`连不上接口地址（${e.message}）。若浏览器报的是权限问题，请到扩展设置里允许访问该站点。`);
    }
    throw e;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * 拉模型列表。规则照抄桌面端 `models.rs` 的 `parse_ids`：
 * 只认 OpenAI 兼容的 `{"data":[{"id":…}]}` 形状，空列表算失败（别给用户一个空下拉）。
 * 失败要把原因带回去——桌面端是写进设置页提示行，别静默。
 */
export async function listModels(cfg: DirectConfig): Promise<{ models: string[] } | { error: string }> {
  if (!cfg.base.trim()) return { error: "还没填接口地址" };
  if (!cfg.key.trim()) return { error: "还没填 API Key" };
  let text: string;
  try {
    const resp = await fetch(`${normalizeBase(cfg.base)}/models`, {
      headers: { Authorization: `Bearer ${cfg.key}` },
    });
    text = await resp.text();
    if (!resp.ok) {
      return { error: `接口返回 ${resp.status}：${text.slice(0, 120)}` };
    }
  } catch (e) {
    return { error: errText(e) };
  }
  let j: unknown;
  try {
    j = JSON.parse(text);
  } catch {
    return { error: "回包不是 JSON，可能不是 OpenAI 兼容接口" };
  }
  const arr = (j as { data?: unknown })?.data;
  if (!Array.isArray(arr)) return { error: "回包里没有 data 数组，可能不是 OpenAI 兼容接口" };
  const ids = arr
    .map((m) => (m as { id?: unknown })?.id)
    .filter((x): x is string => typeof x === "string")
    .sort();
  const uniq = [...new Set(ids)];
  if (uniq.length === 0) return { error: "接口返回的模型列表是空的" };
  return { models: uniq };
}

/* ---------------- 翻译主流程 ---------------- */

/** 整页一块一次请求，块大小跟桌面端 batch_size 默认值一致 */
const CHUNK_SIZE = 10;

/** 直连模式翻译一批段落，返回与桌面端一样的 results 结构 */
export async function translateDirect(
  cfg: DirectConfig,
  scene: Scene,
  items: TranslateItem[],
  context?: string,
): Promise<TranslateResult[]> {
  if (items.length === 0) return [];
  if (scene !== "full_page") {
    // 一段一个请求：与桌面端 handle_translate 的单段分支一致（划词/悬停/查词都走这里）
    return Promise.all(
      items.map((it) => translateSegment(cfg, scene, it, context).catch((e) => ({ id: it.id, error: errText(e) }))),
    );
  }
  const out: TranslateResult[] = [];
  for (let i = 0; i < items.length; i += CHUNK_SIZE) {
    out.push(...(await translateChunk(cfg, scene, items.slice(i, i + CHUNK_SIZE), context)));
  }
  return out;
}

/** 整页的一整块：编号打包 → 一次请求 → 按编号拆回 */
async function translateChunk(
  cfg: DirectConfig,
  scene: Scene,
  items: TranslateItem[],
  context?: string,
): Promise<TranslateResult[]> {
  const prot = items.map((it) => (it.is_html ? protectHtml(it.text) : { text: it.text, tags: [] as string[] }));
  const sys = systemPrompt(scene);
  const packed = pack(prot.map((p) => p.text));
  let parts: (string | null)[];
  try {
    parts = unpack(await chat(cfg, sys, userMessage(scene, packed, context)), items.length);
  } catch (e) {
    // 整包失败不拖死整页：逐段再试一次
    const err = errText(e);
    return Promise.all(
      items.map((it) => translateSegment(cfg, scene, it, context).catch(() => ({ id: it.id, error: err }))),
    );
  }
  return Promise.all(
    items.map((it, k) => {
      const t = parts[k];
      if (t === null) {
        // 这一段没回来（模型漏了一段）：单独再试一次
        return translateSegment(cfg, scene, it, context).catch((e) => ({ id: it.id, error: errText(e) }));
      }
      if (!it.is_html) return { id: it.id, text: t };
      const restored = restoreHtml(t, prot[k].tags);
      return restored === null ? { id: it.id, error: "译文里的格式占位符对不上" } : { id: it.id, text: restored };
    }),
  );
}

/** 单段翻译：HTML 段先摘标签再翻、译完还原 */
async function translateSegment(
  cfg: DirectConfig,
  scene: Scene,
  item: TranslateItem,
  context?: string,
): Promise<TranslateResult> {
  const p = item.is_html ? protectHtml(item.text) : { text: item.text, tags: [] as string[] };
  // 整页的提示词要求「每段一行、行首有编号 ⟨n⟩」，单段也照样编号，回包再拆掉编号
  const numbered = scene === "full_page";
  const text = numbered ? pack([p.text]) : p.text;
  const reply = (await chat(cfg, systemPrompt(scene), userMessage(scene, text, context))).trim();
  const body = numbered ? (unpack(reply, 1)[0] ?? reply.replace(/^⟨\s*1\s*⟩\s*/, "")) : reply;
  if (!item.is_html) return { id: item.id, text: body };
  const restored = restoreHtml(body, p.tags);
  return restored === null ? { id: item.id, error: "译文里的格式占位符对不上" } : { id: item.id, text: restored };
}
