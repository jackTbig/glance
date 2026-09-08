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
  for (const k of ["port", "hoverModifier", "hoverDelayMs", "selectionIcon", "defaultDisplay", "disabledHosts"]) {
    assert.ok(k in S.DEFAULT_SETTINGS, `缺 ${k}`);
  }
  assert.equal(S.DEFAULT_SETTINGS.port, 42731);
  assert.equal(S.DEFAULT_SETTINGS.hoverModifier, "alt");
});

test("服务未运行的提示里说了要做什么", () => {
  assert.match(S.SERVICE_DOWN, /启动/);
});
