import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const root = new URL("../", import.meta.url);
const shieldSource = await readFile(
  new URL("public/fast-startup-shield.js", root),
  "utf8",
);
const bridgeSource = await readFile(
  new URL("public/codey-bridge.js", root),
  "utf8",
);

function preparedShield({ enabled = true, timeoutMs = 20 } = {}) {
  return shieldSource
    .replaceAll("__CODEY_FAST_CODEX_STARTUP__", enabled ? "true" : "false")
    .replaceAll("__CODEY_STATSIG_TIMEOUT_MS__", String(timeoutMs));
}

function createWindow(fetchImpl, statsig = undefined) {
  return {
    __STATSIG__: statsig,
    location: { href: "https://localhost/" },
    fetch: fetchImpl,
    setTimeout(callback, delay, ...args) {
      if (delay >= 30000) return null;
      return setTimeout(callback, delay, ...args);
    },
    clearTimeout,
    setInterval: () => 1,
    clearInterval: () => {},
  };
}

function runShield(window, options = {}) {
  const context = vm.createContext({
    window,
    URL,
    AbortController,
    Promise,
    WeakSet,
    Object,
    Date,
    Set,
    globalThis: window,
  });
  vm.runInContext(bridgeSource, context);
  vm.runInContext(preparedShield(options), context);
  return window.__codeyFastStartupShield;
}

test("fast startup shield is enabled by default and exposed as a restart-safe setting", async () => {
  const [configSource, commandSource, launcherSource, cdpSource, uiSource] = await Promise.all([
    readFile(new URL("backend/src/config.rs", root), "utf8"),
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
    readFile(new URL("backend/src/launcher.rs", root), "utf8"),
    readFile(new URL("backend/src/cdp.rs", root), "utf8"),
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
  ]);

  assert.match(configSource, /pub fast_codex_startup: bool/);
  assert.match(configSource, /fast_codex_startup: true/);
  assert.match(commandSource, /config\.fast_codex_startup = config_input\.fast_codex_startup/);
  assert.match(commandSource, /applied\.fast_codex_startup != current\.fast_codex_startup/);
  assert.match(launcherSource, /config\.fast_codex_startup/);
  assert.match(cdpSource, /FAST_STARTUP_STATSIG_TIMEOUT_MS: u64 = 1500/);
  assert.match(uiSource, /checked=\{config\.fastCodexStartup\}/);
  assert.match(uiSource, /远程启动配置超过 1\.5 秒时快速降级/);
});

test("fast startup shield leaves unrelated and responsive fetches unchanged", async () => {
  const calls = [];
  const response = { ok: true };
  const window = createWindow(async (input, init) => {
    calls.push({ input, init });
    return response;
  });
  const state = runShield(window);

  assert.equal(await window.fetch("https://example.com/data", { method: "POST" }), response);
  assert.equal(await window.fetch("https://ab.chatgpt.com/bootstrap"), response);
  assert.equal(calls.length, 2);
  assert.equal(calls[0].init.method, "POST");
  assert.ok(calls[1].init.signal instanceof AbortSignal);
  assert.equal(state.statsigFetches, 1);
  assert.equal(state.statsigTimeouts, 0);
});

test("fast startup shield aborts only a slow Statsig request and releases its client", async () => {
  const events = [];
  const client = {
    loadingStatus: "Loading",
    $emt(event) {
      events.push(event);
    },
  };
  const window = createWindow((_input, init) => new Promise((_resolve, reject) => {
    init.signal.addEventListener("abort", () => reject(new Error("aborted")), { once: true });
  }), { firstInstance: client });
  const state = runShield(window);

  await assert.rejects(window.fetch("https://api.statsigcdn.com/bootstrap"), /aborted/);
  assert.equal(state.statsigTimeouts, 1);
  assert.equal(client.loadingStatus, "Ready");
  assert.equal(events.length, 1);
  assert.equal(events[0].name, "values_updated");
});

test("fast startup shield patches a Statsig client that exposes initializeAsync late", async () => {
  const intervalCallbacks = [];
  const client = { loadingStatus: "Loading" };
  const window = createWindow(async () => ({ ok: true }), { firstInstance: client });
  window.setInterval = (callback) => {
    intervalCallbacks.push(callback);
    return 1;
  };
  const state = runShield(window);

  client.initializeAsync = () => new Promise(() => {});
  intervalCallbacks[0]();
  assert.equal(await client.initializeAsync(), null);
  assert.equal(state.clientFallbacks, 1);
  assert.equal(client.loadingStatus, "Ready");
});

test("fast startup shield can be disabled without wrapping fetch", async () => {
  const originalFetch = async () => ({ ok: true });
  const window = createWindow(originalFetch);
  const state = runShield(window, { enabled: false });

  assert.equal(window.fetch, originalFetch);
  assert.equal(state.enabled, false);
  assert.equal(state.installed, false);
});

test("fast startup shield restores native fetch after the startup window", () => {
  const restoreCallbacks = [];
  const originalFetch = async () => ({ ok: true });
  const window = createWindow(originalFetch);
  window.setTimeout = (callback, delay, ...args) => {
    if (delay >= 30000) {
      restoreCallbacks.push(callback);
      return null;
    }
    return setTimeout(callback, delay, ...args);
  };
  const state = runShield(window);

  assert.notEqual(window.fetch, originalFetch);
  assert.equal(state.active, true);
  restoreCallbacks[0]();
  assert.equal(window.fetch, originalFetch);
  assert.equal(state.active, false);
});
