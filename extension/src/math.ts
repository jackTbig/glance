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

/// 在 root 内就地渲染公式。**失败保留原文**：公式写坏不能吃整段译文。
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
