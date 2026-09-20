import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
const pluginHelpers = await loadTypeScriptModule(new URL("../src/codeyPlugins.ts", import.meta.url));

const source = await readFile(new URL("../src/PluginConfigDialog.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const plugin = () => ({
  id: "demo", name: "Demo", version: "1", enabled: true, status: "running",
  capabilities: [],
  configPath: "/demo/config.json",
});
const flush = () => new Promise(resolve => setImmediate(resolve));

// Model hook state, keyed component identity and effect cleanup without a DOM dependency.
function dialogHarness(initial) {
  const fibers = new Map(), calls = [], changed = [];
  let current, cursor, tree, input = initial, closed = 0, staleWrites = 0;
  const react = {
    useState(initialValue) {
      const fiber = current, index = cursor++;
      if (!(index in fiber.hooks)) fiber.hooks[index] = typeof initialValue === "function" ? initialValue() : initialValue;
      return [fiber.hooks[index], value => {
        if (!fiber.mounted) { staleWrites++; return; }
        fiber.hooks[index] = typeof value === "function" ? value(fiber.hooks[index]) : value;
      }];
    },
    useRef(value) {
      const index = cursor++;
      return current.hooks[index] ??= { current: value };
    },
    useEffect(effect, deps) {
      const fiber = current, index = cursor++, previous = fiber.hooks[index];
      if (previous && deps.every((value, i) => Object.is(value, previous.deps[i]))) return;
      fiber.effects.push(() => {
        previous?.cleanup?.();
        fiber.hooks[index] = { deps, cleanup: effect() };
      });
    },
  };
  const jsx = (type, props, key) => ({ type, props, key });
  const modules = {
    react,
    "./codeyPlugins": pluginHelpers,
    "react/jsx-runtime": { jsx, jsxs: jsx, Fragment: "fragment" },
    "./api": { invoke(command, args) {
      return new Promise((resolve, reject) => calls.push({ command, args, resolve, reject }));
    } },
    "./appUtils": { errorText: error => error.message },
    "./components/ui": Object.fromEntries(["Button", "Dialog", "DialogContent", "DialogDescription", "DialogHeader", "DialogTitle"].map(name => [name, name])),
  };
  const exports = {};
  new Function("require", "exports", compiled)(name => {
    assert.ok(name in modules, `unexpected import: ${name}`);
    return modules[name];
  }, exports);
  const cleanup = fiber => {
    fiber.mounted = false;
    fiber.hooks.forEach(hook => hook?.cleanup?.());
  };
  function render(next = input) {
    input = next;
    const visited = new Set();
    function visit(node, path) {
      if (Array.isArray(node)) return node.map((child, index) => visit(child, `${path}.${index}`));
      if (!node || typeof node !== "object") return node;
      const address = `${path}:${node.key ?? ""}`;
      if (typeof node.type === "function") {
        let fiber = fibers.get(address);
        if (fiber && fiber.type !== node.type) { cleanup(fiber); fiber = null; }
        if (!fiber) { fiber = { type: node.type, hooks: [], effects: [], mounted: true }; fibers.set(address, fiber); }
        visited.add(address);
        current = fiber; cursor = 0;
        return visit(node.type(node.props), `${address}.child`);
      }
      return { ...node, props: { ...node.props, children: visit(node.props.children, `${address}.children`) } };
    }
    tree = visit(jsx(exports.PluginConfigDialog, { plugin: input, onClose: () => closed++, onChanged: result => changed.push(result) }), "root");
    for (const [address, fiber] of fibers) if (!visited.has(address)) { cleanup(fiber); fibers.delete(address); }
    for (const fiber of fibers.values()) for (const effect of fiber.effects.splice(0)) effect();
    return tree;
  }
  function find(type) {
    const walk = node => {
      if (Array.isArray(node)) return node.flatMap(walk);
      if (!node || typeof node !== "object") return [];
      return [...(node.type === type ? [node] : []), ...walk(node.props.children)];
    };
    return walk(tree);
  }
  render();
  return { render, find, calls, changed, get closed() { return closed; }, get staleWrites() { return staleWrites; } };
}

const hash = "a".repeat(64);
const text = '{\n  "text": "saved"\n}\n';
async function load(harness, index = 0, content = text, version = "1", pluginId = "demo") {
  harness.calls[index].resolve({ pluginId, version, path: "/demo/config.json", content, sha256: hash });
  await flush(); harness.render();
}
const button = (h, name) => h.find("Button").find(node => node.props.children === name);
const edit = (h, content) => { h.find("textarea")[0].props.onChange({ target: { value: content } }); h.render(); };

