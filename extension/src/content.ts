/** 页面里的整页翻译、悬停翻译、划词翻译 */
import {
  blockAt,
  collectBlocks,
  DEFAULT_SETTINGS,
  dictEntryToText,
  looksLikeWord,
  parseDictEntry,
  type DictEntry,
  hasInlineMarkup,
  isTyping,
  MARK_ATTR,
  matchesHotkey,
  TRANSLATION_CLASS,
  worthTranslating,
  type DisplayMode,
  type PageCommand,
  type PageStatus,
  type Reply,
  type Scene,
  type Settings,
  type TranslateItem,
  type TranslateResult,
} from "./shared";

/** 包住原文的壳，用于「仅译文」时单独隐藏原文 */
const ORIGINAL_CLASS = "tt-original";

/** 扩展重新加载后，老页面里残留的这份脚本就失效了；所有 DOM 监听挂在这上面，一次全摘 */
const abort = new AbortController();
const EXT_GONE = "插件刚更新过，请刷新页面（F5）后再用。";

let settings: Settings = DEFAULT_SETTINGS;
let display: DisplayMode = "original";
let pageTranslated = false;
let busy = false;

const styleId = "tt-style";
const CSS = `
.${TRANSLATION_CLASS} {
  display: block;
  margin: 4px 0 8px 0;
  padding-left: 8px;
  border-left: 3px solid rgba(80,140,240,.55);
  color: inherit;
  font-size: inherit;
  line-height: 1.6;
}
/* 进度与失败提示跟在原文最后一个字后面，行内、极小，尽量不动页面布局 */
.tt-badge {
  display: inline-block; vertical-align: middle; margin-left: .35em;
  line-height: 1; font-size: .85em; font-style: normal; user-select: none;
}
.tt-badge.tt-loading {
  width: .85em; height: .85em; border-radius: 50%;
  border: 2px solid rgba(80,140,240,.25); border-top-color: rgba(80,140,240,.9);
  animation: tt-spin .8s linear infinite;
}
.tt-badge.tt-error { color: rgba(205,60,60,.95); cursor: pointer; white-space: nowrap; }
.tt-badge.tt-error:hover { text-decoration: underline; }
@keyframes tt-spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .tt-badge.tt-loading { animation-duration: 2.4s; } }
/* 原文包一层 display:contents 的壳：结构上存在、布局上等于不存在，
   这样「仅译文」只藏原文而不动页面其它部分（早先按 body 直接子元素隐藏，会把整页连译文一起藏掉） */
.${ORIGINAL_CLASS} { display: contents; }
.tt-hide-original .${ORIGINAL_CLASS} { display: none; }
/* 单段翻译（悬停 / 快捷键）永远中英对照：整页处于「仅译文」时，这一段的原文也留着 */
.tt-hide-original .tt-paired > .${ORIGINAL_CLASS} { display: contents; }
.tt-working { outline: 2px dashed rgba(80,140,240,.5); outline-offset: 2px; }
#tt-toast {
  position: fixed; z-index: 2147483647; right: 16px; bottom: 16px;
  background: rgba(28,28,32,.95); color: #fff; padding: 10px 14px; border-radius: 8px;
  font: 13px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; max-width: 360px;
  box-shadow: 0 4px 20px rgba(0,0,0,.35);
}
#tt-icon {
  position: absolute; z-index: 2147483647; width: 26px; height: 26px; border-radius: 6px;
  background: #285ab4; color: #fff; border: none; cursor: pointer;
  font: 14px/26px system-ui, sans-serif; text-align: center; padding: 0;
  box-shadow: 0 2px 8px rgba(0,0,0,.3);
}
/* 划词卡片与桌面端的取词卡片是同一张卡：配色、字号、版式逐项对齐
   apps/desktop/src/ui.rs 的 draw_panel 与 draw_entry。改这里要同步改那边。 */
#tt-panel {
  position: absolute; z-index: 2147483647; width: 420px; max-width: calc(100vw - 16px);
  background: rgba(28,28,32,.965); color: #e6e6e6; border-radius: 10px; padding: 12px;
  font: 15px/1.55 "Microsoft YaHei", "Segoe UI", system-ui, sans-serif;
  box-shadow: 0 6px 24px rgba(0,0,0,.4); box-sizing: border-box; text-align: left;
}
#tt-panel .tt-head {
  display: flex; gap: 6px; align-items: center; padding-bottom: 6px; margin-bottom: 8px;
  border-bottom: 1px solid rgba(255,255,255,.14);
}
#tt-panel .tt-title { color: rgb(150,180,255); font-size: 13px; }
#tt-panel .tt-from { color: #888; font-size: 12px; }
#tt-panel .tt-head button {
  background: rgb(222,222,222); border: none; border-radius: 4px; color: #222;
  cursor: pointer; font-size: 13px; line-height: 1; padding: 3px 6px; min-width: 24px;
}
#tt-panel .tt-head button:hover { background: rgb(240,240,240); }
#tt-panel .tt-head .tt-copy { margin-left: auto; }
#tt-panel .tt-src { color: rgb(150,150,150); font-size: 14px; margin-bottom: 6px; }
#tt-panel .tt-body { color: #fff; font-size: 15px; }
#tt-panel.tt-pinned { border: 1px solid rgba(120,170,255,.5); }
#tt-panel .tt-dict-head { color: #fff; font-size: 24px; font-weight: 600; line-height: 1.25; }
#tt-panel .tt-dict-phon { color: rgb(140,140,140); font-size: 12px; margin-top: 2px; }
#tt-panel .tt-dict-senses { margin-top: 8px; }
#tt-panel .tt-dict-sense { display: flex; align-items: baseline; margin-bottom: 4px; }
#tt-panel .tt-dict-pos { flex: 0 0 34px; color: rgb(150,180,255); font-size: 13px; }
#tt-panel .tt-dict-text { flex: 1; color: rgb(235,235,235); font-size: 15px; }
#tt-panel .tt-dict-forms { color: rgb(130,130,130); font-size: 11px; margin-top: 2px; }
#tt-panel .tt-dict-ex { margin-top: 8px; padding-top: 6px; border-top: 1px solid rgba(255,255,255,.14); }
#tt-panel .tt-dict-ex-en { color: rgb(205,205,205); font-size: 13px; font-style: italic; }
#tt-panel .tt-dict-ex-zh { color: rgb(150,150,150); font-size: 12px; margin-top: 2px; }
`;

