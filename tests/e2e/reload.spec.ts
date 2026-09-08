/**
 * 扩展被重新加载后，老页面里残留的 content script 会失去扩展上下文。
 * 这时用户一划词，浏览器原本抛的是 "Extension context invalidated."，
 * 本用例验证我们把它换成了可操作的中文提示。
 *
 * 单独一个文件跑：reload 会让整个扩展重启，与其它用例互相干扰。
 */
import { test, expect, chromium, type BrowserContext } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";
import { existsSync, readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";

const HERE = dirname(fileURLToPath(import.meta.url));
const EXT = resolve(HERE, "../../extension/dist");
const TARGET = process.env.CARGO_TARGET_DIR ?? "D:/DevCache/tt-target";
const MOCK_PORT = 41989;
const SERVICE_PORT = 41988;
const PAGES_PORT = 41987;
const PAGE = `http://127.0.0.1:${PAGES_PORT}/article.html`;

let mock: ChildProcess;
let service: ChildProcess;
let pages: Server;
let ctx: BrowserContext;

async function waitPort(port: number, path: string, ms = 20_000) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    try {
      if ((await fetch(`http://127.0.0.1:${port}${path}`)).ok) return;
    } catch {
      /* 还没起来 */
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`等 127.0.0.1:${port}${path} 超时`);
}

test.beforeAll(async () => {
  pages = await new Promise<Server>((ok) => {
    const s = createServer((req, res) => {
      const name = (req.url ?? "/").split("?")[0].replace(/^\//, "") || "article.html";
      const file = join(HERE, "pages", name);
      if (name.includes("..") || !existsSync(file)) return void res.writeHead(404).end();
      res.writeHead(200, { "Content-Type": "text/html; charset=utf-8" });
      res.end(readFileSync(file));
    });
    s.listen(PAGES_PORT, "127.0.0.1", () => ok(s));
  });
  mock = spawn(resolve(TARGET, "debug/mock-llm.exe"), ["--port", String(MOCK_PORT)], { stdio: "ignore" });
  await waitPort(MOCK_PORT, "/stats");
  service = spawn(
    resolve(TARGET, "debug/translate-tool-service.exe"),
    ["--port", String(SERVICE_PORT), "--base-url", `http://127.0.0.1:${MOCK_PORT}`, "--model", "mock"],
    { stdio: "ignore" },
  );
  await waitPort(SERVICE_PORT, "/v1/health");

  ctx = await chromium.launchPersistentContext("", {
    headless: false,
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`],
  });
  const [sw] = ctx.serviceWorkers().length ? ctx.serviceWorkers() : [await ctx.waitForEvent("serviceworker")];
  await sw.evaluate(async (port) => chrome.storage.sync.set({ port, selectionIcon: true }), SERVICE_PORT);
});

test.afterAll(async () => {
  await ctx?.close();
  pages?.close();
  mock?.kill();
  service?.kill();
});

test("重载扩展后，老页面划词给中文提示而不是英文报错", async () => {
  const page = await ctx.newPage();
  await page.goto(PAGE);
  await page.waitForTimeout(600); // 等 content script 注入

  // 重载扩展：老页面里的 content script 从此失去扩展上下文
  const sw = ctx.serviceWorkers()[0];
  await sw.evaluate(() => chrome.runtime.reload()).catch(() => {
    /* reload 会掐断这条连接，抛错是正常的 */
  });
  await page.waitForTimeout(1500);

  // 老页面上划词（用的还是失效的那份脚本）
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
  if ((await icon.count()) > 0) {
    await icon.click();
  }
  // 不管是图标点击还是提示，都不该出现英文原始错误
  await expect(page.locator("#tt-toast, #tt-panel")).toContainText(/刷新页面|插件刚更新过/, { timeout: 10_000 });
  await expect(page.locator("body")).not.toContainText("Extension context invalidated");
  await page.close();
});
