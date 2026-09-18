import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/codeyPlugins.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText;
const { validatePluginConfig, pluginStatusLabel, applyPluginDefaults, initialPluginValue, setPluginProperty } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

test("plugin configuration rejects invalid values before saving", () => {
  const schema = {
    type: "object", additionalProperties: false, required: ["count", "routes"],
    properties: {
      count: { type: "integer", minimum: 1, maximum: 4 },
      routes: { type: "array", minItems: 1, maxItems: 2, items: { type: "string", minLength: 1 } },
      strategy: { type: "string", enum: ["random", "round_robin"] },
    },
  };
  assert.deepEqual(validatePluginConfig({ count: 2, routes: ["route-a"] }, schema), []);
  for (const value of [
    {}, { count: 1.5, routes: ["a"] }, { count: 5, routes: ["a"] },
    { count: 1, routes: [] }, { count: 1, routes: ["a", "b", "c"] },
    { count: 1, routes: [false] }, { count: 1, routes: ["a"], strategy: "unknown" },
    { count: 1, routes: ["a"], extra: true },
  ]) assert.ok(validatePluginConfig(value, schema).length > 0, JSON.stringify(value));
});

test("frontend schema checks match Unicode and null semantics", () => {
  assert.deepEqual(validatePluginConfig("😀", { type: "string", minLength: 1, maxLength: 1 }), []);
  assert.deepEqual(validatePluginConfig(null, { type: "null" }), []);
  assert.ok(validatePluginConfig("null", { type: "null" }).length);
  assert.ok(validatePluginConfig(JSON.parse('{"toString":1}'), {
    type: "object", properties: {}, additionalProperties: false,
  }).length);
});

test("runtime errors remain visible when an upgrade is pending", () => {
  assert.equal(pluginStatusLabel({ enabled: true, restartRequired: true, lastError: "failed" }), "运行异常");
  assert.equal(pluginStatusLabel({ enabled: true, restartRequired: true }), "待重新启用");
  assert.equal(pluginStatusLabel({ enabled: false }), "已停用");
});

test("defaults preserve unknown configuration and do not mutate input", () => {
  const schema = { type: "object", properties: { count: { type: "integer", default: 3 }, pools: { type: "array", items: { type: "object", properties: { enabled: { type: "boolean", default: true } } } } } };
  const input = { extra: 7, pools: [{}] };
  assert.deepEqual(applyPluginDefaults(input, schema), { extra: 7, count: 3, pools: [{ enabled: true }] });
  assert.deepEqual(input, { extra: 7, pools: [{}] });
  assert.deepEqual(initialPluginValue(schema), { count: 3 });
  assert.deepEqual(setPluginProperty({ count: 3, extra: 7 }, "count", undefined), { extra: 7 });
  assert.ok(validatePluginConfig(initialPluginValue({ type: "integer" }), { type: "integer" }).length);
});

test("enum comparison ignores object key order and keeps array order", () => {
  const schema = { type: "object", enum: [{ first: 1, second: [2, 3] }] };
  assert.deepEqual(validatePluginConfig({ second: [2, 3], first: 1 }, schema), []);
  assert.ok(validatePluginConfig({ second: [3, 2], first: 1 }, schema).length);
});

test("prototype-like field names are own data and required fields cannot be inherited", () => {
  const schema = JSON.parse('{"type":"object","required":["toString"],"properties":{"toString":{"type":"string"},"__proto__":{"type":"object","default":{"value":1}}}}');
  assert.ok(validatePluginConfig({}, schema).length);
  const config = applyPluginDefaults({}, schema);
  assert.equal(Object.getPrototypeOf(config), Object.prototype);
  assert.ok(Object.prototype.hasOwnProperty.call(config, "__proto__"));
  assert.deepEqual(config.__proto__, { value: 1 });
  assert.deepEqual(validatePluginConfig(setPluginProperty(config, "toString", "value"), schema), []);
});

test("object arrays report nested required, bounds, and retained unknown fields", () => {
  const schema = { type: "array", minItems: 1, maxItems: 2, items: { type: "object", required: ["url"], additionalProperties: false, properties: { url: { type: "string", minLength: 1 }, weight: { type: "integer", minimum: 1 } } } };
  assert.deepEqual(validatePluginConfig([{ url: "http://localhost", weight: 1 }], schema), []);
  assert.ok(validatePluginConfig([{ weight: "", legacy: true }], schema).some(error => error.includes("[0].url")));
  assert.equal(validatePluginConfig([{ weight: "", legacy: true }], schema).length, 3);
});