function ensureStyle() {
  if (document.getElementById(styleId)) return;
  const s = document.createElement("style");
  s.id = styleId;
  s.textContent = CSS;
  document.documentElement.appendChild(s);
}

let toastTimer: number | null = null;

/** ms = 0 表示常驻，由调用方自己收掉（进度条用） */
function toast(msg: string, ms = 3200) {
  ensureStyle();
  let d = document.getElementById("tt-toast");
  if (!d) {
    d = document.createElement("div");
    d.id = "tt-toast";
    document.body.appendChild(d);
  }
  d.textContent = msg;
  if (toastTimer !== null) {
    clearTimeout(toastTimer);
    toastTimer = null;
  }
  if (ms > 0) {
    toastTimer = window.setTimeout(() => {
      d?.remove();
      toastTimer = null;
    }, ms);
  }
}

/** 扩展上下文还活着吗：重新加载/更新扩展后，老页面里的这份脚本会失去 chrome.runtime.id */
function extensionAlive(): boolean {
  try {
    return chrome.runtime?.id !== undefined;
  } catch {
    return false;
  }
}

/** 上下文没了：摘掉所有监听、清掉页面上的浮层，只提示一次 */
let gaveUp = false;
function giveUp() {
  if (gaveUp) return;
  gaveUp = true;
  abort.abort();
  try {
    icon?.remove();
    panel?.remove();
    observer?.disconnect();
  } catch {
    /* 页面可能已经在卸载 */
  }
  toast(EXT_GONE, 6000);
}

function send(msg: unknown): Promise<Reply> {
  return new Promise((resolve) => {
    if (!extensionAlive()) {
      giveUp();
      resolve({ ok: false, error: EXT_GONE });
      return;
    }
    try {
      chrome.runtime.sendMessage(msg, (r: Reply) => {
        const err = chrome.runtime.lastError?.message;
        if (err) {
          if (/context invalidated|Receiving end does not exist/i.test(err)) giveUp();
          resolve({ ok: false, error: /context invalidated/i.test(err) ? EXT_GONE : err });
        } else {
          resolve(r);
        }
      });
    } catch (e) {
      // 上下文失效时 sendMessage 是同步抛异常，不走回调
      const msg = e instanceof Error ? e.message : String(e);
      if (/context invalidated/i.test(msg)) {
        giveUp();
        resolve({ ok: false, error: EXT_GONE });
      } else {
        resolve({ ok: false, error: msg });
      }
    }
  });
}

async function translate(scene: Scene, items: TranslateItem[], context?: string): Promise<TranslateResult[]> {
  const r = await send({ kind: "translate", scene, items, context });
  if (!r.ok) throw new Error(r.error);
  return (r as { ok: true; results: TranslateResult[] }).results;
}

// ---------------- 整页翻译 ----------------

