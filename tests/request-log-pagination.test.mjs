import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/RequestLogDialog.tsx", import.meta.url), "utf8");
const tree = ts.createSourceFile("RequestLogDialog.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let effectSource;
const visit = (node) => {
  if (ts.isCallExpression(node) && node.expression.getText(tree) === "useEffect"
    && node.getText(tree).includes('"query_route_request_logs"')) effectSource = node.getText(tree);
  ts.forEachChild(node, visit);
};
visit(tree);
assert.ok(effectSource);
const compiled = ts.transpileModule(effectSource, {}).outputText;

function createListHarness(invoke) {
  const state = { result: null, loading: false, cursors: {}, calls: [] };
  let dependencies;
  let cleanup;
  const context = {
    opened: true, validRange: true, refreshRevision: 0,
    filters: { cursorMode: true, fromUnixMs: 100, toUnixMs: 2000, provider: "provider-a" },
    page: 1, pageSize: 20, cursor: null,
    requestRevision: { current: 0 }, listTask: { current: Promise.resolve() },
    setResult: (value) => { state.result = value; },
    setLoading: (value) => { state.loading = value; },
    setCursors: (update) => { state.cursors = update(state.cursors); },
    setError: (message) => { if (message) assert.fail(message); }, errorText: String,
    invoke: async (command, args) => {
      assert.equal(command, "query_route_request_logs");
      state.calls.push(args);
      return invoke(args);
    },
    useEffect: (callback, nextDependencies) => {
      if (dependencies && nextDependencies.every((value, index) => Object.is(value, dependencies[index]))) return;
      cleanup?.();
      dependencies = nextDependencies;
      cleanup = callback();
    },
  };
  return {
    state,
    render(page) {
      context.page = page;
      context.cursor = page === 1 ? null : state.cursors[page] ?? null;
      new Function(...Object.keys(context), compiled)(...Object.values(context));
      return context.listTask.current;
    },
  };
}

test("jumping to an unvisited page makes one page query and caches the next cursor", async () => {
  const nextCursor = { timestampUnixMs: 900, requestId: "page-419-last" };
  const harness = createListHarness(async ({ page }) => ({ items: [{ requestId: `page-${page}` }], nextCursor }));
  await harness.render(419);
  assert.deepEqual(harness.state.calls, [{
    cursorMode: false, fromUnixMs: 100, toUnixMs: 2000, provider: "provider-a",
    page: 419, pageSize: 20, cursor: null,
  }]);
  assert.equal(harness.state.result.items[0].requestId, "page-419");
  await harness.render(420);
  assert.equal(harness.state.calls[1].cursorMode, true);
  assert.deepEqual(harness.state.calls[1].cursor, nextCursor);
  assert.deepEqual(Object.keys(harness.state.cursors), ["420", "421"]);
});

test("consecutive jumps without cached cursors still reload, and the first page uses a null cursor", async () => {
  const harness = createListHarness(async ({ page }) => ({ items: [{ requestId: `page-${page}` }], nextCursor: null }));
  await harness.render(419);
  await harness.render(200);
  await harness.render(1);
  assert.deepEqual(harness.state.calls.map(({ page, cursorMode }) => [page, cursorMode]), [[419, false], [200, false], [1, true]]);
  assert.ok(harness.state.calls.every(({ cursor }) => cursor === null));
  assert.equal(harness.state.result.items[0].requestId, "page-1");
});

test("a superseded jump cannot overwrite the latest page or its cached cursor", async () => {
  let resolveOldPage;
  const harness = createListHarness(({ page }) => page === 419
    ? new Promise((resolve) => { resolveOldPage = resolve; })
    : Promise.resolve({ items: [{ requestId: `page-${page}` }], nextCursor: null }));
  const oldQuery = harness.render(419);
  await Promise.resolve();
  const latestQuery = harness.render(200);
  resolveOldPage({ items: [{ requestId: "stale" }], nextCursor: { timestampUnixMs: 100, requestId: "stale" } });
  await Promise.all([oldQuery, latestQuery]);
  assert.equal(harness.state.result.items[0].requestId, "page-200");
  assert.deepEqual(harness.state.cursors, {});
  assert.equal(harness.state.loading, false);
});
