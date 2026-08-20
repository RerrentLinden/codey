import assert from "node:assert/strict";
import test from "node:test";

import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

async function loadModelIdHelpers() {
  return loadTypeScriptModule(
    new URL("../src/modelIds.ts", import.meta.url),
  );
}

test("model IDs compare case-insensitively while preserving first spelling", async () => {
  const {
    includesModelId,
    modelIdsEqual,
    modelKey,
    partitionModelIdsByKey,
    uniqueModelIds,
    withoutModelId,
  } = await loadModelIdHelpers();

  assert.equal(modelKey(" Provider-Coder "), "provider-coder");
  assert.equal(modelIdsEqual("Provider-Coder", " provider-coder "), true);
  assert.equal(
    includesModelId(["Provider-Coder", "Provider-Reasoner"], "PROVIDER-CODER"),
    true,
  );
  assert.deepEqual(
    uniqueModelIds([
      " Provider-Coder ",
      "provider-coder",
      "Provider-Reasoner",
      "",
    ]),
    ["Provider-Coder", "Provider-Reasoner"],
  );
  assert.deepEqual(
    withoutModelId(
      ["Provider-Coder", "Provider-Reasoner", "provider-coder"],
      " PROVIDER-CODER ",
    ),
    ["Provider-Reasoner"],
  );
  assert.deepEqual(
    partitionModelIdsByKey(
      ["Provider-Coder", "other-model", "PROVIDER-REASONER"],
      new Set(["provider-coder", "provider-reasoner"]),
    ),
    {
      matching: ["Provider-Coder", "PROVIDER-REASONER"],
      remaining: ["other-model"],
    },
  );
});