/** 把元素里原有的内容包进 .tt-original 壳（只包一次） */
function wrapOriginal(el: HTMLElement) {
  if (el.querySelector(`:scope > .${ORIGINAL_CLASS}`)) return;
  const shell = document.createElement("span");
  shell.className = ORIGINAL_CLASS;
  while (el.firstChild) shell.appendChild(el.firstChild);
  el.appendChild(shell);
}

const BADGE_CLASS = "tt-badge";

function clearBadge(el: Element) {
  el.querySelectorAll(`:scope > .${BADGE_CLASS}`).forEach((n) => n.remove());
}

/** 段落已发出去、还没回来：原文末尾跟一个转圈的小点 */
function attachPending(el: HTMLElement) {
  el.querySelectorAll(`:scope > .${TRANSLATION_CLASS}`).forEach((n) => n.remove());
  wrapOriginal(el);
  clearBadge(el);
  const b = document.createElement("span");
  b.className = `${BADGE_CLASS} tt-loading`;
  b.title = "翻译中…";
  b.setAttribute("aria-label", "翻译中");
  el.appendChild(b);
  el.setAttribute(MARK_ATTR, "1");
}

/** 失败：原文末尾跟一个可点的小图标，详细原因放在 tooltip 里，别撑大页面 */
function attachFailed(el: HTMLElement, reason: string, original: string) {
  el.querySelectorAll(`:scope > .${TRANSLATION_CLASS}`).forEach((n) => n.remove());
  wrapOriginal(el);
  clearBadge(el);
  const b = document.createElement("span");
  b.className = `${BADGE_CLASS} tt-error`;
  b.textContent = "⚠ 重试";
  b.title = `翻译失败：${reason}
点击重试`;
  b.addEventListener("click", async (e) => {
    e.stopPropagation();
    e.preventDefault();
    attachPending(el);
    try {
      const [r] = await withTimeout(translate("full_page", [{ id: "retry", text: original, is_html: false }]), ITEM_TIMEOUT_MS);
      if (r.text !== undefined) {
        attachTranslation(el, r.text, false, original);
      } else {
        attachFailed(el, r.error ?? "未知错误", original);
      }
    } catch (err) {
      attachFailed(el, err instanceof Error ? err.message : String(err), original);
    }
  });
  el.appendChild(b);
  el.setAttribute(MARK_ATTR, "1");
}

function attachTranslation(el: HTMLElement, text: string, failed: boolean, original?: string) {
  if (failed) {
    attachFailed(el, text, original ?? el.textContent ?? "");
    return;
  }
  el.querySelectorAll(`:scope > .${TRANSLATION_CLASS}`).forEach((n) => n.remove());
  wrapOriginal(el);
  clearBadge(el); // 译文来了，把转圈或失败图标收掉
  const d = document.createElement("div");
  d.className = TRANSLATION_CLASS;
  // 译文可能带行内标签，用 innerHTML 但先去掉脚本类节点
  d.innerHTML = sanitize(text);
  el.appendChild(d);
  el.setAttribute(MARK_ATTR, "1");
}

/** 只保留安全的行内标签，避免把模型吐出的奇怪内容当 HTML 执行 */
export function sanitize(html: string): string {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  const allowed = new Set(["A", "B", "STRONG", "I", "EM", "U", "S", "CODE", "SPAN", "BR", "SUP", "SUB", "MARK", "SMALL"]);
  const walk = (node: Element) => {
    for (const child of Array.from(node.children)) {
      if (!allowed.has(child.tagName)) {
        child.replaceWith(...Array.from(child.childNodes));
        continue;
      }
      for (const attr of Array.from(child.attributes)) {
        const ok = child.tagName === "A" ? ["href", "title", "target", "rel"].includes(attr.name) : ["title"].includes(attr.name);
        const bad = attr.name.startsWith("on") || /^\s*javascript:/i.test(attr.value);
        if (!ok || bad) child.removeAttribute(attr.name);
      }
      if (child.tagName === "A") {
        child.setAttribute("rel", "noopener noreferrer");
      }
      walk(child);
    }
  };
  walk(tpl.content as unknown as Element);
  return tpl.innerHTML;
}

function setDisplay(mode: DisplayMode) {
  display = mode;
  // 原文的显隐交给 CSS（.tt-hide-original + .tt-original），这里只管译文块本身
  document.body.classList.toggle("tt-hide-original", mode === "translation_only");
  document.querySelectorAll(`.${TRANSLATION_CLASS}`).forEach((n) => {
    (n as HTMLElement).style.display = mode === "original" ? "none" : "block";
  });
}

/** 同时飞几个请求（跟桌面端的并发池对齐）、单段多久算超时 */
let pageConcurrency = 8;
const ITEM_TIMEOUT_MS = 60_000;

