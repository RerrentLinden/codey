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
