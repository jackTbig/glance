// 公式渲染的纯函数部分：定界符识别。渲染本体靠 Playwright 端到端用例盯
import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const out = join(mkdtempSync(join(tmpdir(), "tt-")), "math.mjs");
const r = await build({
  entryPoints: ["src/math.ts"],
  bundle: true,
  format: "esm",
  write: false,
  target: "node20",
});
writeFileSync(out, r.outputFiles[0].text);
const M = await import(pathToFileURL(out).href);

test("hasMath 认三种定界符", () => {
  assert.equal(M.hasMath("质能方程 \\(E=mc^2\\) 成立"), true);
  assert.equal(M.hasMath("块级 \\[x^2\\] 也认"), true);
  assert.equal(M.hasMath("display 形式 $$x^2$$ 也认"), true);
});

test("restoreMath 把占位符换回原 HTML，令牌丢了就少一处不报错", () => {
  const parts = { "[[M1]]": '<span class="katex">x</span>', "[[M2]]": "<math>y</math>" };
  assert.equal(M.restoreMath("甲 [[M1]] 乙 [[M2]] 丙", parts), '甲 <span class="katex">x</span> 乙 <math>y</math> 丙');
  assert.equal(M.restoreMath("模型没保留令牌", parts), "模型没保留令牌");
  assert.equal(M.restoreMath("[[M1]] 出现两次 [[M1]]", { "[[M1]]": "Z" }), "Z 出现两次 Z");
});

test("hasMath 不被普通文本与单个美元误触发", () => {
  assert.equal(M.hasMath("The price is $5 and $10, no math here."), false);
  assert.equal(M.hasMath("普通译文，没有公式"), false);
  assert.equal(M.hasMath("反斜杠加 n 不是公式"), false);
});