/** 给 Promise 加超时，免得某一段永远转圈 */
function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(() => reject(new Error("翻译超时，稍后重试")), ms);
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      (e) => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}

/** 翻一段：先挂上转圈占位，回来后换成译文或失败图标 */
async function translateOneBlock(el: HTMLElement, item: TranslateItem): Promise<boolean> {
  attachPending(el);
  try {
    const [r] = await withTimeout(translate("full_page", [item]), ITEM_TIMEOUT_MS);
    if (r.text !== undefined) {
      attachTranslation(el, r.text, false, item.text);
      return true;
    }
    attachTranslation(el, r.error ?? "未知错误", true, item.text);
    return false;
  } catch (e) {
    attachTranslation(el, e instanceof Error ? e.message : String(e), true, item.text);
    return false;
  }
}

/**
 * 整页翻译：**一段一个请求**，从最上面的段开始发。
 * 发出去的那一段立刻挂上转圈占位，回来就换成译文；失败或超时挂红色警示图标，点它重试。
 * 逐段发比打包发多花些 token（每次都带一遍系统提示词），换来的是每段独立可见、独立重试。
 */
async function translatePage() {
  if (busy) return;
  busy = true;
  try {
    const blocks = collectBlocks(document.body, (e) => getComputedStyle(e));
    if (blocks.length === 0) {
      toast("这个页面没有找到可翻译的段落");
      return;
    }
    const items: TranslateItem[] = blocks.map((el, i) => {
      const isHtml = hasInlineMarkup(el);
      return { id: String(i), text: isHtml ? el.innerHTML : (el.textContent ?? ""), is_html: isHtml };
    });

    const h = await send({ kind: "health" });
    if (h.ok && "health" in h && h.health.concurrency > 0) pageConcurrency = h.health.concurrency;

    const total = items.length;
    let finished = 0;
    let failed = 0;
    toast(`翻译中 0/${total}…`, 0);

    // 从上往下取，同时飞 pageConcurrency 个
    let next = 0;
    const worker = async () => {
      while (next < total) {
        const i = next++;
        const el = blocks[i];
        if (!el?.isConnected) {
          finished++;
          continue;
        }
        const ok = await translateOneBlock(el, items[i]);
        if (!ok) failed++;
        if (!pageTranslated) {
          pageTranslated = true;
          observeNewContent();
        }
        setDisplay(display === "original" ? settings.defaultDisplay : display);
        finished++;
        toast(`翻译中 ${finished}/${total}…`, 0);
      }
    };
    await Promise.all(Array.from({ length: Math.min(pageConcurrency, total) }, worker));

    if (failed === 0) {
      toast(`完成，共 ${total} 段`);
    } else if (failed === total) {
      toast("整页翻译失败，点任意标红的段落可重试", 6000);
    } else {
      toast(`完成 ${total} 段，其中 ${failed} 段失败（点标红处重试）`, 5000);
    }
  } catch (e) {
    toast(e instanceof Error ? e.message : String(e), 6000);
  } finally {
    busy = false;
  }
}

/** 撤掉一个段落的译文，把它还原成翻译前的样子 */
function removeTranslationFrom(el: Element) {
  el.querySelectorAll(`:scope > .${TRANSLATION_CLASS}`).forEach((n) => n.remove());
  clearBadge(el);
  el.querySelectorAll(`:scope > .${ORIGINAL_CLASS}`).forEach((shell) => {
    while (shell.firstChild) el.insertBefore(shell.firstChild, shell);
    shell.remove();
  });
  el.removeAttribute(MARK_ATTR);
  el.classList.remove("tt-paired", "tt-working");
  if (lastHovered === el) lastHovered = null; // 允许马上再翻一次
}

function restorePage() {
  document.querySelectorAll(`[${MARK_ATTR}]`).forEach(removeTranslationFrom);
  // 兜底：清掉任何残留（比如翻译中途被打断的）
  document.querySelectorAll(`.${TRANSLATION_CLASS}, .${BADGE_CLASS}`).forEach((n) => n.remove());
  document.querySelectorAll(`.${ORIGINAL_CLASS}`).forEach((shell) => {
    const parent = shell.parentNode;
    if (!parent) return;
    while (shell.firstChild) parent.insertBefore(shell.firstChild, shell);
    shell.remove();
  });
  document.body.classList.remove("tt-hide-original");
  pageTranslated = false;
  display = "original";
  observer?.disconnect();
  observer = null;
}

