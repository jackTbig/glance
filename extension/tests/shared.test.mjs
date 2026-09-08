// 纯函数测试：用 esbuild 把 TS 现编成 ESM 再导入（不引 ts-node）
import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const out = join(mkdtempSync(join(tmpdir(), "tt-")), "shared.mjs");
const r = await build({
  entryPoints: ["src/shared.ts"],
  bundle: true,
  format: "esm",
  write: false,
  target: "node20",
});
writeFileSync(out, r.outputFiles[0].text);
const S = await import(pathToFileURL(out).href);

test("中文判定", () => {
  assert.equal(S.isMostlyChinese("这是一段中文，含 API 两个词"), true);
  assert.equal(S.isMostlyChinese("Mostly English with 一个 word"), false);
  assert.equal(S.isMostlyChinese("   "), false);
});

test("值得翻译", () => {
  assert.equal(S.worthTranslating("hello"), true);
  assert.equal(S.worthTranslating("这已经是中文"), false, "中文不必翻");
  assert.equal(S.worthTranslating("+"), false);
  assert.equal(S.worthTranslating("12 34"), false);
  assert.equal(S.worthTranslating(" a "), false, "单字符不翻");
});

test("默认设置字段齐全", () => {
  for (const k of ["port", "hoverModifier", "hoverDelayMs", "selectionIcon", "defaultDisplay", "disabledHosts", "keyFullPage", "keySelection"]) {
    assert.ok(k in S.DEFAULT_SETTINGS, `缺 ${k}`);
  }
  assert.equal(S.DEFAULT_SETTINGS.port, 42731);
  assert.equal(S.DEFAULT_SETTINGS.hoverModifier, "alt");
});

test("服务未运行的提示里说了要做什么", () => {
  assert.match(S.SERVICE_DOWN, /启动/);
});

test("快捷键解析：至少要有 Alt 或 Ctrl", () => {
  assert.ok(S.parseHotkey("Alt+Shift+H"));
  assert.ok(S.parseHotkey("ctrl+q"));
  assert.equal(S.parseHotkey("Shift+H"), null, "只有 Shift 会吃掉正常打字");
  assert.equal(S.parseHotkey("H"), null);
  assert.equal(S.parseHotkey(""), null);
  assert.equal(S.parseHotkey("Alt+H+J"), null, "两个主键");
});

test("快捷键匹配：按 code 比，Alt 改变 key 时也认得", () => {
  const ev = (o) => ({ altKey: false, ctrlKey: false, shiftKey: false, key: "", code: "", ...o });
  assert.equal(S.matchesHotkey(ev({ altKey: true, shiftKey: true, code: "KeyH", key: "˙" }), "Alt+Shift+H"), true,
    "macOS 上按住 Alt 时 e.key 会变成别的字符，靠 code 才认得出");
  assert.equal(S.matchesHotkey(ev({ altKey: true, shiftKey: true, code: "KeyH", key: "H" }), "Alt+Shift+S"), false);
  assert.equal(S.matchesHotkey(ev({ altKey: true, code: "KeyH", key: "h" }), "Alt+Shift+H"), false, "少按 Shift 不算");
  assert.equal(S.matchesHotkey(ev({ altKey: true, shiftKey: true, ctrlKey: true, code: "KeyH" }), "Alt+Shift+H"), false,
    "多按 Ctrl 也不算");
});

test("输入框里不抢按键", () => {
  assert.equal(S.isTyping({ tagName: "INPUT" }), true);
  assert.equal(S.isTyping({ tagName: "TEXTAREA" }), true);
  assert.equal(S.isTyping({ tagName: "DIV", isContentEditable: true }), true);
  assert.equal(S.isTyping({ tagName: "P", isContentEditable: false }), false);
  assert.equal(S.isTyping(null), false);
});

test("段落翻译不占组合键，只用修饰键（避免两套语义打架）", () => {
  assert.equal("keyBlock" in S.DEFAULT_SETTINGS, false, "段落翻译只靠 hoverModifier，不再有独立组合键");
});

test("默认快捷键不与 Chrome 内置键冲突", () => {
  // Alt+Shift+T 是 Chrome 的工具栏焦点键，扩展永远拿不到，别拿来当默认
  for (const k of [S.DEFAULT_SETTINGS.keyFullPage, S.DEFAULT_SETTINGS.keySelection]) {
    assert.notEqual(k.toLowerCase(), "alt+shift+t");
    assert.ok(S.parseHotkey(k), `${k} 必须是合法写法`);
  }
});
