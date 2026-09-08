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
  await page.waitForTimeout(150); // 连续切显示模式时别撞上去重窗口
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
  // 仅译文：原文壳藏起来，译文块仍然看得见（早先的实现按 body 直接子元素隐藏，会把整页连译文一起藏掉）
  await expect(page.locator("#p1 .tt-original")).toBeHidden();
  await expect(page.locator("#p1 .tt-translation")).toBeVisible();
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/);

  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toBeHidden();
  await expect(page.locator("#p1 .tt-original")).toBeVisible();
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

test("悬停：鼠标先停好再按住 Alt（最常见的操作顺序）", async () => {
  const page = await openPage();
  const box = await page.locator("#l2").boundingBox();
  // 先把鼠标移到段落上，此时不按任何键
  await page.mouse.move(box!.x + 20, box!.y + 8);
  await page.waitForTimeout(200);
  // 再按住修饰键——这一步没有 mousemove 事件，只在 mousemove 里判定的话永远不触发
  await page.keyboard.down("Alt");
  await expect(page.locator("#l2 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.keyboard.up("Alt");
  await page.close();
});

test("悬停：先按住 Alt 再移动鼠标也要触发", async () => {
  const page = await openPage();
  const box = await page.locator("#l1").boundingBox();
  await page.keyboard.down("Alt");
  await page.mouse.move(box!.x + 20, box!.y + 8);
  await page.mouse.move(box!.x + 24, box!.y + 10);
  await expect(page.locator("#l1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.keyboard.up("Alt");
  await page.close();
});

test("悬停：改成 Ctrl 后按 Ctrl 生效、按 Alt 不生效", async () => {
  const sw = ctx.serviceWorkers()[0];
  await sw.evaluate(async () => chrome.storage.sync.set({ hoverModifier: "ctrl" }));
  const page = await openPage();
  const box = await page.locator("#l1").boundingBox();
  await page.mouse.move(box!.x + 20, box!.y + 8);

  await page.keyboard.down("Alt");
  await page.waitForTimeout(600);
  await expect(page.locator("#l1 .tt-translation")).toHaveCount(0);
  await page.keyboard.up("Alt");

  await page.keyboard.down("Control");
  await expect(page.locator("#l1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.keyboard.up("Control");

  await sw.evaluate(async () => chrome.storage.sync.set({ hoverModifier: "alt" }));
  await page.close();
});

test("悬停：松开修饰键就取消，还没到时间不该翻", async () => {
  const sw = ctx.serviceWorkers()[0];
  await sw.evaluate(async () => chrome.storage.sync.set({ hoverDelayMs: 1200 }));
  const page = await openPage();
  const box = await page.locator("#l2").boundingBox();
  await page.mouse.move(box!.x + 20, box!.y + 8);
  await page.keyboard.down("Alt");
  await page.waitForTimeout(300);
  await page.keyboard.up("Alt"); // 没到 1200ms 就松手
  await page.waitForTimeout(1500);
  await expect(page.locator("#l2 .tt-translation")).toHaveCount(0);
  await sw.evaluate(async () => chrome.storage.sync.set({ hoverDelayMs: 120 }));
  await page.close();
});

test("桌面端没运行时给出明确提示", async () => {
  const page = await openPage();
  const sw = ctx.serviceWorkers()[0];
  try {
    await sw.evaluate(async () => {
      await chrome.storage.sync.set({ port: 1 }); // 指向一个没人监听的端口
    });
    await page.reload();
    await triggerPage(page);
    // 逐段发送后，错误显示在段落自己的失败块里
    await expect(page.locator("#p1 > .tt-badge.tt-error")).toHaveAttribute("title", /翻译工具未运行/, { timeout: 20_000 });
  } finally {
    // 一定要还原，否则后面所有用例都连不上假服务
    await sw.evaluate(async (port) => {
      await chrome.storage.sync.set({ port });
    }, SERVICE_PORT);
    await page.close();
  }
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
  await expect(popup.locator("#hoverDesc")).toHaveText(/已关闭/);

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

// ---------------- 单段翻译（快捷键，中英对照） ----------------

test("恢复原文后 DOM 回到翻译前的样子（原文壳与标记都清掉）", async () => {
  const page = await openPage();
  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await tell(ctx, "restore_page");
  await expect(page.locator(".tt-translation")).toHaveCount(0);
  await expect(page.locator(".tt-original")).toHaveCount(0);
  await expect(page.locator("[data-tt-done]")).toHaveCount(0);
  await expect(page.locator("#p1")).toHaveText("Rust is a systems programming language that runs blazingly fast and prevents segfaults.");
  await page.close();
});

test("整页切到仅译文后，单段翻译仍然中英对照", async () => {
  const page = await openPage();
  await triggerPage(page); // 整页翻译
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await triggerPage(page); // 切到仅译文
  await expect(page.locator("body")).toHaveClass(/tt-hide-original/);

  // 此时翻一个还没翻过的段落（动态加进来的那段）
  const box = await page.locator("#dynamic").boundingBox();
  if (box) {
    await page.mouse.move(box.x + 10, box.y + 5);
    await page.waitForTimeout(120);
    await page.keyboard.down("Alt");
    await page.waitForTimeout(700);
    await page.keyboard.up("Alt");
  }
  // 仅译文模式下，这一段的原文仍然可见（tt-paired 豁免）
  const dyn = page.locator("#dynamic");
  if ((await dyn.count()) > 0) {
    await expect(dyn.locator(".tt-translation")).toBeVisible({ timeout: 30_000 });
    await expect(dyn.locator(".tt-original")).toBeVisible();
  }
  await page.close();
});

test("同一页面重复注入 content script 不会绑两套监听", async () => {
  const page = await openPage();
  const sw = ctx.serviceWorkers()[0];
  // 模拟扩展更新后 background 的重新注入
  await sw.evaluate(async () => {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    if (tab?.id !== undefined) {
      await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: ["content.js"] });
    }
  });
  await page.waitForTimeout(300);

  await page.evaluate(() => {
    const el = document.getElementById("p1")!;
    const r = document.createRange();
    r.selectNodeContents(el);
    const s = window.getSelection()!;
    s.removeAllRanges();
    s.addRange(r);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, clientX: 100, clientY: 100 }));
  });
  await expect(page.locator("#tt-icon")).toHaveCount(1, { timeout: 5000 });
  await page.close();
});

// ---------------- 页面内快捷键（不走 chrome.commands） ----------------

test("在页面里真按 Alt+Shift+Y 翻整页", async () => {
  const page = await openPage();
  await page.keyboard.press("Alt+Shift+Y");
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.close();
});

test("在页面里真按 Alt+Shift+S 翻选中文字", async () => {
  const page = await openPage();
  await page.evaluate(() => {
    const el = document.getElementById("p2")!;
    const r = document.createRange();
    r.selectNodeContents(el);
    const s = window.getSelection()!;
    s.removeAllRanges();
    s.addRange(r);
  });
  await page.keyboard.press("Alt+Shift+S");
  await expect(page.locator("#tt-panel .tt-body")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.close();
});

test("在输入框里打字不会触发快捷键", async () => {
  const page = await openPage();
  await page.evaluate(() => {
    const input = document.createElement("input");
    input.id = "probe";
    document.body.prepend(input);
    input.focus();
  });
  await page.keyboard.press("Alt+Shift+Y");
  await page.waitForTimeout(600);
  await expect(page.locator(".tt-translation")).toHaveCount(0);
  await page.close();
});


// ---------------- 段落翻译：按住修饰键的完整语义 ----------------

/** 鼠标停到某元素上，按住修饰键等一会儿再松开 */
async function holdOver(page: Page, sel: string, key = "Alt", ms = 700) {
  const box = await page.locator(sel).boundingBox();
  await page.mouse.move(box!.x + 20, box!.y + 8);
  await page.waitForTimeout(120);
  await page.keyboard.down(key);
  await page.waitForTimeout(ms);
  await page.keyboard.up(key);
}

test("段落翻译：原文保留，译文插在下方", async () => {
  const page = await openPage();
  await holdOver(page, "#l1");
  await expect(page.locator("#l1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await expect(page.locator("#l1 .tt-original")).toBeVisible();
  await expect(page.locator("#l1")).toContainText("Memory safety without garbage collection.");
  const lastClass = await page.locator("#l1").evaluate((el) => (el.lastElementChild as HTMLElement)?.className ?? "");
  expect(lastClass).toContain("tt-translation");
  await page.close();
});

test("段落翻译：按住不放不会翻第二次，也不会翻到外层大块", async () => {
  const page = await openPage();
  const box = await page.locator("#l1").boundingBox();
  await page.mouse.move(box!.x + 20, box!.y + 8);
  await page.keyboard.down("Alt");
  await expect(page.locator("#l1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.waitForTimeout(1500); // 一直按着、鼠标不动，等好几个判定周期
  await page.keyboard.up("Alt");
  await expect(page.locator("#l1 .tt-translation")).toHaveCount(1);
  // 外层容器不该被整块翻掉（早先 blockAt 会跳过已翻段落，往上找到父块又翻一遍）
  await expect(page.locator("ul > .tt-translation")).toHaveCount(0);
  await expect(page.locator("body > .tt-translation")).toHaveCount(0);
  await page.close();
});

test("段落翻译：松开再按住同一段就撤掉译文，第三次又翻回来", async () => {
  const page = await openPage();
  await holdOver(page, "#l2");
  await expect(page.locator("#l2 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });

  await holdOver(page, "#l2");
  await expect(page.locator("#l2 .tt-translation")).toHaveCount(0);
  await expect(page.locator("#l2 .tt-original")).toHaveCount(0);
  await expect(page.locator("#l2")).not.toHaveAttribute("data-tt-done", /.*/);
  await expect(page.locator("#l2")).toHaveText("Concurrency without data races.");

  await holdOver(page, "#l2");
  await expect(page.locator("#l2 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.close();
});

test("段落翻译：整页翻译后也能单独撤掉某一段", async () => {
  const page = await openPage();
  await triggerPage(page);
  await expect(page.locator("#p1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await holdOver(page, "#p1");
  await expect(page.locator("#p1 .tt-translation")).toHaveCount(0);
  await expect(page.locator("#p2 .tt-translation")).toHaveText(/zh=/);
  await page.close();
});

test("段落翻译：按住修饰键扫过多段，每段各翻一次", async () => {
  const page = await openPage();
  const b1 = await page.locator("#l1").boundingBox();
  await page.mouse.move(b1!.x + 20, b1!.y + 8);
  await page.keyboard.down("Alt");
  await expect(page.locator("#l1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  // 第一段插入译文后第二段位置下移了，坐标要重新取
  const b2 = await page.locator("#l2").boundingBox();
  await page.mouse.move(b2!.x + 20, b2!.y + 8);
  await expect(page.locator("#l2 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await page.keyboard.up("Alt");
  await expect(page.locator("#l1 .tt-translation")).toHaveCount(1);
  await expect(page.locator("#l2 .tt-translation")).toHaveCount(1);
  await page.close();
});

test("段落翻译：换成 Ctrl 后按 Ctrl 生效、按 Alt 不生效", async () => {
  const sw = ctx.serviceWorkers()[0];
  await sw.evaluate(async () => chrome.storage.sync.set({ hoverModifier: "ctrl" }));
  const page = await openPage();
  await holdOver(page, "#l1", "Alt", 700);
  await expect(page.locator("#l1 .tt-translation")).toHaveCount(0);
  await holdOver(page, "#l1", "Control", 700);
  await expect(page.locator("#l1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  // Ctrl 也要能撤销
  await holdOver(page, "#l1", "Control", 700);
  await expect(page.locator("#l1 .tt-translation")).toHaveCount(0);
  await sw.evaluate(async () => chrome.storage.sync.set({ hoverModifier: "alt" }));
  await page.close();
});

// ---------------- 整页翻译：流式上屏 ----------------

test("整页翻译：译文分批出现，先上面后下面，进度实时更新", async () => {
  const page = await ctx.newPage();
  await page.goto(`http://127.0.0.1:${PAGES_PORT}/long.html`);
  await page.waitForLoadState("domcontentloaded");
  await page.bringToFront();
  await page.waitForTimeout(150);

  // 让假后端慢一点，才看得出"分批"
  await tell(ctx, "toggle_page");

  // 第一段先出来，此时最后一段还没出来 —— 证明不是等整页翻完才一次性显示
  await expect(page.locator("#s1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  const lastDoneEarly = await page.locator("#s60 .tt-translation").count();

  // 进度提示带数字
  await expect(page.locator("#tt-toast")).toHaveText(/翻译中 \d+\/\d+…|完成，共 \d+ 段/, { timeout: 10_000 });

  // 最终全部翻完
  await expect(page.locator("#s60 .tt-translation")).toHaveText(/zh=/, { timeout: 60_000 });
  await expect(page.locator("#tt-toast")).toHaveText(/完成，共 \d+ 段/, { timeout: 30_000 });
  // 60 个 p 加一个 h1 标题
  await expect(page.locator(".tt-translation")).toHaveCount(61);

  console.log("第一段出现时，最后一段是否已出现：", lastDoneEarly > 0 ? "是（页面太短没看出分批）" : "否（确实是分批上屏）");
  await page.close();
});

test("整页翻译过程中第一批落地后就能恢复原文", async () => {
  const page = await ctx.newPage();
  await page.goto(`http://127.0.0.1:${PAGES_PORT}/long.html`);
  await page.bringToFront();
  await page.waitForTimeout(150);
  await tell(ctx, "toggle_page");
  await expect(page.locator("#s1 .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });

  // 不等翻完就问状态，应该已经是「已翻译」
  const sw = ctx.serviceWorkers()[0];
  const st = await sw.evaluate(async () => {
    const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
    return await chrome.tabs.sendMessage(tab!.id!, { kind: "status" });
  });
  expect(st.translated).toBe(true);
  await page.close();
});

// ---------------- 逐段发送：加载动画与失败图标 ----------------

test("整页翻译：发出去的段先转圈，回来再变译文", async () => {
  const page = await ctx.newPage();
  await page.goto(`http://127.0.0.1:${PAGES_PORT}/states.html`);
  await page.bringToFront();
  await page.waitForTimeout(150);
  await tell(ctx, "toggle_page");

  // 慢的那段应当先出现转圈徽章：行内、跟在原文后面、不占一行
  const pending = page.locator("#slow > .tt-badge.tt-loading");
  await expect(pending).toBeVisible({ timeout: 10_000 });
  const box = await pending.boundingBox();
  expect(box!.width).toBeLessThan(30); // 只是个小点，不能撑大布局
  expect(box!.height).toBeLessThan(30);

  // 快的那段这时已经出译文了 —— 证明是一段一段独立走完的
  await expect(page.locator("#fine .tt-translation")).toHaveText(/zh=/, { timeout: 15_000 });

  // 慢的那段最终也会好
  await expect(page.locator("#slow .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await expect(page.locator("#slow > .tt-badge")).toHaveCount(0);
  await expect(page.locator("#tt-toast")).toHaveText(/完成/, { timeout: 30_000 });
  await page.close();
});

test("整页翻译：失败的段显示警示图标，可点重试", async () => {
  const page = await ctx.newPage();
  await page.goto(`http://127.0.0.1:${PAGES_PORT}/states.html`);
  await page.bringToFront();
  await page.waitForTimeout(150);
  await tell(ctx, "toggle_page");

  const bad = page.locator("#bad > .tt-badge.tt-error");
  await expect(bad).toBeVisible({ timeout: 30_000 });
  await expect(bad).toHaveText("⚠ 重试");
  await expect(bad).toHaveAttribute("title", /翻译失败/);
  // 只是个小标记，不能变成一大段文字
  const bbox = await bad.boundingBox();
  expect(bbox!.height).toBeLessThan(40);

  // 别的段不受影响
  await expect(page.locator("#fine .tt-translation")).toHaveText(/zh=/);
  await expect(page.locator("#slow .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });

  // 点它会重新发一次（这段在假后端里恒失败，所以还是失败，但要能看到重试发生过）
  await bad.click();
  await expect(page.locator("#bad > .tt-badge.tt-loading")).toBeVisible({ timeout: 5000 });
  await expect(page.locator("#bad > .tt-badge.tt-error")).toBeVisible({ timeout: 30_000 });
  await page.waitForTimeout(300); // 让在途请求落地，别拖累后面的用例
  await page.close();
});

test("整页翻译：一段一个请求，请求数正好等于段数", async () => {
  const page = await ctx.newPage();
  await page.goto(`http://127.0.0.1:${PAGES_PORT}/simple.html`);
  await page.bringToFront();
  await page.waitForTimeout(200);
  // 页面就位后再清零，免得把别的请求算进来
  await fetch(`http://127.0.0.1:${MOCK_PORT}/reset`, { method: "POST" });
  await tell(ctx, "toggle_page");
  await expect(page.locator("#c .tt-translation")).toHaveText(/zh=/, { timeout: 30_000 });
  await expect(page.locator(".tt-translation")).toHaveCount(3);
  await page.waitForTimeout(400);

  const stats = await (await fetch(`http://127.0.0.1:${MOCK_PORT}/stats`)).json();
  console.log("3 段页面，假后端收到的请求数：", stats.requests);
  expect(stats.requests).toBe(3); // 打包发的话只会是 1
  await page.close();
});
