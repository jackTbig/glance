/** 插件各部分共用的类型、默认设置与纯函数（纯函数便于在 Node 里单测） */

export const DEFAULT_PORT = 42731;

export type Scene = "full_page" | "selection" | "hover" | "screenshot";

/** 页面显示模式 */
export type DisplayMode = "bilingual" | "translation_only" | "original";

export interface Settings {
  port: number;
  /** 悬停修饰键；none = 关闭悬停 */
  hoverModifier: "alt" | "ctrl" | "shift" | "none";
  hoverDelayMs: number;
  /** 选中文字后弹小图标 */
  selectionIcon: boolean;
  /** 整页翻译默认显示模式 */
  defaultDisplay: DisplayMode;
  /** 这些站点不做任何自动行为（悬停、图标） */
  disabledHosts: string[];
}

export const DEFAULT_SETTINGS: Settings = {
  port: DEFAULT_PORT,
  hoverModifier: "alt",
  hoverDelayMs: 300,
  selectionIcon: true,
  defaultDisplay: "bilingual",
  disabledHosts: [],
};

/** 桌面端 /v1/health 的回包 */
export interface Health {
  version: string;
  backend?: string;
  model?: string;
  concurrency: number;
  batch_size: number;
}

/** popup 问 content script 要的当前页状态 */
export interface PageStatus {
  translated: boolean;
  display: DisplayMode;
  /** 已翻译的段落数 */
  blocks: number;
  /** 页面自己报的域名。不靠 tabs 权限读 tab.url：`activeTab` 只在用户点图标那一下授予，
   *  别的入口（比如直接打开 popup.html）读到的 url 是空的。 */
  host: string;
}

export interface TranslateItem {
  id: string;
  text: string;
  is_html?: boolean;
}

export interface TranslateResult {
  id: string;
  text?: string;
  error?: string;
}

/** background ← content 的消息 */
export type Message =
  | { kind: "translate"; scene: Scene; items: TranslateItem[]; context?: string }
  | { kind: "health" }
  | { kind: "settings" };

/** popup / background → content script */
export type PageCommand =
  | { kind: "status" }
  | { kind: "translate_page" }
  | { kind: "restore_page" }
  | { kind: "toggle_page" }
  | { kind: "translate_selection" }
  | { kind: "translate_block" }
  | { kind: "set_display"; mode: DisplayMode };

export type Reply =
  | { ok: true; results: TranslateResult[] }
  | { ok: true; health: Health }
  | { ok: true; settings: Settings }
  | { ok: false; error: string };

/** 服务未运行时给用户看的提示 */
export const SERVICE_DOWN =
  "翻译工具未运行。请先启动桌面端 translate-tool，再刷新页面重试。";

/** 这些标签里的文字不翻译 */
const SKIP_TAGS = new Set([
  "SCRIPT", "STYLE", "NOSCRIPT", "CODE", "PRE", "KBD", "SAMP", "VAR",
  "TEXTAREA", "INPUT", "SELECT", "OPTION", "SVG", "MATH", "CANVAS", "VIDEO", "AUDIO",
]);

/** 可以整块拿来翻译的块级标签 */
const BLOCK_TAGS = new Set([
  "P", "LI", "DD", "DT", "TD", "TH", "BLOCKQUOTE", "FIGCAPTION", "SUMMARY",
  "H1", "H2", "H3", "H4", "H5", "H6", "CAPTION", "LABEL", "ARTICLE", "SECTION", "DIV",
]);

export const TRANSLATION_CLASS = "tt-translation";
export const MARK_ATTR = "data-tt-done";

/** 汉字占比过半就当作中文，不必翻译 */
export function isMostlyChinese(text: string): boolean {
  let han = 0;
  let total = 0;
  for (const c of text) {
    if (/[\p{L}\p{N}]/u.test(c)) {
      total++;
      if (/[一-鿿㐀-䶿]/u.test(c)) han++;
    }
  }
  return total > 0 && han * 2 > total;
}

/** 值得翻译：至少两个字符且含字母或汉字 */
export function worthTranslating(text: string): boolean {
  const t = text.trim();
  return t.replace(/\s/g, "").length >= 2 && /[\p{L}]/u.test(t) && !isMostlyChinese(t);
}

function isSkipped(el: Element): boolean {
  if (SKIP_TAGS.has(el.tagName)) return true;
  if (el.classList.contains(TRANSLATION_CLASS)) return true;
  if (el.hasAttribute(MARK_ATTR)) return true;
  if (el.closest(`[${MARK_ATTR}]`) !== null) return true;
  const he = el as HTMLElement;
  if (he.isContentEditable) return true;
  return false;
}

/** 元素是否可见（隐藏内容不翻，省钱也省得译文错位） */
export function isVisible(el: Element, getStyle: (e: Element) => CSSStyleDeclaration): boolean {
  const s = getStyle(el);
  if (s.display === "none" || s.visibility === "hidden" || s.opacity === "0") return false;
  const r = (el as HTMLElement).getBoundingClientRect?.();
  if (r && r.width === 0 && r.height === 0) return false;
  return true;
}

/**
 * 挑出页面里要翻译的块。规则：
 * 只取“自身直接含有文本、且不再含有其它块级子元素”的元素，避免父子重复翻译。
 */
export function collectBlocks(root: ParentNode, getStyle: (e: Element) => CSSStyleDeclaration): HTMLElement[] {
  const out: HTMLElement[] = [];
  const walk = (el: Element) => {
    if (isSkipped(el)) return;
    if (!isVisible(el, getStyle)) return;
    const hasBlockChild = Array.from(el.children).some((c) => BLOCK_TAGS.has(c.tagName) && !isSkipped(c));
    if (!hasBlockChild && BLOCK_TAGS.has(el.tagName)) {
      const text = (el.textContent ?? "").trim();
      if (worthTranslating(text) && text.length <= 4000) {
        out.push(el as HTMLElement);
        return;
      }
    }
    for (const c of Array.from(el.children)) walk(c);
  };
  for (const c of Array.from(root.children ?? [])) walk(c);
  return out;
}

/** 元素内部是否有行内标签，有的话要按 HTML 送翻以保留链接与格式 */
export function hasInlineMarkup(el: Element): boolean {
  return el.children.length > 0;
}

/** 找鼠标下最近的可翻译块 */
export function blockAt(target: Element | null, getStyle: (e: Element) => CSSStyleDeclaration): HTMLElement | null {
  let el: Element | null = target;
  while (el && el !== document.body) {
    if (BLOCK_TAGS.has(el.tagName) && !isSkipped(el) && isVisible(el, getStyle)) {
      const t = (el.textContent ?? "").trim();
      if (worthTranslating(t) && t.length <= 4000) return el as HTMLElement;
    }
    el = el.parentElement;
  }
  return null;
}