test("loads and saves file text verbatim with digest, rejects duplicate saves", async () => {
  const h = dialogHarness(plugin()); await load(h);
  assert.equal(h.find("textarea")[0].props.value, text);
  const draft = '{ "_comments": { "text": "字段说明" }, "text" : "draft", "rules": [{ "_comments": { "x": "说明" }, "x": 1 }] }\n'; edit(h, draft);
  const save = button(h, "保存配置").props.onClick; save(); save(); h.render();
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1].args, { pluginId: "demo", content: draft, expectedSha256: hash });
  h.find("Dialog")[0].props.onOpenChange(false);
  assert.equal(h.closed, 0);
  const result = { plugins: [plugin()], platform: "linux", arch: "x86_64" };
  h.calls[1].resolve(result); await flush();
  assert.deepEqual(h.changed, [result]); assert.equal(h.closed, 1);
});

test("loads malformed JSON for repair and validates before saving", async () => {
  const h = dialogHarness(plugin()); await load(h, 0, '{ broken');
  assert.equal(h.find("textarea")[0].props.value, '{ broken');
  for (const invalid of ["{", "[]", "null", '"text"', '{"_comments":[]}', '{"rules":[{"_comments":{"x":123}}]}']) {
    edit(h, invalid); button(h, "保存配置").props.onClick(); h.render();
    assert.equal(h.calls.length, 1);
    assert.ok(h.find("p").some(n => n.props.role === "alert"));
  }
});

test("dirty close and reload require confirmation and keep drafts", async () => {
  const h = dialogHarness(plugin()); await load(h); edit(h, '{ "draft": true }');
  h.find("Dialog")[0].props.onOpenChange(false); h.render();
  assert.equal(h.closed, 0); assert.equal(h.find("section").length, 1);
  button(h, "继续编辑").props.onClick(); h.render();
  button(h, "重新加载").props.onClick(); h.render();
  assert.equal(h.calls.length, 1);
  button(h, "放弃修改").props.onClick(); h.render();
  assert.equal(h.calls.length, 2);
  await load(h, 1, '{ "external": true }');
  assert.equal(h.find("textarea")[0].props.value, '{ "external": true }');
});

test("load errors can retry; save conflicts preserve draft", async () => {
  const h = dialogHarness(plugin()); h.calls[0].reject(new Error("read failed")); await flush(); h.render();
  button(h, "重试读取").props.onClick(); await load(h, 1);
  edit(h, '{"draft":true}'); button(h, "保存配置").props.onClick();
  h.calls[2].reject(new Error("配置文件已被修改")); await flush(); h.render();
  assert.equal(h.find("textarea")[0].props.value, '{"draft":true}');
  assert.equal(h.closed, 0); assert.equal(h.changed.length, 0);
});

test("failed reload invalidates the old content and digest until a successful retry", async () => {
  const h = dialogHarness(plugin()); await load(h);
  button(h, "重新加载").props.onClick(); h.render();
  assert.equal(h.find("textarea").length, 0);
  assert.equal(button(h, "保存配置").props.disabled, true);
  h.calls[1].reject(new Error("read failed")); await flush(); h.render();
  assert.equal(h.find("textarea").length, 0);
  assert.equal(button(h, "保存配置").props.disabled, true);
  button(h, "保存配置").props.onClick();
  assert.equal(h.calls.length, 2);
  button(h, "重试读取").props.onClick();
  h.calls[2].resolve({ pluginId: "demo", version: "1", path: "/demo/config.json", content: '{"fresh":true}', sha256: "b".repeat(64) });
  await flush(); h.render();
  assert.equal(h.find("textarea")[0].props.value, '{"fresh":true}');
  edit(h, '{"fresh":false}'); button(h, "保存配置").props.onClick();
  assert.equal(h.calls[3].args.expectedSha256, "b".repeat(64));
});

for (const change of [{ version: "2" }, { id: "other" }]) test(`old reads ignored after identity change ${JSON.stringify(change)}`, async () => {
  const h = dialogHarness(plugin()); const next = { ...plugin(), ...change }; h.render(next);
  await load(h, 1, '{"new":true}', next.version, next.id);
  await load(h, 0, '{"old":true}');
  assert.equal(h.find("textarea")[0].props.value, '{"new":true}'); assert.equal(h.staleWrites, 0);
});

for (const outcome of ["resolve", "reject"]) test(`old save ${outcome} ignored after version change`, async () => {
  const h = dialogHarness(plugin()); await load(h); edit(h, '{"draft":true}'); button(h, "保存配置").props.onClick();
  h.render({ ...plugin(), version: "2" });
  if (outcome === "resolve") h.calls[1].resolve({ plugins: [], platform: "linux", arch: "x86_64" });
  else h.calls[1].reject(new Error("stale"));
  await flush(); h.render();
  assert.equal(h.changed.length, 0); assert.equal(h.closed, 0); assert.equal(h.staleWrites, 0);
});

test("runtime refresh preserves draft and same text closes without warning", async () => {
  const h = dialogHarness(plugin()); await load(h); edit(h, '{"draft":true}'); h.render({ ...plugin(), enabled: false });
  assert.equal(h.calls.length, 1); assert.equal(h.find("textarea")[0].props.value, '{"draft":true}');
  edit(h, text); button(h, "返回插件管理").props.onClick(); assert.equal(h.closed, 1);
});
