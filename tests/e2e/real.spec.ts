/**
 * 用真实后端跑一遍插件（只在手动指定时跑：`npx playwright test real --grep-invert=""`）。
 * 需要桌面端已在运行（默认端口 42731）并配好 Key。
 */
import { test, expect, chromium, type BrowserContext } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { dirname, resolve, join } from "node:path";
import { existsSync, readFileSync } from "node:fs";
import { createServer, type Server } from "node:http";

const HERE = dirname(fileURLToPath(import.meta.url));
const EXT = resolve(HERE, "../../extension/dist");
const PAGES_PORT = 41996;
const PAGE = `http://127.0.0.1:${PAGES_PORT}/article.html`;

let pages: Server;
let ctx: BrowserContext;

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
  ctx = await chromium.launchPersistentContext("", {
    headless: false,
    args: [`--disable-extensions-except=${EXT}`, `--load-extension=${EXT}`],
  });
  const [sw] = ctx.serviceWorkers().length ? ctx.serviceWorkers() : [await ctx.waitForEvent("serviceworker")];
  await sw.evaluate(async () => {
    await chrome.storage.sync.set({ port: 42731, defaultDisplay: "bilingual" });
  });
});

test.afterAll(async () => {
  await ctx?.close();
  pages?.close();
});

test("真实后端整页翻译", async () => {
  const page = await ctx.newPage();
  await page.goto(PAGE);
  await tell(ctx, "toggle_page");
  const t = page.locator("#p1 .tt-translation");
  await expect(t).toBeVisible({ timeout: 90_000 });
  const zh = await t.textContent();
  console.log("译文：", zh);
  expect(zh).toMatch(/[\u4e00-\u9fff]/);
  const link = page.locator("#p2 .tt-translation a");
  await expect(link).toHaveAttribute("href", "https://doc.rust-lang.org");
  console.log("带链接段：", await page.locator("#p2 .tt-translation").innerHTML());
  await page.close();
});
