import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const { loadRequestLogModels } = await loadTypeScriptModule(new URL("../src/requestLogModels.ts", import.meta.url));

test("model candidates include low-frequency models beyond the statistics and page limits", async () => {
  const models = Array.from({ length: 251 }, (_, index) => `model-${String(index).padStart(3, "0")}`);
  const calls = [];
  const loaded = await loadRequestLogModels(async (cursor) => {
    calls.push(cursor);
    const start = cursor ? models.indexOf(cursor) + 1 : 0;
    const page = models.slice(start, start + 200);
    return { queryable: true, models: page, nextCursor: start + 200 < models.length ? page.at(-1) : null };
  }, () => true);
  assert.deepEqual(loaded, models);
  assert.deepEqual(calls, [undefined, "model-199"]);
});

test("closing or superseding a model query discards results and stops pagination", async () => {
  let active = true;
  let resolvePage;
  let calls = 0;
  const loading = loadRequestLogModels(() => {
    calls += 1;
    return new Promise((resolve) => { resolvePage = resolve; });
  }, () => active);
  active = false;
  resolvePage({ queryable: true, models: ["stale"], nextCursor: "stale" });
  assert.equal(await loading, null);
  assert.equal(calls, 1);
});

test("model query refuses non-advancing pagination instead of looping", async () => {
  await assert.rejects(loadRequestLogModels(async () => ({ queryable: true, models: ["same"], nextCursor: "same" }), () => true), /游标未前进/);
});
