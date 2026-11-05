// 直连模式：提示词 / 打包 / HTML 保护必须与桌面端（Rust）一致
//
// 用例直接从 Rust 源码抠，不在这里抄一份：抄一份两边迟早漂移
// （Rust 改了提示词、这里的断言还是旧的，测试照样绿，结果同一个词
//   在桌面端给词典卡、在网页里只给一句译文）。
import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { writeFileSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const out = join(mkdtempSync(join(tmpdir(), "tt-direct-")), "direct.mjs");
const r = await build({ entryPoints: ["src/direct.ts"], bundle: true, format: "esm", write: false, target: "node20" });
writeFileSync(out, r.outputFiles[0].text);
const D = await import(pathToFileURL(out).href);

/** Rust 字符串字面量：先取出原文，再按 Rust 的规则解转义 */
function unescapeRust(s) {
  let o = "";
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "\\" && i + 1 < s.length) {
      const n = s[++i];
      if (n === "n") o += "\n";
      else if (n === "t") o += "\t";
      else if (n === "r") o += "\r";
      else if (n === "\\") o += "\\";
      else if (n === '"') o += '"';
      else if (n === "\n") { /* 字符串行续行：跳过 */ }
      else o += n;
    } else o += s[i];
  }
  return o;
}

/** 取 marker 之后的第一个字符串字面量 */
function rustLiteralAfter(src, marker) {
  const i = src.indexOf(marker);
  assert.ok(i >= 0, `Rust 源码里找不到 ${marker}，镜像测试失去对照`);
  const q = src.indexOf('"', i);
  assert.ok(q >= 0, `Rust 源码里 ${marker} 后面没有字符串字面量`);
  let j = q + 1;
  let raw = "";
  for (;;) {
    const c = src[j];
    if (c === undefined) break;
    if (c === "\\") {
      const n = src[j + 1];
      if (n === "\n") { j += 2; continue; } // 行续行
      if (n === "\r" && src[j + 2] === "\n") { j += 3; continue; }
      raw += "\\" + n;
      j += 2;
      continue;
    }
    if (c === '"') break;
    raw += c;
    j++;
  }
  return unescapeRust(raw);
}

const PROMPTS = readFileSync("../crates/ai-translate/src/prompts.rs", "utf8");

test("提示词与桌面端一致（原文取自 prompts.rs）", () => {
  const common = rustLiteralAfter(PROMPTS, "fn common").replaceAll("{t}", "简体中文");
  const fullPage = rustLiteralAfter(PROMPTS, "const FULL_PAGE");
  const selection = rustLiteralAfter(PROMPTS, "const SELECTION");
  const hover = rustLiteralAfter(PROMPTS, "const HOVER");
  const screenshot = rustLiteralAfter(PROMPTS, "const SCREENSHOT");
  const word = rustLiteralAfter(PROMPTS, "Direction::ToChinese =>");

  assert.equal(D.systemPrompt("hover"), `${common}\n${hover}`);
  assert.equal(D.systemPrompt("selection"), `${common}\n${selection}`);
  assert.equal(D.systemPrompt("full_page"), `${common}\n${fullPage}`);
  assert.equal(D.systemPrompt("screenshot"), `${common}\n${screenshot}`);
  // 取词走词典提示词，不叠 common（那句「只输出译文」会把词典条目掐掉）
  assert.equal(D.systemPrompt("word"), word);
  assert.ok(!D.systemPrompt("word").includes("只输出译文"));
});

test("用户消息与桌面端一致", () => {
  // 这几个前缀在 Rust 的 user_message 里，抠不出来就逐条核对源码
  for (const s of ["请翻译以下各段：", "请给出这个词的词典条目：", "请翻译：", "上下文（仅供参考，不要翻译）："]) {
    assert.ok(PROMPTS.includes(s), `prompts.rs 里没有这句：${s}`);
  }
  assert.equal(D.userMessage("full_page", "⟨1⟩ a"), "请翻译以下各段：\n⟨1⟩ a");
  assert.equal(D.userMessage("word", "glance"), "请给出这个词的词典条目：\nglance");
  assert.equal(D.userMessage("hover", "x"), "请翻译：\nx");
  assert.equal(D.userMessage("hover", "x", "prev"), "上下文（仅供参考，不要翻译）：\nprev\n\n请翻译：\nx");
});

test("打包与拆包与桌面端一致", () => {
  // 用例抄自 crates/ai-translate/src/batch.rs 的测试
  assert.equal(D.pack(["a", "b\nc"]), "⟨1⟩ a\n⟨2⟩ b c");
  assert.deepEqual(D.unpack("⟨2⟩ 乙\n⟨1⟩ 甲", 2), ["甲", "乙"]);
  assert.deepEqual(D.unpack("⟨1⟩ 甲\n⟨3⟩ 丙", 3), ["甲", null, "丙"]);
  assert.deepEqual(D.unpack("好的，译文如下：\n⟨1⟩ 甲\n⟨1⟩ 又一遍\n⟨9⟩ 越界", 1), ["甲"]);
  assert.deepEqual(D.unpack("zh=⟨1⟩ 甲", 1), ["甲"]);
});

