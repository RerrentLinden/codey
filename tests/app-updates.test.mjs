import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/useAppUpdates.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
}).outputText;

function harness(enabled) {
  const hooks = [], effects = [], timers = new Map(), requests = [];
  let cursor = 0, timerId = 0, result;
  const options = {
    embedded: false, configLoaded: true, autoCheckCodeyUpdates: enabled,
    isBusy: false, setBusy() {}, setNotice() {}, setConfirmation() {}, beforeInstall: async () => {},
  };
  const react = {
    useState(initial) {
      const index = cursor++;
      if (!(index in hooks)) hooks[index] = initial;
      return [hooks[index], next => { hooks[index] = typeof next === "function" ? next(hooks[index]) : next; }];
    },
    useRef(initial) { return react.useState({ current: initial })[0]; },
    useCallback(callback) { return react.useState(callback)[0]; },
    useEffect(effect, deps) {
      const index = cursor++, previous = hooks[index];
      if (previous && deps.every((value, i) => Object.is(value, previous.deps[i]))) return;
      effects.push(() => {
        previous?.cleanup?.();
        hooks[index] = { deps, cleanup: effect() };
      });
    },
  };
  const window = new EventTarget();
  window.setTimeout = callback => { timers.set(++timerId, callback); return timerId; };
  window.clearTimeout = id => timers.delete(id);
  const exports = {};
  new Function("require", "exports", "window", compiled)(name => {
    if (name === "react") return react;
    if (name === "./api") return { invoke: () => new Promise((resolve, reject) => requests.push({ resolve, reject })) };
    if (name === "./appUtils") return { withTimeout: promise => promise, errorText: String };
    if (name === "./formatters") return { formatBytes: String };
    throw new Error(`Unexpected import ${name}`);
  }, exports, window);
  const render = () => {
    cursor = 0;
    result = exports.useAppUpdates(options);
    effects.splice(0).forEach(effect => effect());
    return result;
  };
  render();
  return { options, render, requests, timers, window };
}

const available = { currentVersion: "1.1.1", latestVersion: "1.2.0", updateAvailable: true };
const settle = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };

test("disabled automatic checks still allow a manual check", async () => {
  const h = harness(false);
  assert.equal(h.requests.length, 0);
  const checking = h.render().checkForUpdates();
  assert.equal(h.requests.length, 1);
  h.requests[0].resolve(available);
  await checking;
  assert.deepEqual(h.render().updateCheck, available);
});

test("disabling an in-flight automatic check ignores its result and clears pending", async () => {
  const h = harness(true);
  assert.equal(h.render().automaticallyChecking, true);
  h.options.autoCheckCodeyUpdates = false;
  h.render();
  assert.equal(h.render().automaticallyChecking, false);
  h.requests[0].resolve(available);
  await settle();
  assert.equal(h.render().updateCheck, null);
  assert.equal(h.window.__codeyUpdateAvailability, undefined);
  assert.equal(h.timers.size, 0);
});

test("disabling cancels the next automatic check and enabling starts again", async () => {
  const h = harness(true);
  h.requests[0].resolve({ ...available, updateAvailable: false });
  await settle();
  h.render();
  assert.equal(h.timers.size, 1);
  h.options.autoCheckCodeyUpdates = false;
  h.render();
  assert.equal(h.timers.size, 0);
  h.options.autoCheckCodeyUpdates = true;
  h.render();
  assert.equal(h.requests.length, 2);
});

test("detecting an update with an asset prompts the confirmation dialog", async () => {
  const h = harness(false);
  let confirmation = null;
  h.options.setConfirmation = (c) => {
    confirmation = c;
  };
  const updateWithAsset = {
    ...available,
    selectedAsset: { fileName: "Codey-1.2.0.dmg", size: 1048576, url: "https://example.com" },
  };
  const checking = h.render().checkForUpdates();
  assert.equal(h.requests.length, 1);
  h.requests[0].resolve(updateWithAsset);
  await checking;
  assert.ok(confirmation);
  assert.equal(confirmation.action, "download-update");
  assert.match(confirmation.title, /1\.2\.0/);
});

test("发现更新后定时器链仍在，可用更新被清空后继续自动检查", async () => {
  const h = harness(true);
  h.requests[0].resolve({ ...available, updateAvailable: true });
  await settle();
  h.render();
  // 暂停只影响本次检查：下一次 tick 必须仍然排程，否则手动清空状态后自动检查会永久失效。
  assert.equal(h.timers.size, 1, "发现可用更新后仍要保留下一次检查");

  const manual = h.render().checkForUpdates();
  h.requests[1].resolve({ currentVersion: "1.1.1", latestVersion: "1.1.1", updateAvailable: false });
  await manual;
  await settle();
  h.render();

  const [tick] = [...h.timers.values()];
  assert.ok(tick, "下一次检查必须已排程");
  h.timers.clear();
  tick();
  await settle();
  assert.equal(h.requests.length, 3, "状态清空后自动检查必须重新发起请求");
});
