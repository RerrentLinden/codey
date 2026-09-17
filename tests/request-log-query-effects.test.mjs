import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const { loadRequestLogModels } = await loadTypeScriptModule(new URL("../src/requestLogModels.ts", import.meta.url));
const source = await readFile(new URL("../src/RequestLogDialog.tsx", import.meta.url), "utf8");
const tree = ts.createSourceFile("RequestLogDialog.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const effects = [];
const visit = (node) => {
  if (ts.isCallExpression(node) && node.expression.getText(tree) === "useEffect") {
    const text = node.getText(tree);
    if (text.includes("loadRequestLogModels") || text.includes('"query_route_request_log_stats"')) effects.push(text);
  }
  ts.forEachChild(node, visit);
};
visit(tree);
assert.equal(effects.length, 2);

test("model and statistics effects load all candidates with only one statistics request", async () => {
  const calls = [];
  let candidates;
  let statistics;
  const context = {
    opened: true, validRange: true, fromUnixMs: 1, toUnixMs: 2000,
    provider: "provider-a", officialAccount: "account-a", refreshRevision: 0,
    filters: { fromUnixMs: 1, toUnixMs: 2000 }, groupBy: "model",
    modelsTask: { current: Promise.resolve() }, statsTask: { current: Promise.resolve() },
    useEffect: (callback) => callback(),
    optionalFilter: (value) => value !== "all",
    loadRequestLogModels,
    setUsedModels: (value) => { candidates = value; },
    setStats: (value) => { statistics = value; },
    setStatsLoading: () => {}, setStatsError: () => {},
    setError: (error) => assert.fail(error), errorText: String,
    invoke: async (command, args) => {
      calls.push({ command, args });
      if (command === "query_route_request_log_stats") return { queryable: true, total: 123, groups: [] };
      assert.equal(command, "query_route_request_log_models");
      assert.equal(args.provider, "provider-a");
      assert.equal(args.officialAccountId, "account-a");
      return args.afterModel
        ? { queryable: true, models: ["model-z"], nextCursor: null }
        : { queryable: true, models: ["model-a"], nextCursor: "model-a" };
    },
  };
  const compiled = ts.transpileModule(effects.join(";\n"), {}).outputText;
  new Function(...Object.keys(context), compiled)(...Object.values(context));
  await Promise.all([context.modelsTask.current, context.statsTask.current]);
  assert.equal(calls.filter(({ command }) => command === "query_route_request_log_stats").length, 1);
  assert.equal(calls.filter(({ command }) => command === "query_route_request_log_models").length, 2);
  assert.deepEqual(candidates, ["model-a", "model-z"]);
  assert.equal(statistics.total, 123);
});