/** 整页后按「双语 → 仅译文 → 原文」轮转 */
function cyclePage() {
  if (!pageTranslated) {
    void translatePage();
    return;
  }
  const next: DisplayMode = display === "bilingual" ? "translation_only" : display === "translation_only" ? "original" : "bilingual";
  setDisplay(next);
  toast(next === "bilingual" ? "双语对照" : next === "translation_only" ? "仅译文" : "原文（再按一次回到双语）");
}

// 动态加载的内容增量翻译
let observer: MutationObserver | null = null;
let pending: Set<HTMLElement> = new Set();
let flushTimer: number | null = null;

function observeNewContent() {
  observer?.disconnect();
  observer = new MutationObserver((records) => {
    if (!pageTranslated) return;
    for (const r of records) {
      for (const n of Array.from(r.addedNodes)) {
        if (n.nodeType !== Node.ELEMENT_NODE) continue;
        const el = n as HTMLElement;
        if (el.classList?.contains(TRANSLATION_CLASS)) continue;
        for (const b of collectBlocks(el.parentElement ?? el, (e) => getComputedStyle(e))) pending.add(b);
      }
    }
    if (pending.size > 0 && flushTimer === null) {
      flushTimer = window.setTimeout(flushPending, 800);
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });
}

async function flushPending() {
  flushTimer = null;
  const blocks = Array.from(pending).filter((b) => b.isConnected && !b.hasAttribute(MARK_ATTR));
  pending.clear();
  if (blocks.length === 0) return;
  try {
    const items = blocks.map((el, i) => {
      const isHtml = hasInlineMarkup(el);
      return { id: String(i), text: isHtml ? el.innerHTML : (el.textContent ?? ""), is_html: isHtml };
    });
    const results = await translate("full_page", items);
    for (const r of results) {
      const el = blocks[Number(r.id)];
      if (el && r.text !== undefined) attachTranslation(el, r.text, false);
    }
    setDisplay(display);
  } catch {
    /* 增量翻译失败不打扰用户 */
  }
}

// ---------------- 划词 ----------------

let panel: HTMLElement | null = null;
let panelPinned = false;
let icon: HTMLElement | null = null;

function removeIcon() {
  icon?.remove();
  icon = null;
}

function closePanel(force = false) {
  if (panelPinned && !force) return;
  panel?.remove();
  panel = null;
  panelPinned = false;
}

/** 卡片「复制」拿走的文本：词典卡是整理后的条目，普通卡是译文 */
let panelCopyText = "";
/** 卡片的触发点（页面坐标）。内容到手后卡片会变高，要按它重摆，否则底部被视口切掉 */
let panelAnchor = { x: 0, y: 0 };

function showPanel(x: number, y: number, title: string, original: string) {
  ensureStyle();
  closePanel(true);
  panelCopyText = "";
  panelAnchor = { x, y };
  const p = document.createElement("div");
  p.id = "tt-panel";
  // 头部与桌面端一致：场景名 · 来源，右侧 复制 / 钉住 / 关闭
  p.innerHTML = `<div class="tt-head"><span class="tt-title"></span><span class="tt-from">· 网页</span><button class="tt-copy" title="复制">⎘</button><button title="钉住">📍</button><button title="关闭">✕</button></div>
    <div class="tt-src"></div><div class="tt-body">翻译中…</div>`;
  (p.querySelector(".tt-title") as HTMLElement).textContent = title;
  (p.querySelector(".tt-src") as HTMLElement).textContent = original.length > 160 ? original.slice(0, 160) + "…" : original;
  const [copy, pin, close] = Array.from(p.querySelectorAll("button")) as HTMLButtonElement[];
  copy.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (!panelCopyText) return;
    try {
      await navigator.clipboard.writeText(panelCopyText);
      toast("已复制", 1200);
    } catch {
      toast("复制失败：这个页面不允许写剪贴板", 2200);
    }
  });
  pin.addEventListener("click", (e) => {
    e.stopPropagation();
    panelPinned = !panelPinned;
    pin.textContent = panelPinned ? "📌" : "📍";
    p.classList.toggle("tt-pinned", panelPinned);
  });
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    closePanel(true);
  });
  document.body.appendChild(p);
  panel = p;
  placePanel();
  return p;
}

/** 摆位：默认在触发点右下，越界翻到另一侧。内容变高后要再调一次 */
function placePanel() {
  if (!panel) return;
  const { x, y } = panelAnchor;
  const r = panel.getBoundingClientRect();
  const left = Math.max(window.scrollX + 8, Math.min(x, window.scrollX + window.innerWidth - r.width - 8));
  const below = y + 12;
  const top = below + r.height + 8 > window.scrollY + window.innerHeight ? y - r.height - 12 : below;
  panel.style.left = `${left}px`;
  panel.style.top = `${Math.max(window.scrollY + 8, top)}px`;
}

