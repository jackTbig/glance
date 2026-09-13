// 插件 ID 必须固定，否则换个加载目录 Chrome 就当成另一个插件、设置全"丢"。
// manifest 里的 "key" 决定 ID：ID = 公钥 DER 的 SHA-256 前 16 字节，每个十六进制位映射到 a..p。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
/** 固定 ID。**改了 manifest 的 key 就等于换插件**，用户得重新加载一次，设置也不跟过来 */
export const EXTENSION_ID = "bhngnlkgffjimhcmmhfkgcgbohdecoch";

function idFromKey(keyB64) {
  const der = Buffer.from(keyB64, "base64");
  const hex = createHash("sha256").update(der).digest("hex").slice(0, 32);
  return [...hex].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join("");
}

test("插件 ID 固定不变", () => {
  for (const p of ["../public/manifest.json", "../dist/manifest.json"]) {
    const m = JSON.parse(readFileSync(resolve(HERE, p), "utf8"));
    assert.ok(m.key, p + " 缺少 key，插件 ID 会跟着加载目录变");
    assert.equal(idFromKey(m.key), EXTENSION_ID, p + " 的 key 变了，插件 ID 会变");
  }
});
