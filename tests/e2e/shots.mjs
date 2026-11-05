/**
 * 拍商店上架用的截图（1280×800）。
 *
 * 用一个临时的小假接口（OpenAI 兼容）当真模型，页面自带的英文句子都能译成中文，
 * 免得截图里出现 "zh=..." 这种假后端的标记。
 *
 *   node shots.mjs
 */
import { chromium } from "@playwright/test";
import { createServer } from "node:http";
import { mkdirSync } from "node:fs";

const OUT = "D:/My_data/Claude/translate-tool/dist/chrome-store";
const PORT = 41777;
const EXT = "D:/My_data/Claude/translate-tool/extension/dist";
const ID = "bhngnlkgffjimhcmmhfkgcgbohdecoch";
mkdirSync(OUT, { recursive: true });

const DICT = {
  "Rust is a systems programming language that runs blazingly fast and prevents segfaults.":
    "Rust 是一门系统编程语言，运行速度极快，并且能避免段错误。",
  "Memory safety without garbage collection.": "不用垃圾回收，也有内存安全。",
  "Concurrency without data races.": "没有数据竞争，也能做并发。",
  "The bound is tight even in the worst case.": "即使在最坏的情况下，这个界也是紧的。",
  "Ownership is a set of rules the compiler checks at compile time.":
    "所有权是一组规则，编译器会在编译期逐条检查。",
  "Zero-cost abstractions let you write high-level code without paying for it at runtime.":
    "零成本抽象让你写高层代码，却不用在运行时为此付出代价。",
  "A tour of Rust": "Rust 概览",
  "6 min read · Programming": "阅读约 6 分钟 · 编程",
  "glance": "一瞥",
  "translation": "翻译",
};

const PAGE = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>A tour of Rust</title>
<style>
  body { font: 16px/1.75 -apple-system, "Segoe UI", system-ui, sans-serif; color: #1f2328;
         margin: 0; padding: 48px 64px; max-width: 980px; }
  h1 { font-size: 30px; margin: 0 0 6px; letter-spacing: -.2px; }
  .meta { color: #656d76; font-size: 14px; margin-bottom: 28px; }
  p { margin: 0 0 18px; }
  a { color: #0969da; }
</style></head>
<body>
  <h1>A tour of Rust</h1>
  <div class="meta">6 min read · Programming</div>
  <p>Rust is a systems programming language that runs blazingly fast and prevents segfaults.</p>
  <p>Ownership is a set of rules the compiler checks at compile time.</p>
  <p>Zero-cost abstractions let you write high-level code without paying for it at runtime.</p>
  <ul>
    <li>Memory safety without garbage collection.</li>
    <li>Concurrency without data races.</li>
  </ul>
</body></html>`;

function translate(text) {
  const t = text.trim();
  if (DICT[t]) return DICT[t];
  return "（此处为演示译文）" + t;
}

/** 假接口：OpenAI 兼容的 /v1/chat/completions */
const api = createServer((req, res) => {
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const j = JSON.parse(body || "{}");
    const user = (j.messages ?? []).filter((m) => m.role === "user").map((m) => m.content).join("\n");
    let out;
    if (user.includes("词典条目")) {
      const word = user.split("\n").filter((l) => l.trim() && !l.trim().endsWith("：")).pop().trim();
      out = [
        `#词 ${word}`,
        "#音 英 /ɡlɑːns/ 美 /ɡlæns/",
        "#释 v. 一瞥，看一眼；浏览",
        "#释 n. 一瞥；闪光",
        "#形 复数 glances；过去式 glanced",
        "#例 She glanced at her watch.",
        "#译 她瞥了一眼手表。",
      ].join("\n");
    } else {
      out = user
        .split("\n")
        .filter((l) => l.trim() && !l.trim().endsWith("："))
        .map((l) => {
          const m = /^⟨\s*(\d+)\s*⟩\s*(.*)$/.exec(l);
          return m ? `⟨${m[1]}⟩ ${translate(m[2])}` : translate(l);
        })
        .join("\n");
    }
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ choices: [{ message: { content: out } }] }));
  });
});

