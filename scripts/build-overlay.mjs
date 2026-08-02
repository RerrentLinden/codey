import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const vite = join(root, "node_modules", "vite", "bin", "vite.js");

const result = spawnSync(
  process.execPath,
  [vite, "build", "--config", "vite.overlay.config.ts"],
  {
    cwd: root,
    stdio: "inherit",
  },
);
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

// public/ 下的注入脚本以源码形态维护，但会被逐字节嵌入 Codey 二进制并在
// Codex 渲染进程内求值。这里统一压缩到 dist-overlay/inject/，cdp.rs 只嵌入
// 压缩产物。
const { transformWithEsbuild } = await import("vite");
const publicDir = join(root, "public");
const injectDir = join(root, "dist-overlay", "inject");
mkdirSync(injectDir, { recursive: true });
let rawTotal = 0;
let minifiedTotal = 0;
for (const name of readdirSync(publicDir).filter((entry) => entry.endsWith(".js"))) {
  const source = readFileSync(join(publicDir, name), "utf8");
  // esbuild 在解析层就会常量折叠 `"__CODEY_X__" === "true"` 这类比较，任何
  // minify 开关都关不掉；这些占位符是 cdp.rs 按启动设置做运行时替换的锚点。
  // 因此压缩后逐一校验占位符仍在，丢失即整文件回退为源码拷贝。
  const markers = [...new Set(source.match(/__CODEY[A-Z_]*__/g) ?? [])];
  const { code } = await transformWithEsbuild(source, name, {
    minify: true,
    target: "es2022",
    sourcemap: false,
  });
  const lostMarkers = markers.filter((marker) => !code.includes(marker));
  const output = lostMarkers.length > 0 ? source : code;
  if (lostMarkers.length > 0) {
    console.log(
      `[overlay] kept ${name} unminified (folded markers: ${lostMarkers.join(", ")})`,
    );
  }
  writeFileSync(join(injectDir, name), output);
  rawTotal += Buffer.byteLength(source);
  minifiedTotal += Buffer.byteLength(output);
}
console.log(
  `[overlay] minified inject scripts: ${rawTotal} -> ${minifiedTotal} bytes`,
);

// 只删除“每个逗号选择器都带 -rtl 类”的独立规则；与 body/:host 等共享选择
// 器列表的规则（如 semi-always-* 主题变量块）原样保留。花括号配对带引号感
// 知，@ 块整体按嵌套深度跳过。
function stripRtlOnlyRules(css) {
  let output = "";
  let index = 0;
  let removed = 0;
  while (index < css.length) {
    const braceIndex = css.indexOf("{", index);
    if (braceIndex === -1) {
      output += css.slice(index);
      break;
    }
    const selector = css.slice(index, braceIndex);
    let depth = 1;
    let cursor = braceIndex + 1;
    let quote = "";
    while (cursor < css.length && depth > 0) {
      const character = css[cursor];
      if (quote) {
        if (character === "\\") cursor += 1;
        else if (character === quote) quote = "";
      } else if (character === '"' || character === "'") {
        quote = character;
      } else if (character === "{") {
        depth += 1;
      } else if (character === "}") {
        depth -= 1;
      }
      cursor += 1;
    }
    const block = css.slice(index, cursor);
    const trimmed = selector.trim();
    const droppable =
      !trimmed.startsWith("@") &&
      trimmed.length > 0 &&
      trimmed
        .split(",")
        .every((part) => /-rtl(?![A-Za-z0-9_-])/.test(part));
    if (droppable) {
      removed += block.length;
    } else {
      output += block;
    }
    index = cursor;
  }
  console.log(`[overlay] stripped rtl-only css rules: ${removed} bytes`);
  return output;
}

const cssPath = join(root, "dist-overlay", "codey.css");
writeFileSync(cssPath, stripRtlOnlyRules(readFileSync(cssPath, "utf8")));
