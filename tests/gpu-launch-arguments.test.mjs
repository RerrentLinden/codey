import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { readAppStyles } from "./helpers/read-app-styles.mjs";

const root = new URL("../", import.meta.url);

test("three-position GPU slider is accessible and disabled on macOS", async () => {
  const [sectionsSource, stylesSource, appSource, previewSource] = await Promise.all([
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
    readAppStyles(root),
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/main.tsx", root), "utf8"),
  ]);

  assert.match(sectionsSource, /isMacClient: boolean/);
  assert.match(sectionsSource, /\{ value: "off", label: "关闭" \}/);
  assert.match(sectionsSource, /\{ value: "disableGpu", label: "禁用 GPU" \}/);
  assert.match(
    sectionsSource,
    /\{ value: "disableGpuRasterization", label: "禁用 GPU 栅格化" \}/,
  );
  assert.match(
    sectionsSource,
    /<fieldset[\s\S]{0,150}disabled=\{isMacClient \|\| isBusy\}/,
  );
  assert.match(sectionsSource, /type="radio"/);
  assert.match(sectionsSource, /checked=\{gpuLaunchMode\.value === mode\.value\}/);
  assert.match(sectionsSource, /gpuLaunchMode: mode\.value/);
  assert.match(sectionsSource, /<legend className="sr-only">Codex GPU 启动模式<\/legend>/);
  assert.match(sectionsSource, /aria-describedby="gpu-launch-mode-description"/);
  assert.match(sectionsSource, /macOS 下已禁用，不会向 Codex 传递 GPU 诊断参数/);
  assert.match(stylesSource, /\.gpu-mode-slider-thumb/);
  assert.match(stylesSource, /transform: translateX\(var\(--gpu-mode-offset\)\)/);
  assert.match(stylesSource, /@media \(prefers-reduced-motion: reduce\)/);
  assert.match(
    appSource,
    /<FeaturePolicyCard[\s\S]{0,200}isMacClient=\{status\.clientPlatform === "macos"\}/,
  );
  assert.match(previewSource, /gpuLaunchMode: "off" as const/);
});
