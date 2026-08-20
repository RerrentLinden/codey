import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("subagent settings expose the five supported role controls", async () => {
  const [featurePolicySource, modelHookSource] = await Promise.all([
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
  ]);

  assert.match(featurePolicySource, /checked=\{config\.subagentOptimization\}/);
  assert.match(
    featurePolicySource,
    /onCheckedChange=\{\(checked\) =>\s*onSubagentOptimizationChange\(checked\)\s*\}/,
  );
  for (const [id, name] of [
    ["codey_quick_scan", "快速定位"],
    ["codey_deep_research", "深度检索"],
    ["codey_visual_analysis", "视觉分析"],
    ["codey_worker", "代码实施"],
    ["codey_visual_worker", "视觉实施"],
  ]) {
    assert.match(featurePolicySource, new RegExp(`id: "${id}"`));
    assert.match(featurePolicySource, new RegExp(`name: "${name}"`));
  }
  assert.match(featurePolicySource, /config\.subagentRoles\[task\.id\]/);
  assert.match(
    modelHookSource,
    /modelState\.officialModels\s*\.filter\(\(model\) => model\.supported\)/,
  );
});
