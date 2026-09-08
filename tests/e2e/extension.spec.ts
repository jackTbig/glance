/**
 * Chrome 插件端到端：加载真实插件 + 起 mock-llm 假后端 + 起桌面端的本地服务，
 * 在固定测试页上验证整页、悬停、划词。
 *
 * 前置：`cargo build -p translate-tool -p mock-llm` 已产出 exe（脚本会自己起进程）。
 */
import { test, expect, chromium, type BrowserContext, type Page } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";
import { existsSync, readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "../..");
const EXT = resolve(REPO, "extension/dist");
const TARGET = process.env.CARGO_TARGET_DIR ?? "D:/DevCache/tt-target";
const MOCK_PORT = 41999;
const SERVICE_PORT = 41998;
const PAGES_PORT = 41997;
// 测试页必须走 http：MV3 的 content script 默认不注入 file:// 页面
const PAGE = `http://127.0.0.1:${PAGES_PORT}/article.html`;

let mock: ChildProcess;
let service: ChildProcess;
let pages: Server;
let ctx: BrowserContext;

/** 托管 tests/e2e/pages 下的静态页面 */
function servePages(port: number): Promise<Server> {
  const s = createServer((req, res) => {
    const name = (req.url ?? "/").split("?")[0].replace(/^\//, "") || "article.html";
    const file = join(HERE, "pages", name);
    if (name.includes("..") || !existsSync(file)) {
      res.writeHead(404).end("not found");
      return;
    }
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
    res.end(readFileSync(file));
  });
  return new Promise((ok) => s.listen(port, "127.0.0.1", () => ok(s)));
}

async function waitPort(port: number, path: string, ms = 20_000) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}${path}`);
      if (r.ok) return;
    } catch {
      /* 还没起来 */
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`等 127.0.0.1:${port}${path} 超时`);
}

/** 给当前标签页的 content script 发消息；刚 goto 完脚本可能还没注入，重试到成功 */
async function tell(ctx: BrowserContext, kind: string, tries = 30) {
  const sw = ctx.serviceWorkers()[0];
  for (let i = 0; i < tries; i++) {
    try {
      await sw.evaluate(async (k) => {
        const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
        if (tab?.id === undefined) throw new Error("no tab");
        await chrome.tabs.sendMessage(tab.id, { kind: k });
      }, kind);
      return;
    } catch (e) {
      if (i === tries - 1) throw e;
      await new Promise((r) => setTimeout(r, 200));
    }
  }
}

test.beforeAll(async () => {
  expect(existsSync(EXT), `插件没打包，先在 extension/ 跑 npm run build：${EXT}`).toBeTruthy();
  const mockExe = resolve(TARGET, "debug/mock-llm.exe");
  const svcExe = resolve(TARGET, "debug/translate-tool-service.exe");
  expect(existsSync(mockExe), `缺 ${mockExe}，先跑 cargo build -p mock-llm`).toBeTruthy();
  expect(existsSync(svcExe), `缺 ${svcExe}，先跑 cargo build -p translate-tool --bin translate-tool-service`).toBeTruthy();

  pages = await servePages(PAGES_PORT);
  mock = spawn(mockExe, ["--port", String(MOCK_PORT)], { stdio: "ignore" });
  await waitPort(MOCK_PORT, "/stats");
  service = spawn(svcExe, ["--port", String(SERVICE_PORT), "--base-url", `http://127.0.0.1:${MOCK_PORT}`, "--model", "mock"], {
    stdio: "ignore",
  });
  await waitPort(SERVICE_PORT, "/v1/health");

  ctx = await chromium.launchPersistentContext("", {
    headless: false,
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`, "--allow-file-access-from-files"],
  });
  // 把插件设置改到测试端口
  const [sw] = ctx.serviceWorkers().length ? ctx.serviceWorkers() : [await ctx.waitForEvent("serviceworker")];
  await sw.evaluate(async (port) => {
    await chrome.storage.sync.set({ port, hoverModifier: "alt", hoverDelayMs: 120, selectionIcon: true, defaultDisplay: "bilingual" });
  }, SERVICE_PORT);
});

test.afterAll(async () => {
  await ctx?.close();
  pages?.close();
  mock?.kill();
  service?.kill();
});

async function openPage(): Promise<Page> {
  const page = await ctx.newPage();
  await page.goto(PAGE);
  await page.waitForLoadState("domcontentloaded");
  return page;
}

/** 直接调 content script 里挂的消息入口（等价于点插件图标） */
async function triggerPage(page: Page) {
  await page.bringToFront();
  await tell(ctx, "toggle_page");
}

test("整页翻译：逐段插入译文，中文段与代码块跳过", async () => {
  const page = await openPage();
  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });

  // 行内链接与加粗要还原
  const p2 = page.locator("#p2 .tt-translation");
  await expect(p2.locator("a")).toHaveAttribute("href", "https://doc.rust-lang.org");
  await expect(p2.locator("b")).toHaveCount(1);

  // 中文段在插件侧就被挑掉了，连请求都不发，所以不会有译文块
  await expect(page.locator("#p3 .tt-translation")).toHaveCount(0);
  await expect(page.locator("#p3")).toHaveText("这一段已经是中文，翻译时应当原样保留。");
  // 代码块与隐藏段不翻
  await expect(page.locator("#code .tt-translation")).toHaveCount(0);
  await expect(page.locator("#p4 .tt-translation")).toHaveCount(0);
  await expect(page.locator("#short .tt-translation")).toHaveCount(0);

  // 列表项也翻
  await expect(page.locator("#l1 .tt-translation")).toHaveText(/zh=/);
  await page.close();
});

test("整页翻译：再触发一次切到仅译文，第三次回到原文", async () => {
  const page = await openPage();
  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });

  await triggerPage(page);
  await expect(page.locator("body")).toHaveClass(/tt-hide-original/);

  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toBeHidden();
  await page.close();
});

test("动态加载的段落会被增量翻译", async () => {
  const page = await openPage();
  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await expect(page.locator("#dynamic .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.close();
});

test("划词：选中后出图标，点击弹面板给译文", async () => {
  const page = await openPage();
  await page.evaluate(() => {
    const el = document.getElementById("p1")!;
    const r = document.createRange();
    r.selectNodeContents(el);
    const s = window.getSelection()!;
    s.removeAllRanges();
    s.addRange(r);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 100, clientY: 100 }));
  });
  const icon = page.locator("#tt-icon");
  await expect(icon).toBeVisible({ timeout: 5000 });
  await icon.click();
  await expect(page.locator("#tt-panel .tt-body")).toHaveText(/zh=/, { timeout: 30_000 });

  // 关闭按钮能关掉
  await page.locator("#tt-panel button[title=关闭]").click();
  await expect(page.locator("#tt-panel")).toHaveCount(0);
  await page.close();
});

test("悬停：按住 Alt 停在段落上就地插译文", async () => {
  const page = await openPage();
  const box = await page.locator("#l2").boundingBox();
  await page.keyboard.down("Alt");
  await page.mouse.move(box!.x + 20, box!.y + 8);
  await page.mouse.move(box!.x + 24, box!.y + 10);
  await expect(page.locator("#l2 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.keyboard.up("Alt");
  await page.close();
});

test("桌面端没运行时给出明确提示", async () => {
  const page = await openPage();
  const sw = ctx.serviceWorkers()[0];
  await sw.evaluate(async () => {
    await chrome.storage.sync.set({ port: 1 }); // 指向一个没人监听的端口
  });
  await page.reload();
  await triggerPage(page);
  await expect(page.locator("#tt-toast")).toHaveText(/翻译工具未运行/, { timeout: 20_000 });
  await sw.evaluate(async (port) => {
    await chrome.storage.sync.set({ port });
  }, SERVICE_PORT);
  await page.close();
});

// ---------------- 功能菜单（popup） ----------------

/** 直接打开 popup.html 当普通页面测：Playwright 点不了扩展图标 */
async function openPopup(): Promise<Page> {
  const sw = ctx.serviceWorkers()[0];
  const extId = new URL(sw.url()).host;
  const p = await ctx.newPage();
  await p.goto(`chrome-extension://${extId}/popup.html`);
  // 等 init() 跑完再交互：它是异步的（读设置 → 查桌面端 → 问页面状态），
  // 中途点击会被随后的 render() 覆盖掉
  await p.waitForFunction(() => document.getElementById("conn")?.textContent !== "检测中…", null, { timeout: 15_000 });
  return p;
}

test("菜单：显示连接状态与当前翻译服务", async () => {
  const popup = await openPopup();
  await expect(popup.locator("#conn")).toHaveText(/桌面端已连接/, { timeout: 15_000 });
  await expect(popup.locator("#conn")).toHaveClass(/ok/);
  await expect(popup.locator("#serviceName")).toHaveText("mock");
  await popup.close();
});

test("菜单：两个功能格子能开关，状态即时落到设置里", async () => {
  const popup = await openPopup();
  const hover = popup.locator("#tile-hover");
  const icon = popup.locator("#tile-icon");
  await expect(hover).toHaveClass(/on/);
  await expect(icon).toHaveClass(/on/);

  await hover.click();
  await expect(hover).not.toHaveClass(/on/);
  await expect(popup.locator("#hoverDesc")).toHaveText("已关闭，点击开启");

  await icon.click();
  await expect(icon).not.toHaveClass(/on/);

  // 存到 chrome.storage 里了
  const sw = ctx.serviceWorkers()[0];
  const saved = await sw.evaluate(async () => chrome.storage.sync.get(["hoverModifier", "selectionIcon"]));
  expect(saved.hoverModifier).toBe("none");
  expect(saved.selectionIcon).toBe(false);

  // 还原，别影响后面的用例
  await hover.click();
  await icon.click();
  await expect(hover).toHaveClass(/on/);
  await popup.close();
});

test("菜单：点悬停格子里的键名循环换修饰键", async () => {
  const popup = await openPopup();
  await expect(popup.locator("#hoverKey")).toHaveText("Alt");
  await popup.locator("#hoverKey").click();
  await expect(popup.locator("#hoverKey")).toHaveText("Ctrl");
  await popup.locator("#hoverKey").click();
  await expect(popup.locator("#hoverKey")).toHaveText("Shift");
  await popup.locator("#hoverKey").click();
  await expect(popup.locator("#hoverKey")).toHaveText("Alt");
  // 点键名不该把格子关掉
  await expect(popup.locator("#tile-hover")).toHaveClass(/on/);
  await popup.close();
});

test("菜单：本站停用那一行要么藏起来，要么显示具体域名", async () => {
  const popup = await openPopup();
  const row = popup.locator("#siteRow");
  if (await row.isVisible()) {
    // 显示时必须带具体域名，不能是「在  停用」这种空文案
    await expect(popup.locator("#siteLabel")).toHaveText(/^在 \S+ 停用$/);
  }
  await popup.close();
});

test("站点停用后悬停与划词图标都不再触发，快捷键仍可用", async () => {
  const sw = ctx.serviceWorkers()[0];
  await sw.evaluate(async () => chrome.storage.sync.set({ disabledHosts: ["127.0.0.1"] }));
  const page = await openPage();

  // 悬停：不该插译文
  const box = await page.locator("#l1").boundingBox();
  await page.keyboard.down("Alt");
  await page.mouse.move(box!.x + 20, box!.y + 8);
  await page.mouse.move(box!.x + 24, box!.y + 10);
  await page.waitForTimeout(1200);
  await expect(page.locator("#l1 .tt-translation")).toHaveCount(0);
  await page.keyboard.up("Alt");

  // 划词图标：不该出现
  await page.evaluate(() => {
    const el = document.getElementById("p1")!;
    const r = document.createRange();
    r.selectNodeContents(el);
    const s = window.getSelection()!;
    s.removeAllRanges();
    s.addRange(r);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 100, clientY: 100 }));
  });
  await page.waitForTimeout(400);
  await expect(page.locator("#tt-icon")).toHaveCount(0);

  // 整页快捷键不受站点停用影响
  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });

  await sw.evaluate(async () => chrome.storage.sync.set({ disabledHosts: [] }));
  await page.close();
});

test("菜单：桌面端没运行时状态变红并提示", async () => {
  const sw = ctx.serviceWorkers()[0];
  await sw.evaluate(async () => chrome.storage.sync.set({ port: 1 }));
  const popup = await openPopup();
  await expect(popup.locator("#conn")).toHaveText(/桌面端未运行/, { timeout: 15_000 });
  await expect(popup.locator("#conn")).toHaveClass(/bad/);
  await expect(popup.locator("#downHint")).toBeVisible();
  await popup.close();
  await sw.evaluate(async (port) => chrome.storage.sync.set({ port }), SERVICE_PORT);
});
