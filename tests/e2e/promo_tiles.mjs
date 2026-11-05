/**
 * 生成商店要的两张图块：marquee 1400×560、small 440×280。
 *
 * 商店的规格：PNG 或 JPEG，画满不留边、边缘清楚，缩到一半还看得懂。
 * Playwright 截的是不透明页面，出来就是 24 位 RGB（无 alpha），符合要求。
 *
 *   node promo_tiles.mjs
 */
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const OUT = "D:/My_data/Claude/translate-tool/dist/chrome-store";
const ICON = readFileSync("D:/My_data/Claude/translate-tool/extension/public/icons/icon_128.png").toString("base64");
const ICON_URL = `data:image/png;base64,${ICON}`;

const CSS = `
  * { margin:0; padding:0; box-sizing:border-box; }
  body { font-family:-apple-system,"Segoe UI","Microsoft YaHei",system-ui,sans-serif; }
  .tile { position:relative; overflow:hidden; background:#0f1420; color:#eef2ff; }
  .glow { position:absolute; border-radius:50%; filter:blur(90px); opacity:.55; }
  .g1 { width:520px; height:520px; background:#2f6bff; left:-120px; top:-160px; }
  .g2 { width:460px; height:460px; background:#7b4bff; right:-120px; bottom:-190px; }
  .row { position:relative; display:flex; align-items:center; gap:28px; height:100%; }
  .logo { width:118px; height:118px; border-radius:26px; box-shadow:0 18px 48px rgba(0,0,0,.45); flex:none; }
  h1 { font-size:52px; font-weight:700; letter-spacing:-.5px; }
  p.tag { font-size:25px; color:#b9c6e8; margin-top:12px; }
  .card { background:#ffffff; border-radius:14px; box-shadow:0 18px 44px rgba(0,0,0,.35); padding:22px 26px; }
  .card .src { color:#4b5563; font-size:17px; line-height:1.6; }
  .card .dst { color:#111827; font-size:18px; line-height:1.6; border-left:3px solid #2f6bff; padding-left:12px; margin-top:8px; }
  .marquee { width:1400px; height:560px; }
  .marquee .row { justify-content:space-between; padding:0 76px; }
  .marquee .card { width:520px; }
  .small { width:440px; height:280px; }
  .small .row { flex-direction:column; gap:16px; justify-content:center; text-align:center; padding:0 30px; }
  .small .logo { width:74px; height:74px; border-radius:18px; }
  .small h1 { font-size:30px; }
  .small p.tag { font-size:16px; margin-top:6px; }
`;

const MARQUEE = `<!doctype html><html><head><meta charset="utf-8"><style>${CSS}</style></head>
<body><div class="tile marquee">
  <div class="glow g1"></div><div class="glow g2"></div>
  <div class="row">
    <div style="display:flex;align-items:center;gap:28px">
      <img class="logo" src="${ICON_URL}" />
      <div>
        <h1>简译 Glance</h1>
        <p class="tag">整页 / 悬停 / 划词，译成简体中文</p>
        <p class="tag" style="font-size:20px;color:#8ea0c8">可搭配桌面端，也能直连 DeepSeek、OpenAI 等接口</p>
      </div>
    </div>
    <div class="card">
      <div class="src">Rust is a systems programming language that runs blazingly fast and prevents segfaults.</div>
      <div class="dst">Rust 是一门系统编程语言，运行速度极快，并且能避免段错误。</div>
    </div>
  </div>
</div></body></html>`;

const SMALL = `<!doctype html><html><head><meta charset="utf-8"><style>${CSS}</style></head>
<body><div class="tile small">
  <div class="glow g1" style="width:260px;height:260px;left:-90px;top:-90px"></div>
  <div class="glow g2" style="width:220px;height:220px;right:-70px;bottom:-80px"></div>
  <div class="row">
    <img class="logo" src="${ICON_URL}" />
    <div>
      <h1>简译 Glance</h1>
      <p class="tag">整页 / 悬停 / 划词翻译</p>
    </div>
  </div>
</div></body></html>`;

const ctx = await chromium.launch();
try {
  for (const [name, html, w, h] of [
    ["marquee-promo-tile-1400x560.png", MARQUEE, 1400, 560],
    ["small-promo-tile-440x280.png", SMALL, 440, 280],
  ]) {
    const page = await ctx.newPage();
    await page.setViewportSize({ width: w, height: h });
    await page.setContent(html, { waitUntil: "load" });
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${OUT}/${name}` });
    await page.close();
    console.log("已生成", name);
  }
} finally {
  await ctx.close();
}
