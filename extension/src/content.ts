/** 页面里的整页翻译、悬停翻译、划词翻译 */
import {
  blockAt,
  collectBlocks,
  DEFAULT_SETTINGS,
  hasInlineMarkup,
  MARK_ATTR,
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
.tt-hide-original > *:not(.${TRANSLATION_CLASS}) { display: none !important; }
.tt-hide-original { }
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

function send(msg: unknown): Promise<Reply> {
  return new Promise((resolve) => {
    chrome.runtime.sendMessage(msg, (r: Reply) => {
      if (chrome.runtime.lastError) {
        resolve({ ok: false, error: chrome.runtime.lastError.message ?? "扩展通信失败" });
      } else {
        resolve(r);
      }
    });
  });
}

async function translate(scene: Scene, items: TranslateItem[], context?: string): Promise<TranslateResult[]> {
  const r = await send({ kind: "translate", scene, items, context });
  if (!r.ok) throw new Error(r.error);
  return (r as { ok: true; results: TranslateResult[] }).results;
}

// ---------------- 整页翻译 ----------------

function attachTranslation(el: HTMLElement, text: string, failed: boolean, original?: string) {
  el.querySelectorAll(`:scope > .${TRANSLATION_CLASS}`).forEach((n) => n.remove());
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
  const root = document.body;
  root.classList.toggle("tt-hide-original", mode === "translation_only");
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

function restorePage() {
  document.querySelectorAll(`.${TRANSLATION_CLASS}`).forEach((n) => n.remove());
  document.querySelectorAll(`[${MARK_ATTR}]`).forEach((n) => n.removeAttribute(MARK_ATTR));
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

let hoverTimer: number | null = null;
let lastHovered: HTMLElement | null = null;

function modifierDown(e: MouseEvent): boolean {
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

async function translateHover(el: HTMLElement) {
  if (el.hasAttribute(MARK_ATTR)) return;
  const isHtml = hasInlineMarkup(el);
  const text = isHtml ? el.innerHTML : (el.textContent ?? "");
  el.setAttribute(MARK_ATTR, "1");
  try {
    const [r] = await translate("hover", [{ id: "h", text, is_html: isHtml }]);
    if (r.text !== undefined) {
      attachTranslation(el, r.text, false);
    } else {
      el.removeAttribute(MARK_ATTR);
      toast(r.error ?? "翻译失败");
    }
  } catch (e) {
    el.removeAttribute(MARK_ATTR);
    toast(e instanceof Error ? e.message : String(e));
  }
}

// ---------------- 事件接线 ----------------

function hostDisabled(): boolean {
  return settings.disabledHosts.some((h) => location.hostname.endsWith(h));
}

function install() {
  ensureStyle();

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
  });

  document.addEventListener("mousedown", (e) => {
    if (icon && e.target !== icon) removeIcon();
    if (panel && !panel.contains(e.target as Node)) closePanel();
  });

  document.addEventListener("mousemove", (e) => {
    if (settings.hoverModifier === "none" || hostDisabled()) return;
    if (!modifierDown(e)) {
      if (hoverTimer !== null) {
        clearTimeout(hoverTimer);
        hoverTimer = null;
      }
      return;
    }
    const el = blockAt(e.target as Element, (x) => getComputedStyle(x));
    if (!el || el === lastHovered) return;
    lastHovered = el;
    if (hoverTimer !== null) clearTimeout(hoverTimer);
    hoverTimer = window.setTimeout(() => {
      hoverTimer = null;
      if (lastHovered === el) void translateHover(el);
    }, settings.hoverDelayMs);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closePanel(true);
      removeIcon();
    }
  });

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
        cyclePage();
        break;
      case "restore_page":
        restorePage();
        break;
      case "set_display":
        if (pageTranslated) setDisplay(msg.mode);
        break;
      case "translate_selection": {
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

async function boot() {
  const r = await send({ kind: "settings" });
  if (r.ok && "settings" in r) settings = r.settings;
  install();
}

void boot();

// 供测试引用
export { cyclePage, restorePage, translatePage };
