/** 页面里的整页翻译、悬停翻译、划词翻译 */
import {
  blockAt,
  collectBlocks,
  DEFAULT_SETTINGS,
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
.${TRANSLATION_CLASS}.tt-failed { border-left-color: rgba(230,110,110,.8); opacity: .85; cursor: pointer; }
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
#tt-panel {
  position: absolute; z-index: 2147483647; max-width: 380px; min-width: 220px;
  background: rgba(28,28,32,.97); color: #eee; border-radius: 10px; padding: 10px 12px;
  font: 14px/1.6 system-ui, -apple-system, "Segoe UI", sans-serif;
  box-shadow: 0 6px 24px rgba(0,0,0,.4);
}
#tt-panel .tt-head { display: flex; gap: 8px; align-items: center; font-size: 12px; color: #9ab; margin-bottom: 6px; }
#tt-panel .tt-head button { margin-left: auto; background: none; border: none; color: #9ab; cursor: pointer; font-size: 14px; }
#tt-panel .tt-src { color: #999; font-size: 12px; margin-bottom: 6px; }
#tt-panel.tt-pinned { border: 1px solid rgba(120,170,255,.5); }
`;

function ensureStyle() {
  if (document.getElementById(styleId)) return;
  const s = document.createElement("style");
  s.id = styleId;
  s.textContent = CSS;
  document.documentElement.appendChild(s);
}

function toast(msg: string, ms = 3200) {
  ensureStyle();
  document.getElementById("tt-toast")?.remove();
  const d = document.createElement("div");
  d.id = "tt-toast";
  d.textContent = msg;
  document.body.appendChild(d);
  setTimeout(() => d.remove(), ms);
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

function attachTranslation(el: HTMLElement, text: string, failed: boolean, original?: string) {
  el.querySelectorAll(`:scope > .${TRANSLATION_CLASS}`).forEach((n) => n.remove());
  wrapOriginal(el);
  const d = document.createElement("div");
  d.className = TRANSLATION_CLASS + (failed ? " tt-failed" : "");
  if (failed) {
    d.textContent = `翻译失败：${text}（点这里重试）`;
    d.title = "点击重试";
    d.addEventListener("click", async (e) => {
      e.stopPropagation();
      d.textContent = "重试中…";
      try {
        const src = original ?? el.textContent ?? "";
        const [r] = await translate("full_page", [{ id: "retry", text: src, is_html: false }]);
        attachTranslation(el, r.text ?? r.error ?? "", !r.text);
      } catch (err) {
        d.textContent = `翻译失败：${err instanceof Error ? err.message : String(err)}（点这里重试）`;
      }
    });
  } else {
    // 译文可能带行内标签，用 innerHTML 但先去掉脚本类节点
    d.innerHTML = sanitize(text);
  }
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

async function translatePage() {
  if (busy) return;
  busy = true;
  try {
    const blocks = collectBlocks(document.body, (e) => getComputedStyle(e));
    if (blocks.length === 0) {
      toast("这个页面没有找到可翻译的段落");
      return;
    }
    toast(`正在翻译 ${blocks.length} 个段落…`, 60_000);
    const items: TranslateItem[] = blocks.map((el, i) => {
      const isHtml = hasInlineMarkup(el);
      return { id: String(i), text: isHtml ? el.innerHTML : (el.textContent ?? ""), is_html: isHtml };
    });
    const results = await translate("full_page", items);
    let failed = 0;
    for (const r of results) {
      const el = blocks[Number(r.id)];
      if (!el) continue;
      if (r.text !== undefined) {
        attachTranslation(el, r.text, false);
      } else {
        failed++;
        attachTranslation(el, r.error ?? "未知错误", true, items[Number(r.id)].text);
      }
    }
    pageTranslated = true;
    setDisplay(settings.defaultDisplay);
    document.getElementById("tt-toast")?.remove();
    toast(failed === 0 ? `完成，共 ${results.length} 段` : `完成 ${results.length} 段，其中 ${failed} 段失败（点译文可重试）`);
    observeNewContent();
  } catch (e) {
    document.getElementById("tt-toast")?.remove();
    toast(e instanceof Error ? e.message : String(e), 6000);
  } finally {
    busy = false;
  }
}

/** 撤掉一个段落的译文，把它还原成翻译前的样子 */
function removeTranslationFrom(el: Element) {
  el.querySelectorAll(`:scope > .${TRANSLATION_CLASS}`).forEach((n) => n.remove());
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
  document.querySelectorAll(`.${TRANSLATION_CLASS}`).forEach((n) => n.remove());
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

function showPanel(x: number, y: number, title: string, original: string) {
  ensureStyle();
  closePanel(true);
  const p = document.createElement("div");
  p.id = "tt-panel";
  p.innerHTML = `<div class="tt-head"><span></span><button title="钉住">📍</button><button title="关闭">✕</button></div>
    <div class="tt-src"></div><div class="tt-body">翻译中…</div>`;
  (p.querySelector(".tt-head span") as HTMLElement).textContent = title;
  (p.querySelector(".tt-src") as HTMLElement).textContent = original.length > 160 ? original.slice(0, 160) + "…" : original;
  const [pin, close] = Array.from(p.querySelectorAll("button")) as HTMLButtonElement[];
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
  // 摆位：默认右下，越界翻到另一侧
  const r = p.getBoundingClientRect();
  const left = Math.max(8, Math.min(x, window.scrollX + window.innerWidth - r.width - 8));
  const top = y + r.height + 8 > window.scrollY + window.innerHeight ? y - r.height - 12 : y + 12;
  p.style.left = `${left}px`;
  p.style.top = `${Math.max(8, top)}px`;
  panel = p;
  return p;
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
  showPanel(x, y, "划词翻译", text);
  try {
    const [r] = await translate("selection", [{ id: "s", text }]);
    setPanelBody(r.text ?? r.error ?? "", r.text === undefined);
  } catch (e) {
    setPanelBody(e instanceof Error ? e.message : String(e), true);
  }
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