const site = createServer((req, res) => {
  res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
  res.end(PAGE);
});

await new Promise((r) => api.listen(PORT, "127.0.0.1", r));
await new Promise((r) => site.listen(PORT + 1, "127.0.0.1", r));

const ctx = await chromium.launchPersistentContext("", {
  headless: false,
  viewport: { width: 1280, height: 800 },
  args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`],
});

try {
  const sw = ctx.serviceWorkers().length ? ctx.serviceWorkers()[0] : await ctx.waitForEvent("serviceworker");
  // 走直连模式，接口指向本地假服务
  await sw.evaluate(async (port) => {
    await chrome.storage.sync.set({
      backend: "direct",
      apiBase: `http://127.0.0.1:${port}/v1`,
      apiModel: "deepseek-flash",
      defaultDisplay: "bilingual",
    });
    await chrome.storage.local.set({ apiKey: "demo-key" });
  }, PORT);

  // 1) 整页双语对照
  const page = await ctx.newPage();
  await page.goto(`http://127.0.0.1:${PORT + 1}/`);
  await page.waitForLoadState("domcontentloaded");
  await sw.evaluate(async () => {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    // content script 可能还没注入完，重试到能通
    for (let i = 0; i < 30; i++) {
      try {
        await chrome.tabs.sendMessage(tab.id, { kind: "toggle_page" });
        return;
      } catch {
        await new Promise((r) => setTimeout(r, 200));
      }
    }
    throw new Error("content script 一直没注入上");
  });
  await page.waitForSelector(".tt-translation", { timeout: 20000 });
  await page.waitForTimeout(5200); // 等提示条自己收掉
  await page.screenshot({ path: `${OUT}/screenshot-1-page.png` });

  // 2) 划词：选中一个词弹词典卡
  await page.evaluate(() => {
    const el = document.querySelector("p");
    // 翻译后原文被包进 .tt-original 壳，文字节点在里面
    const shell = el.querySelector(".tt-original") ?? el;
    const node = [...shell.childNodes].find((n) => n.nodeType === 3 && n.data.includes("Rust"));
    const at = node.data.indexOf("Rust");
    const r = document.createRange();
    r.setStart(node, at);
    r.setEnd(node, at + 4);
    const s = window.getSelection();
    s.removeAllRanges();
    s.addRange(r);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 120, clientY: 100 }));
  });
  await page.waitForTimeout(600);
  const icon = page.locator("#tt-icon");
  if (await icon.count()) {
    await icon.click({ timeout: 5000 }).catch(() => {});
    await page.waitForTimeout(1500);
  }
  await page.screenshot({ path: `${OUT}/screenshot-2-word.png` });
  await page.close();

  // 3) 设置页（直连模式）
  const opt = await ctx.newPage();
  await opt.setViewportSize({ width: 1280, height: 800 });
  await opt.goto(`chrome-extension://${ID}/options.html`);
  await opt.waitForTimeout(700);
  await opt.selectOption("#backend", "direct");
  await opt.waitForTimeout(300);
  // 填成一份像样的真实配置，别把演示用的本地地址拍进去
  await opt.fill("#apiBase", "https://api.deepseek.com/v1");
  await opt.fill("#apiModel", "deepseek-flash");
  await opt.fill("#apiKey", "sk-****************");
  await opt.waitForTimeout(200);
  await opt.screenshot({ path: `${OUT}/screenshot-3-settings.png` });
  await opt.close();

  // 4) 插件菜单
  const menu = await ctx.newPage();
  await menu.setViewportSize({ width: 420, height: 640 });
  await menu.goto(`chrome-extension://${ID}/popup.html`);
  await menu.waitForTimeout(1200);
  await menu.screenshot({ path: `${OUT}/screenshot-4-menu.png` });
  await menu.close();

  console.log("截图完成 →", OUT);
} finally {
  await ctx.close();
  api.close();
  site.close();
}
