import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { build } from "vite";

test("HeroUI CSS builds valid checkbox selectors and preserves reduced motion", async () => {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const result = await build({
    root,
    configFile: fileURLToPath(new URL("../vite.overlay.config.ts", import.meta.url)),
    logLevel: "silent",
    build: {
      write: false,
      lib: { entry: fileURLToPath(new URL("./fixtures/heroui-css-entry.js", import.meta.url)) },
    },
  });
  const assets = [result].flat().flatMap(({ output }) => output);
  const css = assets.find((asset) => asset.type === "asset" && asset.fileName.endsWith(".css"))?.source;
  assert.equal(typeof css, "string", "build must emit CSS");
  assert.equal(/:is\(\s*\)/.test(css), false, "CSS must not contain empty :is() selectors");
  assert.ok(
    /\.checkbox__control:is\([^{}]+\):{1,2}before(?:,[^{}]*)?\{transition-property:none\}/.test(css),
    "explicit reduced motion must disable the checkbox pseudo-element transition",
  );
  assert.ok(
    /@media\s*\(prefers-reduced-motion:\s*reduce\)\{[^{}]*\.checkbox__control:not\([^{}]+\):{1,2}before(?:,[^{}]*)?\{transition-property:none\}/.test(css),
    "system reduced motion must disable the checkbox pseudo-element transition",
  );
});