function el(tag: string, cls: string, text?: string): HTMLElement {
  const d = document.createElement(tag);
  d.className = cls;
  if (text !== undefined) d.textContent = text;
  return d;
}

/**
 * 画词典卡。版式与桌面端 `ui::draw_entry` 一一对应：
 * 词头 → 音标 → 按词性分行的释义（词性列定宽对齐）→ 词形 → 分隔线 → 例句与译文。
 * 每块都可能缺（模型不知道就整行省略），所以逐块判空，不连坐。
 * **一律 textContent，不拼 HTML**：内容来自模型输出，拼进 innerHTML 等于把页面交给它。
 */
function setPanelDict(e: DictEntry, original: string) {
  const body = panel?.querySelector(".tt-body") as HTMLElement | null;
  if (!body) return;
  // 词头本身就是原文，不再另起一行灰字——与桌面端一致
  panel?.querySelector(".tt-src")?.remove();
  body.textContent = "";
  const head = e.head || original.trim();
  if (head) body.appendChild(el("div", "tt-dict-head", head));
  if (e.phonetic) body.appendChild(el("div", "tt-dict-phon", e.phonetic));
  if (e.senses.length) {
    const box = el("div", "tt-dict-senses");
    for (const s of e.senses) {
      const row = el("div", "tt-dict-sense");
      row.appendChild(el("span", "tt-dict-pos", s.pos));
      row.appendChild(el("span", "tt-dict-text", s.text));
      box.appendChild(row);
    }
    body.appendChild(box);
  }
  if (e.forms) body.appendChild(el("div", "tt-dict-forms", e.forms));
  if (e.example) {
    const ex = el("div", "tt-dict-ex");
    ex.appendChild(el("div", "tt-dict-ex-en", e.example));
    if (e.exampleZh) ex.appendChild(el("div", "tt-dict-ex-zh", e.exampleZh));
    body.appendChild(ex);
  }
  panelCopyText = dictEntryToText(e);
  placePanel();
}

function setPanelBody(html: string, failed = false) {
  const body = panel?.querySelector(".tt-body") as HTMLElement | null;
  if (!body) return;
  body.innerHTML = failed ? "" : sanitize(html);
  if (failed) {
    body.textContent = html;
    body.style.color = "#e88";
  }
}

async function translateSelection(x: number, y: number) {
  const sel = window.getSelection();
  const text = (sel?.toString() ?? "").trim();
  if (!worthTranslating(text)) {
    toast("请先选中要翻译的文字");
    return;
  }
  // 与桌面端同一套判定（dict.ts 的 looksLikeWord 对应 Rust 的 looks_like_word）：
  // 取到单个词或短语给词典卡，句子给译文卡。标题也跟桌面端一样叫「取词释义」
  const word = looksLikeWord(text);
  showPanel(x, y, word ? "取词释义" : "划词翻译", text);
  try {
    const [r] = await translate(word ? "word" : "selection", [{ id: "s", text }]);
    if (r.text === undefined) {
      setPanelBody(r.error ?? "", true);
    } else {
      // 模型没按格式回（一条标记都没有）就退回普通译文，别给一张空词典卡
      const entry = word ? parseDictEntry(r.text) : null;
      if (entry) {
        setPanelDict(entry, text);
      } else {
        setPanelBody(r.text);
        panelCopyText = r.text;
      }
    }
  } catch (e) {
    setPanelBody(e instanceof Error ? e.message : String(e), true);
  }
  placePanel();
}

// ---------------- 悬停 ----------------

/**
 * 同一个动作在极短时间内只执行一次。
 * 只为了挡住「同一次按键经 chrome.commands 与页面监听各来一遍」（两者相差几十毫秒），
 * 窗口不能开大：人连按两次最快也要 150 ms 上下，开到 300 ms 会把用户的连按吞掉。
 */
const lastFired: Record<string, number> = {};
function once(action: string, ms = 120): boolean {
  const now = Date.now();
  if (now - (lastFired[action] ?? 0) < ms) return false;
  lastFired[action] = now;
  return true;
}

let hoverTimer: number | null = null;
let lastHovered: HTMLElement | null = null;
/** 鼠标最后停留的位置：按快捷键翻"当前这段"时要靠它找元素 */
let lastMouse = { x: 0, y: 0 };

function modifierDown(e: { altKey: boolean; ctrlKey: boolean; shiftKey: boolean }): boolean {
  switch (settings.hoverModifier) {
    case "alt":
      return e.altKey;
    case "ctrl":
      return e.ctrlKey;
    case "shift":
      return e.shiftKey;
    default:
      return false;
  }
}

