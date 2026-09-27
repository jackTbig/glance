// 三个入口各自独立打成 IIFE：MV3 的 content script 不能用 ESM import，
// 而 IIFE 不支持多入口共享 chunk，所以一个入口打一次，shared.ts 会被各自内联。
import { build } from "esbuild";
import { cp, mkdir, rm } from "node:fs/promises";

const watch = process.argv.includes("--watch");
await rm("dist", { recursive: true, force: true });
await mkdir("dist", { recursive: true });

for (const name of ["background", "content", "options", "popup"]) {
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
// KaTeX 样式与字体随扩展投放（web_accessible_resources 里列了 katex/*），不联网
await mkdir("dist/katex", { recursive: true });
await cp("node_modules/katex/dist/katex.min.css", "dist/katex/katex.min.css");
await cp("node_modules/katex/dist/fonts", "dist/katex/fonts", { recursive: true });
console.log("打包完成 → extension/dist（Chrome 扩展页「加载已解压的扩展程序」选这个目录）");
if (watch) console.log("（watch 模式未启用：改完重新跑 npm run build）");
