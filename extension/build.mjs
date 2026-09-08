// 三个入口各自独立打成 IIFE：MV3 的 content script 不能用 ESM import，
// 而 IIFE 不支持多入口共享 chunk，所以一个入口打一次，shared.ts 会被各自内联。
import { build } from "esbuild";
import { cp, mkdir, rm } from "node:fs/promises";

const watch = process.argv.includes("--watch");
await rm("dist", { recursive: true, force: true });
await mkdir("dist", { recursive: true });

for (const name of ["background", "content", "options"]) {
  await build({
    entryPoints: [`src/${name}.ts`],
    bundle: true,
    format: "iife",
    target: "chrome110",
    outfile: `dist/${name}.js`,
    logLevel: "info",
    sourcemap: false,
    legalComments: "none",
  });
}
await cp("public", "dist", { recursive: true });
console.log("打包完成 → extension/dist（Chrome 扩展页「加载已解压的扩展程序」选这个目录）");
if (watch) console.log("（watch 模式未启用：改完重新跑 npm run build）");
