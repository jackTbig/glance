/** 浏览器里的划词：桌面端的取词路径要能读到选区（走无障碍接口，不靠模拟 Ctrl+C） */
import { test, expect, chromium, type BrowserContext } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";
import { existsSync, readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { execFile } from "node:child_process";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "../..");
const EXT = resolve(REPO, "extension/dist");
const PAGES_PORT = 41986;
const PAGE = `http://127.0.0.1:${PAGES_PORT}/article.html`;
const PROBE = "D:/DevCache/tt-target/debug/examples/browser_sel_probe.exe";

let ctx: BrowserContext;
let pages: Server;

function servePages(port: number): Promise<Server> {
  const s = createServer((req, res) => {
    const name = (req.url ?? "/").split("?")[0].replace("/", "") || "article.html";
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

test.beforeAll(async () => {
  expect(existsSync(EXT), "插件没打包").toBeTruthy();
  expect(existsSync(PROBE), "探针没编").toBeTruthy();
  pages = await servePages(PAGES_PORT);
  ctx = await chromium.launchPersistentContext("", {
    headless: false,
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`],
  });
  const [sw] = ctx.serviceWorkers().length ? ctx.serviceWorkers() : [await ctx.waitForEvent("serviceworker")];
  await sw.evaluate(async () => {
    await chrome.storage.sync.set({ selectionIcon: true, hoverModifier: "alt", hoverDelayMs: 120 });
  });
});

test.afterAll(async () => {
  await ctx?.close();
  pages?.close();
});

test("浏览器里划词能读到选区", async () => {
  const page = await ctx.newPage();
  await page.goto(PAGE);
  await page.waitForLoadState("domcontentloaded");
  await page.waitForTimeout(600);

  // 拖选一段
  const box = (await page.locator("#p1").boundingBox())!;
  await page.mouse.move(box.x + 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width - 2, box.y + box.height / 2, { steps: 10 });
  await page.mouse.up();
  await page.waitForTimeout(300);

  const domSel = await page.evaluate(() => window.getSelection()?.toString() ?? "");
  expect(domSel.length, "拖选应当有选区").toBeGreaterThan(10);

  const { stdout } = await new Promise<{ stdout: string }>((ok, bad) => {
    execFile(PROBE, ["Test Article - Google Chrome for Testing"], { timeout: 60_000 }, (e, out, err) => {
      if (e) bad(new Error(`${e.message}\n${out}\n${err}`));
      else ok({ stdout: out });
    });
  });
  console.error("\n===PROBE===\n" + stdout + "\n===END===\n");

  expect(stdout, "浏览器窗口没找到").not.toContain("CHROME_WINDOW_NOT_FOUND");
  expect(stdout, "应当走无障碍接口而不是模拟 Ctrl+C").toContain("SOURCE=Uia");
  const text = stdout.split("\n").find((l) => l.startsWith("TEXT="))?.slice(5) ?? "";
  expect(text.length, "应当读到选区文字").toBeGreaterThan(10);
  expect(text).toContain("Rust");
});