/** 这次按下的是不是悬停用的那个修饰键 */
function isHoverModifierKey(e: KeyboardEvent): boolean {
  switch (settings.hoverModifier) {
    case "alt":
      return e.key === "Alt";
    case "ctrl":
      return e.key === "Control";
    case "shift":
      return e.key === "Shift";
    default:
      return false;
  }
}

function cancelHover() {
  if (hoverTimer !== null) {
    clearTimeout(hoverTimer);
    hoverTimer = null;
  }
}

/** 本次按住修饰键期间已经处理过的那一段：松开键才清空，所以「松开再按」= 再处理一次 */
let handledThisHold: Element | null = null;

/**
 * 安排一次悬停翻译。两条路都会走到这里：
 * 1. 按住修饰键的状态下移动鼠标；
 * 2. 鼠标已经停好了才按下修饰键——**这条最常用**，而它没有新的 mousemove 事件，
 *    早先只在 mousemove 里判定，导致「先把鼠标停在文字上再按 Alt」完全不触发。
 */
function scheduleHoverAt(x: number, y: number, force = false) {
  if (settings.hoverModifier === "none" || hostDisabled()) return;
  const target = document.elementFromPoint(x, y);
  // 已翻译的段落要认出来自己，不能交给 blockAt——它会跳过带标记的元素，
  // 于是往上找到更大的父块，结果是把整块又翻一遍（用户看到的就是「重复按会再翻译」）
  const done = target?.closest(`[${MARK_ATTR}]`) ?? null;
  const el = (done ?? blockAt(target, (t) => getComputedStyle(t))) as HTMLElement | null;
  if (!el) return;
  // 同一次按住期间，同一段只处理一次；松开修饰键再按才会再处理（这就是「再按一次撤销」）
  if (el === handledThisHold && !force) return;
  if (el === lastHovered && !force) return;
  lastHovered = el;
  cancelHover();
  hoverTimer = window.setTimeout(() => {
    hoverTimer = null;
    if (lastHovered !== el) return;
    handledThisHold = el;
    if (el.hasAttribute(MARK_ATTR)) {
      removeTranslationFrom(el);
      toast("已撤掉这一段的译文", 1400);
    } else {
      void translateHover(el);
    }
  }, settings.hoverDelayMs);
}

/**
 * 翻译单个段落，原文留在原处、译文插在紧下方——中英对照。
 * 文字直接从 DOM 取（innerHTML 或 textContent），不做任何图像识别。
 */
async function translateHover(el: HTMLElement) {
  if (el.hasAttribute(MARK_ATTR)) return;
  const isHtml = hasInlineMarkup(el);
  const text = isHtml ? el.innerHTML : (el.textContent ?? "");
  el.setAttribute(MARK_ATTR, "1");
  el.classList.add("tt-working"); // 翻译中给个淡淡的高亮，知道点中的是哪段
  try {
    const [r] = await translate("hover", [{ id: "h", text, is_html: isHtml }]);
    if (r.text !== undefined) {
      attachTranslation(el, r.text, false);
      // 单段翻译一律对照显示：整页切过「仅译文」时 body 上有 tt-hide-original，
      // 这里打个标记把这一段的原文豁免掉，否则用户看到的是原文消失、只剩译文
      el.classList.add("tt-paired");
    } else {
      el.removeAttribute(MARK_ATTR);
      toast(r.error ?? "翻译失败");
    }
  } catch (e) {
    el.removeAttribute(MARK_ATTR);
    toast(e instanceof Error ? e.message : String(e));
  } finally {
    el.classList.remove("tt-working");
  }
}

// ---------------- 事件接线 ----------------

function hostDisabled(): boolean {
  return settings.disabledHosts.some((h) => location.hostname.endsWith(h));
}

