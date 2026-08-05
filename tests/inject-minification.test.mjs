import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { transformWithEsbuild } from "vite";

const root = new URL("../", import.meta.url);
const fixtures = [
  ["fast-startup-shield.js", "__CODEY_FAST_CODEX_STARTUP__"],
  ["pet-control-shield.js", "__CODEY_SLIM_PET__"],
  ["voice-control-shield.js", "__CODEY_SLIM_VOICE__"],
];

test("boolean injection markers survive esbuild minification", async () => {
  for (const [name, marker] of fixtures) {
    const source = await readFile(new URL(`public/${name}`, root), "utf8");
    const { code } = await transformWithEsbuild(source, name, {
      minify: true,
      target: "es2022",
      sourcemap: false,
    });

    assert.match(code, new RegExp(marker), `${name} lost its runtime replacement marker`);
    assert.ok(
      Buffer.byteLength(code) < Buffer.byteLength(source),
      `${name} unexpectedly fell back to source-sized output`,
    );
  }
});
