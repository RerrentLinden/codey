import assert from "node:assert/strict";
import test from "node:test";

import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const paginationModule = new URL(
  "../src/modelPickerPagination.ts",
  import.meta.url,
);

test("model picker filters case-insensitively and pages bounded results", async () => {
  const {
    MODEL_PICKER_PAGE_SIZE,
    filterModelOptions,
    nextVisibleModelCount,
    visibleModelOptions,
  } = await loadTypeScriptModule(paginationModule);
  const models = Array.from({ length: 450 }, (_, index) =>
    index % 2 === 0 ? `Provider-${index}` : `Other-${index}`
  );

  assert.equal(MODEL_PICKER_PAGE_SIZE, 200);
  assert.equal(filterModelOptions(models, "  PROVIDER-2  ")[0], "Provider-2");
  assert.equal(filterModelOptions(models, "provider").length, 225);
  assert.equal(filterModelOptions(models, ""), models);

  const firstPage = visibleModelOptions(models, MODEL_PICKER_PAGE_SIZE);
  assert.equal(firstPage.length, 200);
  assert.equal(nextVisibleModelCount(firstPage.length, models.length), 400);
  assert.equal(nextVisibleModelCount(400, models.length), 450);
  assert.equal(nextVisibleModelCount(450, models.length), 450);
});
