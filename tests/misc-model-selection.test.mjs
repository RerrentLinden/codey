import assert from "node:assert/strict";
import test from "node:test";

import { readSource } from "./helpers/read-source.mjs";

test("misc model setting is part of the saved configuration", async () => {
  const [typesSource, mockApiSource] = await Promise.all([
    readSource("src/App.types.ts"),
    readSource("src/dev/mockApi.ts"),
  ]);

  assert.match(typesSource, /miscModel: string;/);
  assert.match(mockApiSource, /miscModel: "",/);
});

test("misc model card drives naming, commit messages, and the review fallback", async () => {
  const cardSource = await readSource("src/MiscModelCard.tsx");

  assert.match(cardSource, /<ModelCombobox/);
  assert.match(cardSource, /aria-label="杂事模型"/);
  assert.match(cardSource, /恢复默认/);
  assert.match(cardSource, /codex-auto-review/);
  assert.match(cardSource, /misc-model-field/);
  assert.match(cardSource, /misc-model-body/);
  assert.match(cardSource, /miscModel: ""/);
  assert.match(cardSource, /miscModel: value/);
  assert.match(cardSource, /onConfigChange/);
});

test("misc model card sits between the prompt grid and the feature policy card", async () => {
  const appSource = await readSource("src/App.tsx");

  const gridIndex = appSource.indexOf('className="prompt-subagent-grid"');
  const cardIndex = appSource.indexOf("<MiscModelCard");
  const policyIndex = appSource.indexOf("<FeaturePolicyCard");
  assert.ok(gridIndex >= 0 && cardIndex >= 0 && policyIndex >= 0);
  assert.ok(gridIndex < cardIndex && cardIndex < policyIndex);
  assert.match(appSource, /import \{ MiscModelCard \} from "\.\/MiscModelCard";/);
});
