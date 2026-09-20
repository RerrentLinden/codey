import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
const pluginHelpers = await loadTypeScriptModule(new URL("../src/codeyPlugins.ts", import.meta.url));
const documentHelpers = {};
const documentSource = await readFile(new URL("../src/pluginConfigDocument.ts", import.meta.url), "utf8");
new Function("require", "exports", ts.transpileModule(documentSource, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText)(name => { assert.equal(name, "./codeyPlugins"); return pluginHelpers; }, documentHelpers);

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
    "./pluginConfigDocument": documentHelpers,
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
const field = (h, name) => [...h.find("input"), ...h.find("textarea"), ...h.find("select")].find(node => node.props["aria-label"] === name);
const edit = (h, value, name = "text") => { field(h, name).props.onChange({ target: { value } }); h.render(); };

test("saves only changed values with digest and preserves annotations and whitespace", async () => {
  const h = dialogHarness(plugin());
  const original = '{ "_comments": { "text": "字段说明" }, "text" : "saved", "rules": [{ "_comments": { "x": "说明" }, "x": 1 }] }\n';
  await load(h, 0, original);
  assert.equal(field(h, "text").props.value, "saved");
  assert.equal(h.find("textarea").length, 0);
  edit(h, 'draft "quoted"\nline');
  const save = button(h, "保存配置").props.onClick; save(); save(); h.render();
  assert.equal(h.calls.length, 2);
  assert.deepEqual(h.calls[1].args, { pluginId: "demo", content: original.replace('"saved"', JSON.stringify('draft "quoted"\nline')), expectedSha256: hash });
  h.find("Dialog")[0].props.onOpenChange(false);
  assert.equal(h.closed, 0);
  const result = { plugins: [plugin()], platform: "linux", arch: "x86_64" };
  h.calls[1].resolve(result); await flush();
  assert.deepEqual(h.changed, [result]); assert.equal(h.closed, 1);
});

test("annotations are non-selectable text above immutable field labels", async () => {
  const h = dialogHarness(plugin()); await load(h, 0, '{"_comments":{"text":"说明第一行\\n说明第二行"},"text":"value","rules":[{"_comments":{"x":"嵌套说明"},"x":1}]}');
  const comment = h.find("p").find(node => node.props.children === "说明第一行\n说明第二行");
  assert.deepEqual(comment.props.style, { userSelect: "none", WebkitUserSelect: "none" });
  assert.equal(comment.props.contentEditable, undefined);
  const row = h.find("div").find(node => Array.isArray(node.props.children) && node.props.children[0]?.props?.id === comment.props.id);
  assert.ok(row);
  assert.equal(field(h, "text").props["aria-describedby"], comment.props.id);
  assert.ok(h.find("label").some(node => node.props.children === "text"));
  assert.ok(field(h, "rules.0.x"));
  assert.equal(h.find("input").length, 2);
  assert.equal(field(h, "_comments.text"), undefined);
});

test("malformed or unsupported files keep path and require external repair", async () => {
  for (const invalid of ["{", "[]", "null", '"text"', '{"_comments":[]}', '{"rules":[{"_comments":{"x":123}}]}']) {
    const h = dialogHarness(plugin()); await load(h, 0, invalid);
    assert.equal(h.find("input").length, 0); assert.equal(h.find("textarea").length, 0);
    assert.equal(button(h, "保存配置").props.disabled, true);
    assert.ok(h.find("p").some(node => node.props.children === "/demo/config.json"));
    assert.ok(h.find("p").some(node => node.props.role === "alert" && node.props.children.includes("修正配置文件后重新加载")));
    button(h, "保存配置").props.onClick(); assert.equal(h.calls.length, 1);
  }
});

test("scalar validation blocks invalid saves and preserves numeric tokens", async () => {
  const h = dialogHarness(plugin()); await load(h, 0, '{"count":9007199254740993,"enabled":true,"optional":null,"multi":"first\\nsecond","empty":[],"object":{}}');
  assert.equal(field(h, "count").props.value, "9007199254740993");
  assert.equal(field(h, "count").props.type, "text");
  assert.equal(field(h, "enabled").type, "select");
  assert.equal(field(h, "multi").type, "textarea");
  assert.equal(h.find("p").filter(n => n.props.children === "无可编辑值").length, 2);
  edit(h, "1e", "count");
  assert.equal(button(h, "保存配置").props.disabled, true);
  button(h, "保存配置").props.onClick(); assert.equal(h.calls.length, 1);
  edit(h, "9007199254740994", "count"); edit(h, "false", "enabled"); edit(h, '"value"', "optional");
  button(h, "保存配置").props.onClick();
  assert.ok(h.calls[1].args.content.includes('"count":9007199254740994'));
  assert.ok(h.calls[1].args.content.includes('"optional":"value"'));
});

test("dirty close and reload require confirmation and keep drafts", async () => {
  const h = dialogHarness(plugin()); await load(h); edit(h, "draft");
  h.find("Dialog")[0].props.onOpenChange(false); h.render();
  assert.equal(h.closed, 0); assert.equal(h.find("section").length, 1);
  button(h, "继续编辑").props.onClick(); h.render();
  button(h, "重新加载").props.onClick(); h.render();
  assert.equal(h.calls.length, 1);
  button(h, "放弃修改").props.onClick(); h.render();
  assert.equal(h.calls.length, 2);
  await load(h, 1, '{ "external": true }');
  assert.equal(field(h, "external").props.value, "true");
});

test("load errors can retry; save conflicts preserve draft", async () => {
  const h = dialogHarness(plugin()); h.calls[0].reject(new Error("read failed")); await flush(); h.render();
  button(h, "重试读取").props.onClick(); await load(h, 1);
  edit(h, "draft"); button(h, "保存配置").props.onClick();
  h.calls[2].reject(new Error("配置文件已被修改")); await flush(); h.render();
  assert.equal(field(h, "text").props.value, "draft");
  assert.equal(h.closed, 0); assert.equal(h.changed.length, 0);
});

test("failed reload invalidates old content and digest until a successful retry", async () => {
  const h = dialogHarness(plugin()); await load(h);
  button(h, "重新加载").props.onClick(); h.render();
  assert.equal(h.find("input").length, 0);
  assert.equal(button(h, "保存配置").props.disabled, true);
  h.calls[1].reject(new Error("read failed")); await flush(); h.render();
  assert.equal(h.find("input").length, 0);
  assert.equal(button(h, "保存配置").props.disabled, true);
  button(h, "保存配置").props.onClick(); assert.equal(h.calls.length, 2);
  button(h, "重试读取").props.onClick();
  h.calls[2].resolve({ pluginId: "demo", version: "1", path: "/demo/config.json", content: '{"fresh":true}', sha256: "b".repeat(64) });
  await flush(); h.render();
  assert.equal(field(h, "fresh").props.value, "true");
  edit(h, "false", "fresh"); button(h, "保存配置").props.onClick();
  assert.equal(h.calls[3].args.expectedSha256, "b".repeat(64));
});

for (const change of [{ version: "2" }, { id: "other" }]) test(`old reads ignored after identity change ${JSON.stringify(change)}`, async () => {
  const h = dialogHarness(plugin()); const next = { ...plugin(), ...change }; h.render(next);
  await load(h, 1, '{"new":true}', next.version, next.id);
  await load(h, 0, '{"old":true}');
  assert.equal(field(h, "new").props.value, "true"); assert.equal(h.staleWrites, 0);
});

for (const outcome of ["resolve", "reject"]) test(`old save ${outcome} ignored after version change`, async () => {
  const h = dialogHarness(plugin()); await load(h); edit(h, "draft"); button(h, "保存配置").props.onClick();
  h.render({ ...plugin(), version: "2" });
  if (outcome === "resolve") h.calls[1].resolve({ plugins: [], platform: "linux", arch: "x86_64" });
  else h.calls[1].reject(new Error("stale"));
  await flush(); h.render();
  assert.equal(h.changed.length, 0); assert.equal(h.closed, 0); assert.equal(h.staleWrites, 0);
});

test("runtime refresh preserves draft and restored value closes without warning", async () => {
  const h = dialogHarness(plugin()); await load(h); edit(h, "draft"); h.render({ ...plugin(), enabled: false });
  assert.equal(h.calls.length, 1); assert.equal(field(h, "text").props.value, "draft");
  edit(h, "saved"); button(h, "返回插件管理").props.onClick(); assert.equal(h.closed, 1);
});

test("keyboard save uses the latest value before a render", async () => {
  const h = dialogHarness(plugin()); await load(h);
  field(h, "text").props.onChange({ target: { value: "latest" } });
  let prevented = false;
  h.find("div").find(node => node.props.onKeyDown).props.onKeyDown({ ctrlKey: true, key: "s", preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  assert.equal(h.calls[1].args.content, text.replace("saved", "latest"));
});