test("HTML 占位符保护与桌面端一致", () => {
  // 用例抄自 crates/ai-translate/src/html_guard.rs 的测试
  const p = D.protectHtml('Hello <a href="x">world</a> and <b>bold</b><br>');
  assert.equal(p.text, "Hello ⟦1⟧world⟦/1⟧ and ⟦2⟧bold⟦/2⟧⟦3/⟧");
  assert.deepEqual(p.tags, ['<a href="x">', "<b>", "<br>"]);

  assert.equal(D.restoreHtml("甲 ⟦1⟧x⟦/1⟧ 乙", ["<i>"]), "甲 <i>x</i> 乙");
  assert.equal(D.restoreHtml("甲 x 乙", ["<i>"]), null, "占位符丢了要报错");
  assert.equal(D.restoreHtml("甲 ⟦1⟧x⟦/1⟧⟦/1⟧ 乙", ["<i>"]), null, "多出也要报错");

  assert.equal(D.protectHtml("no tags").text, "no tags");
  assert.equal(D.protectHtml("a < b and c > d").text, "a < b and c > d");
  assert.equal(D.restoreHtml("纯文本", []), "纯文本");

  const n = D.protectHtml('<b><i>x</i></b><img src="y"/>');
  assert.equal(n.text, "⟦1⟧⟦2⟧x⟦/2⟧⟦/1⟧⟦3/⟧");
  assert.equal(D.restoreHtml(n.text, n.tags), '<b><i>x</i></b><img src="y"/>');
});

test("接口地址归一化", () => {
  assert.equal(D.chatUrl("https://api.deepseek.com/v1"), "https://api.deepseek.com/v1/chat/completions");
  assert.equal(D.chatUrl("https://api.deepseek.com/v1/"), "https://api.deepseek.com/v1/chat/completions");
  assert.equal(D.chatUrl("https://x.com/v1/chat/completions"), "https://x.com/v1/chat/completions");
});

/* ---------------- 整条链路（假 fetch） ---------------- */

function mockFetch(reply) {
  const calls = [];
  globalThis.fetch = async (url, init) => {
    calls.push({ url, body: JSON.parse(init.body) });
    return {
      ok: true,
      status: 200,
      json: async () => ({ choices: [{ message: { content: reply } }] }),
      text: async () => reply,
    };
  };
  return calls;
}

test("整页：编号打包发出去，按编号拆回来", async () => {
  const calls = mockFetch("⟨1⟩ 你好\n⟨2⟩ 世界");
  const rs = await D.translateDirect(
    { base: "https://api.deepseek.com/v1", key: "k", model: "m" },
    "full_page",
    [{ id: "0", text: "hello" }, { id: "1", text: "world" }],
  );
  assert.deepEqual(rs, [
    { id: "0", text: "你好" },
    { id: "1", text: "世界" },
  ]);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, "https://api.deepseek.com/v1/chat/completions");
  assert.equal(calls[0].body.messages[0].role, "system");
  assert.ok(calls[0].body.messages[1].content.includes("⟨1⟩ hello"));
  assert.ok(calls[0].body.messages[1].content.includes("⟨2⟩ world"));
  assert.equal(calls[0].body.model, "m");
});

test("划词：一段一个请求，不加编号", async () => {
  const calls = mockFetch("你好");
  const rs = await D.translateDirect({ base: "https://x.com/v1", key: "k", model: "m" }, "selection", [
    { id: "s", text: "hello" },
  ]);
  assert.deepEqual(rs, [{ id: "s", text: "你好" }]);
  assert.equal(calls.length, 1);
  assert.ok(!calls[0].body.messages[1].content.includes("⟨1⟩"));
});

test("带内联标签的段：标签换成占位符，译完换回来", async () => {
  const calls = mockFetch("你好 ⟦1⟧世界⟦/1⟧");
  const rs = await D.translateDirect({ base: "https://x.com/v1", key: "k", model: "m" }, "selection", [
    { id: "s", text: "hello <b>world</b>", is_html: true },
  ]);
  assert.deepEqual(rs, [{ id: "s", text: "你好 <b>世界</b>" }]);
  assert.ok(calls[0].body.messages[1].content.includes("⟦1⟧"));
  assert.ok(!calls[0].body.messages[1].content.includes("<b>"));
});

test("接口报错：整段失败要有话给用户看", async () => {
  globalThis.fetch = async () => ({
    ok: false,
    status: 401,
    json: async () => ({}),
    text: async () => "invalid api key",
  });
  const rs = await D.translateDirect({ base: "https://x.com/v1", key: "bad", model: "m" }, "selection", [
    { id: "s", text: "hello" },
  ]);
  assert.equal(rs[0].id, "s");
  assert.match(rs[0].error, /401/);
});

test("模型列表：只认 OpenAI 兼容形状，失败要把原因带回去", async () => {
  const cfg = { base: "https://x.com/v1", key: "k", model: "" };
  const reply = (body, ok = true, status = 200) => {
    globalThis.fetch = async () => ({ ok, status, json: async () => body, text: async () => JSON.stringify(body) });
  };

  reply({ object: "list", data: [{ id: "b" }, { id: "a" }, { id: "a" }] });
  assert.deepEqual(await D.listModels(cfg), { models: ["a", "b"] }, "排序并去重");

  reply({ ok: true });
  assert.match((await D.listModels(cfg)).error, /data 数组/);

  reply({ data: [] });
  assert.match((await D.listModels(cfg)).error, /空的/);

  globalThis.fetch = async () => ({ ok: false, status: 404, json: async () => ({}), text: async () => "not found" });
  assert.match((await D.listModels(cfg)).error, /404/);

  globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => ({}), text: async () => "not json" });
  assert.match((await D.listModels(cfg)).error, /不是 JSON/);
});