function install() {
  ensureStyle();

  const opts = { signal: abort.signal };

  document.addEventListener("mouseup", (e) => {
    if (!settings.selectionIcon || hostDisabled()) return;
    // 点「译」图标（或卡片里的按钮）时 mouseup 也会冒泡到这里。图标的 mousedown 为了保住选区
    // 做了 preventDefault，所以这时选区还在——不挡的话会在点击处立刻再生成一个图标，
    // 压在刚弹出的卡片左上角 2.5 秒才消失。
    const t = e.target as Element | null;
    if (t?.closest?.("#tt-icon, #tt-panel")) return;
    setTimeout(() => {
      const text = (window.getSelection()?.toString() ?? "").trim();
      removeIcon();
      if (!worthTranslating(text)) return;
      const b = document.createElement("button");
      b.id = "tt-icon";
      b.textContent = "译";
      b.style.left = `${e.pageX + 6}px`;
      b.style.top = `${e.pageY + 6}px`;
      b.addEventListener("mousedown", (ev) => ev.preventDefault());
      b.addEventListener("click", (ev) => {
        ev.stopPropagation();
        const { pageX, pageY } = ev;
        removeIcon();
        void translateSelection(pageX, pageY);
      });
      document.body.appendChild(b);
      icon = b;
      setTimeout(() => {
        if (icon === b) removeIcon();
      }, 2500);
    }, 10);
  }, opts);

  document.addEventListener("mousedown", (e) => {
    if (icon && e.target !== icon) removeIcon();
    if (panel && !panel.contains(e.target as Node)) closePanel();
  }, opts);

  document.addEventListener(
    "mousemove",
    (e) => {
      lastMouse = { x: e.clientX, y: e.clientY };
    },
    { passive: true, signal: abort.signal },
  );

  document.addEventListener("mousemove", (e) => {
    if (!modifierDown(e)) {
      cancelHover();
      lastHovered = null;
      return;
    }
    scheduleHoverAt(e.clientX, e.clientY);
  }, opts);

  document.addEventListener("keyup", (e) => {
    if (isHoverModifierKey(e)) {
      cancelHover();
      lastHovered = null;
      handledThisHold = null; // 松开了；下次按住可以对同一段再操作一次（翻 ↔ 撤）
    }
  }, opts);

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closePanel(true);
      removeIcon();
      return;
    }
    // 鼠标已经停在某段上、这时才按下修饰键：没有 mousemove 事件，得在这里起判定
    if (!e.repeat && isHoverModifierKey(e)) {
      lastHovered = null; // 允许对同一段重新计时
      scheduleHoverAt(lastMouse.x, lastMouse.y, true);
    }
    // 页面里自己认快捷键。不能只靠 chrome.commands：它最多分配 4 个，
    // 且与浏览器内置键冲突时静默不给（Alt+Shift+T 就永远拿不到）
    if (isTyping(e.target)) return;
    if (matchesHotkey(e, settings.keySelection)) {
      e.preventDefault();
      if (once("selection")) {
        const sel = window.getSelection();
        const r = sel?.rangeCount ? sel.getRangeAt(0).getBoundingClientRect() : null;
        void translateSelection((r?.left ?? 40) + window.scrollX, (r?.bottom ?? 40) + window.scrollY);
      }
    } else if (matchesHotkey(e, settings.keyFullPage)) {
      e.preventDefault();
      if (once("page")) cyclePage();
    }
  }, opts);

  chrome.runtime.onMessage.addListener((msg: PageCommand, _s, sendResponse) => {
    switch (msg.kind) {
      case "status":
        // popup 靠这个判断页面能不能注入、当前是不是已翻译
        sendResponse({
          translated: pageTranslated,
          display,
          blocks: document.querySelectorAll(`.${TRANSLATION_CLASS}`).length,
          host: location.hostname,
        } satisfies PageStatus);
        return false;
      case "translate_page":
        if (!pageTranslated) void translatePage();
        break;
      case "toggle_page":
        if (once("page")) cyclePage();
        break;
      case "restore_page":
        restorePage();
        break;
      case "set_display":
        if (pageTranslated) setDisplay(msg.mode);
        break;
      case "translate_selection": {
        if (!once("selection")) break;
        const sel = window.getSelection();
        const r = sel?.rangeCount ? sel.getRangeAt(0).getBoundingClientRect() : null;
        void translateSelection((r?.left ?? 40) + window.scrollX, (r?.bottom ?? 40) + window.scrollY);
        break;
      }
    }
    sendResponse({ ok: true });
    return false;
  });

  // popup 里改了开关就立刻生效，不用刷新页面
  chrome.storage.onChanged.addListener((changes, area) => {
    if (area !== "sync") return;
    for (const [k, v] of Object.entries(changes)) {
      (settings as unknown as Record<string, unknown>)[k] = v.newValue;
    }
    // 站点被停用时，把已经弹出来的图标与面板收掉
    if (hostDisabled()) {
      removeIcon();
      closePanel(true);
    }
  });
}

const INJECTED_FLAG = "__ttInjected";

async function boot() {
  // 扩展更新后 background 会给已打开的页面重新注入这份脚本；
  // 老的那份上下文已失效（它自己会 giveUp 摘监听），但要挡住同一份被注入两次
  const w = window as unknown as Record<string, unknown>;
  if (w[INJECTED_FLAG]) return;
  w[INJECTED_FLAG] = true;

  const r = await send({ kind: "settings" });
  if (r.ok && "settings" in r) settings = r.settings;
  install();
}

void boot();

// 供测试引用
export { cyclePage, restorePage, translatePage };
