import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/PluginConfigDialog.tsx", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
}).outputText;
const plugin = () => ({
  id: "demo", name: "Demo", version: "1", enabled: true, status: "running",
  config: { text: "saved", nested: { first: 1, second: 2 } },
  configSchema: { type: "object", properties: { text: { type: "string" } } },
  configUi: { type: "html", entry: "config.html", sha256: "first" },
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
    "react/jsx-runtime": { jsx, jsxs: jsx, Fragment: "fragment" },
    "./api": { invoke(command, args) {
      return new Promise((resolve, reject) => calls.push({ command, args, resolve, reject }));
    } },
    "./appUtils": { errorText: error => error.message },
    "./components/ui": Object.fromEntries(["Button", "Dialog", "DialogContent", "DialogDescription", "DialogHeader", "DialogTitle"].map(name => [name, name])),
    "./PluginHtmlConfig": { PluginHtmlConfig: "html-editor" },
    "./SchemaConfigForm": { SchemaConfigForm(props) {
      const [value, onEdit] = react.useState(props.value);
      return jsx("schema-editor", { ...props, value, onEdit });
    } },
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

async function loadHtml(harness, index = 0, html = "<p>config</p>", version = "1") {
  harness.calls[index].resolve({ pluginId: "demo", version, html });
  await flush(); harness.render();
}

test("equal plugin refresh preserves drafts and does not reload HTML", async () => {
  const initial = plugin(), harness = dialogHarness(initial);
  await loadHtml(harness);
  harness.find("html-editor")[0].props.onDraftChange({ text: "draft" });
  const refresh = { ...structuredClone(initial), status: "stopped", enabled: false, lastError: "runtime error" };
  refresh.config.nested = { second: 2, first: 1 };
  harness.render(refresh);
  assert.equal(harness.calls.length, 1);
  harness.find("html-editor")[0].props.onFailure("bridge failed");
  harness.render(); harness.find("Button")[0].props.onClick(); harness.render();
  assert.deepEqual(harness.find("schema-editor")[0].props.value, { text: "draft" });
  harness.find("schema-editor")[0].props.onEdit({ text: "fallback draft" });
  harness.render(structuredClone(refresh));
  assert.deepEqual(harness.find("schema-editor")[0].props.value, { text: "fallback draft" });
});

for (const [name, update] of [
  ["config", value => { value.config.text = "external"; }],
  ["version", value => { value.version = "2"; }],
  ["schema", value => { value.configSchema.properties.text.minLength = 2; }],
  ["UI digest", value => { value.configUi.sha256 = "second"; }],
  ["UI entry", value => { value.configUi.entry = "new.html"; }],
]) test(`${name} changes reset HTML, errors, fallback and draft state`, async () => {
  const initial = plugin(), harness = dialogHarness(initial);
  await loadHtml(harness);
  const editor = harness.find("html-editor")[0];
  editor.props.onDraftChange({ text: "old draft" });
  editor.props.onSave({ text: "old draft" });
  harness.calls[1].reject(new Error("save failed"));
  await flush(); harness.render();
  editor.props.onFailure("bridge failed");
  harness.render(); harness.find("Button")[0].props.onClick(); harness.render();
  const next = structuredClone(initial); update(next); harness.render(next);
  assert.equal(harness.find("schema-editor").length, 0);
  assert.equal(harness.find("html-editor").length, 0);
  assert.equal(harness.find("p").filter(node => node.props.role === "alert").length, 0);
  assert.equal(harness.calls.length, 3);
  await loadHtml(harness, 2, "<p>new config</p>", next.version);
  assert.equal(harness.find("html-editor")[0].props.html, "<p>new config</p>");
  harness.find("html-editor")[0].props.onFailure("new bridge failure");
  harness.render(); harness.find("Button")[0].props.onClick(); harness.render();
  assert.deepEqual(harness.find("schema-editor")[0].props.value, next.config);
});

test("schema-only drafts survive equal refresh and reset after external changes", () => {
  const initial = { ...plugin(), configUi: null }, harness = dialogHarness(initial);
  harness.find("schema-editor")[0].props.onEdit({ text: "draft" });
  harness.render(structuredClone(initial));
  assert.deepEqual(harness.find("schema-editor")[0].props.value, { text: "draft" });
  const next = { ...initial, config: { text: "external" } }; harness.render(next);
  assert.deepEqual(harness.find("schema-editor")[0].props.value, next.config);
});

test("removing HTML UI resets the fallback draft and cancels its pending load", async () => {
  const initial = plugin(), harness = dialogHarness(initial);
  const next = { ...initial, configUi: null, config: { text: "schema only" } };
  harness.render(next);
  await loadHtml(harness);
  assert.deepEqual(harness.find("schema-editor")[0].props.value, next.config);
  assert.equal(harness.staleWrites, 0);
});

test("late HTML responses cannot replace a newer configuration page", async () => {
  const initial = plugin(), harness = dialogHarness(initial);
  harness.render({ ...initial, config: { text: "external" } });
  assert.equal(harness.calls.length, 2);
  await loadHtml(harness, 1, "new page");
  await loadHtml(harness, 0, "old page");
  assert.equal(harness.find("html-editor")[0].props.html, "new page");
  assert.equal(harness.staleWrites, 0);
});

test("save blocks duplicate submissions and closing while pending", async () => {
  const harness = dialogHarness({ ...plugin(), configUi: null });
  const save = harness.find("schema-editor")[0].props.onSave;
  save({ text: "draft" }); save({ text: "duplicate" });
  harness.render();
  assert.equal(harness.calls.length, 1);
  assert.equal(harness.find("schema-editor")[0].props.disabled, true);
  harness.find("Dialog")[0].props.onOpenChange(false);
  let prevented = false;
  harness.find("DialogContent")[0].props.onEscapeKeyDown({ preventDefault() { prevented = true; } });
  assert.equal(prevented, true); assert.equal(harness.closed, 0);
  const result = { plugins: [] }; harness.calls[0].resolve(result);
  await flush();
  assert.deepEqual(harness.changed, [result]); assert.equal(harness.closed, 1);
});

for (const outcome of ["success", "failure"]) test(`configuration changes keep the save lock and ignore obsolete save ${outcome}`, async () => {
  const initial = { ...plugin(), configUi: null }, harness = dialogHarness(initial);
  harness.find("schema-editor")[0].props.onSave({ text: "old draft" });
  harness.render({ ...initial, config: { text: "external" } });
  assert.equal(harness.find("schema-editor")[0].props.disabled, true);
  harness.find("schema-editor")[0].props.onSave({ text: "new draft" });
  harness.find("Dialog")[0].props.onOpenChange(false);
  assert.equal(harness.calls.length, 1); assert.equal(harness.closed, 0);
  if (outcome === "success") harness.calls[0].resolve({ plugins: [initial] });
  else harness.calls[0].reject(new Error("obsolete failure"));
  await flush(); harness.render();
  assert.deepEqual(harness.changed, []); assert.equal(harness.closed, 0);
  assert.equal(harness.staleWrites, 0);
  assert.equal(harness.find("p").filter(node => node.props.role === "alert").length, 0);
  assert.equal(harness.find("schema-editor")[0].props.disabled, false);
  harness.find("schema-editor")[0].props.onSave({ text: "new draft" });
  assert.equal(harness.calls.length, 2);
  const result = { plugins: [{ ...initial, config: { text: "new draft" } }] };
  harness.calls[1].resolve(result); await flush();
  assert.deepEqual(harness.changed, [result]); assert.equal(harness.closed, 1);
});
