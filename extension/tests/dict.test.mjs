// 取词词典卡：判定与解析必须与桌面端（Rust）一致
import { test } from "node:test";
import assert from "node:assert/strict";
import { build } from "esbuild";
import { writeFileSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const out = join(mkdtempSync(join(tmpdir(), "tt-dict-")), "shared.mjs");
const r = await build({ entryPoints: ["src/shared.ts"], bundle: true, format: "esm", write: false, target: "node20" });
writeFileSync(out, r.outputFiles[0].text);
const S = await import(pathToFileURL(out).href);

/**
 * 直接从 Rust 测试里抠出用例清单，而不是在这里抄一份。
 * 抄一份的话两边迟早漂移：Rust 改了规则、这里的用例还是旧的，测试照样绿，
 * 结果同一个词在桌面端给词典卡、在网页里只给一句译文。
 */
function rustWordCases() {
  const src = readFileSync("../crates/ai-translate/src/scene.rs", "utf8");
  const start = src.indexOf("fn word_lookup_only_for_short_latin_terms");
  assert.ok(start >= 0, "Rust 侧的判定用例函数不见了，镜像测试失去对照");
  const body = src.slice(start, src.indexOf("#[test]", start));
  const blocks = [...body.matchAll(/for s in \[([\s\S]*?)\]\s*\{/g)].map((m) => m[1]);
  assert.equal(blocks.length, 2, "应当恰好有「该判成查词」与「不该判成查词」两组");
  const lits = (b) => [...b.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => JSON.parse(`"${m[1]}"`));
  return { yes: lits(blocks[0]), no: lits(blocks[1]) };
}

test("取词判定与桌面端逐条一致（用例取自 Rust 源码）", () => {
  const { yes, no } = rustWordCases();
  assert.ok(yes.length >= 5 && no.length >= 8, "没抠出足够的用例，正则可能失配");
  for (const s of yes) assert.equal(S.looksLikeWord(s), true, `该判成查词：${JSON.stringify(s)}`);
  for (const s of no) assert.equal(S.looksLikeWord(s), false, `不该判成查词：${JSON.stringify(s)}`);
});

const SAMPLE = [
  "#词 glance",
  "#音 英 /ɡlɑːns/ 美 /ɡlæns/",
  "#释 v. 一瞥，看一眼；浏览；掠过",
  "#释 n. 一瞥；闪光",
  "#形 复数 glances；过去式 glanced",
  "#例 She glanced at her watch.",
  "#译 她瞥了一眼手表。",
].join("\n");

test("解析完整词典条目", () => {
  const e = S.parseDictEntry(SAMPLE);
  assert.equal(e.head, "glance");
  assert.equal(e.phonetic, "英 /ɡlɑːns/ 美 /ɡlæns/");
  assert.deepEqual(e.senses, [
    { pos: "v.", text: "一瞥，看一眼；浏览；掠过" },
    { pos: "n.", text: "一瞥；闪光" },
  ]);
  assert.equal(e.forms, "复数 glances；过去式 glanced");
  assert.equal(e.exampleZh, "她瞥了一眼手表。");
  assert.ok(S.dictEntryToText(e).startsWith("glance\n英 /ɡlɑːns/"));
});

test("容忍半截与噪声输出", () => {
  // 只有词头与半行音标，也要能画
  const a = S.parseDictEntry("#词 glance\n#音 英 /ɡlɑ");
  assert.equal(a.head, "glance");
  assert.equal(a.senses.length, 0);
  // 围栏、缩进、模型多嘴的说明行
  const b = S.parseDictEntry("```\n  #词 ok\n这是多余的说明\n#释 adj. 好的\n```");
  assert.equal(b.head, "ok");
  assert.equal(b.senses.length, 1);
});

test("普通译文不是词典条目", () => {
  assert.equal(S.parseDictEntry("她瞥了一眼手表。"), null);
  assert.equal(S.parseDictEntry(""), null);
  assert.equal(S.parseDictEntry("#备注 什么的"), null, "只有 # 号却不是已知标记，同样不算");
});

test("白名单外的前缀不当词性切走", () => {
  assert.deepEqual(S.parseDictEntry("#释 e.g. 举个例子").senses[0], { pos: "", text: "e.g. 举个例子" });
  assert.equal(S.parseDictEntry("#释 n.").senses[0].pos, "", "词性后面没内容也不切");
});
