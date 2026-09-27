//! 公式渲染：译文里的 LaTeX（\(…\)、\[…\]、$$…$$）用 KaTeX 渲染成公式（Jack 2026-09-27）。
//! 只在插件侧做：桌面端卡片不渲染公式。KaTeX 与字体随扩展打包，不联网。

import renderMathInElement from "katex/contrib/auto-render";

/// 是否含公式定界符。不含就完全不碰 KaTeX，普通译文零开销
export function hasMath(text: string): boolean {
  return /\$\$|\\\(|\\\[/.test(text);
}

let cssReady = false;

/// KaTeX 的样式与字体走 web_accessible_resources，按 URL 注入一次
function ensureCss(): void {
  if (cssReady || document.getElementById("tt-katex-css")) {
    cssReady = true;
    return;
  }
  const link = document.createElement("link");
  link.id = "tt-katex-css";
  link.rel = "stylesheet";
  link.href = chrome.runtime.getURL("katex/katex.min.css");
  (document.head ?? document.documentElement).appendChild(link);
  cssReady = true;
}

/// 送翻前把源页里**已渲染的公式节点**换成占位符，译文回来后原样回填
/// （Jack 2026-09-27：模型会把公式抹平成 2^{h-1} 这种纯文本，没法渲染；源页的
/// KaTeX/MathJax/MathML 节点本身就是排好的，直接用比重新渲染可靠）。
/// 占位符是纯文本 `[[Mn]]`，sanitize 不会动它；回填放在 sanitize 之后，
/// 因为 sanitize 会剥掉 class，katex 的 span 被剥了就只剩一堆碎字。
export function extractMath(html: string): { text: string; parts: Record<string, string> } {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  const sel = "span.katex-display, span.katex, mjx-container, math";
  const nodes: Element[] = Array.from(tpl.content.querySelectorAll(sel)).filter((n) => !n.parentElement?.closest(sel));
  // 公式图片（老站点拿 png/svg 当公式排）：按 class/src/alt 的公式特征认，别误伤图标 logo
  for (const img of Array.from(tpl.content.querySelectorAll("img"))) {
    if (img.parentElement?.closest(sel) || nodes.includes(img)) continue;
    if (isMathImg(img)) nodes.push(img);
  }
  const parts: Record<string, string> = {};
  nodes.forEach((n, i) => {
    const tok = "[[M" + (i + 1) + "]]";
    parts[tok] = n.outerHTML;
    n.replaceWith(document.createTextNode(tok));
  });
  return { text: tpl.innerHTML, parts };
}

/// 这张图是不是公式：class/src 带公式服务特征，或 alt/title 里是 TeX 源码（含 ^ _ 反斜杠）
function isMathImg(img: Element): boolean {
  const alt = (img.getAttribute("alt") ?? "") + " " + (img.getAttribute("title") ?? "");
  const cls = img.getAttribute("class") ?? "";
  const src = img.getAttribute("src") ?? "";
  if (/\b(formula|math|latex|equation|eq)\b/i.test(cls)) return true;
  if (/(latex|codecogs|mathtex|formula|equation|eqn)/i.test(src)) return true;
  return /[\\^_]|\$\$/.test(alt) && alt.trim().length > 1;
}

/// 把占位符换回源页公式节点的原 HTML。令牌丢了（模型没保留）就少一处公式，不报错
export function restoreMath(text: string, parts: Record<string, string>): string {
  let out = text;
  for (const tok of Object.keys(parts)) {
    out = out.split(tok).join(parts[tok]);
  }
  return out;
}

/// 在 root 内就地渲染。**失败保留原文**：公式写坏不能吃整段译文。
/// 不认单个 $…$：正文里的美元金额太常见，误渲染比不渲染糟。
export function renderMath(root: HTMLElement): void {
  ensureCss();
  try {
    renderMathInElement(root, {
      delimiters: [
        { left: "$$", right: "$$", display: true },
        { left: "\\[", right: "\\]", display: true },
        { left: "\\(", right: "\\)", display: false },
      ],
      throwOnError: false,
    });
  } catch (e) {
    console.warn("[简译] 公式渲染失败，保留原文：", e);
  }
}
