import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { readSource } from "./helpers/read-source.mjs";

const source = await readSource("public/plugin-marketplace-fix.js");

function setup({ lateBridge = false } = {}) {
  const calls = [];
  const localCalls = [];
  const timers = new Map();
  let timerId = 0;
  let interceptor;
  const response = { plugins: [{ id: "remote", name: "remote", hidden: true }] };
  const nativePromise = Promise.resolve(response);
  const electronBridge = {
    sendMessageFromView(...args) {
      calls.push(args);
      return nativePromise;
    },
  };
  const window = {
    __codeyCall(...args) {
      localCalls.push(args);
      return Promise.resolve({ plugins: [{ id: "local", name: "local", marketplace: "local" }] });
    },
    __codeySharedRuntime: {
      registerFetchInterceptor(_name, callback) { interceptor = callback; },
    },
    clearTimeout(id) { timers.delete(id); },
    setTimeout(callback, delay) {
      timers.set(++timerId, { callback, delay });
      return timerId;
    },
    dispatchEvent() {},
  };
  if (!lateBridge) window.electronBridge = electronBridge;
  const sandbox = { window, CustomEvent: class {}, Headers, Response };
  vm.runInNewContext(source, sandbox);
  return { window, calls, localCalls, timers, nativePromise, response, electronBridge, interceptor, sandbox };
}

test("chat text, paths and arbitrary objects never activate plugin IPC patches", async () => {
  const fixture = setup();
  const { window, calls, localCalls, nativePromise, response } = fixture;
  const messages = [
    { channel: "thread-update", payload: { text: "install-plugin and list-plugins", includeHidden: false } },
    { method: "turn/start", params: { input: "plugin marketplace", includeRemote: false } },
    { method: "turn/start", payload: { method: "list-plugins", includeHidden: false } },
    { type: "invoke", method: "turn/start", payload: { method: "list-plugins" } },
    { type: "mcp-request", request: { method: "tools/call", params: { name: "install-plugin" } } },
    { type: "invoke", payload: { method: "tools/call", request: { method: "list-plugins" } } },
    { description: "plugin", plugin: { includeHidden: false } },
    { payload: { text: "marketplace", file: "/workspace/plugins/example.md" } },
    { metadata: { method: "list-plugins", path: "/plugins/list" } },
    { method: "get-plugin-documentation", options: { includeHidden: false } },
    { method: "plugin-helper/list", options: { includeHidden: false } },
  ];
  for (const message of messages) {
    const result = window.electronBridge.sendMessageFromView(message);
    assert.equal(result, nativePromise, JSON.stringify(message));
    assert.equal(calls.at(-1)[0], message);
    assert.equal(await result, response);
  }
  const payload = { method: "list-plugins", includeHidden: false };
  assert.equal(window.electronBridge.sendMessageFromView("turn/start", payload), nativePromise);
  assert.equal(calls.at(-1)[1], payload);
  assert.equal(localCalls.length, 0);
  assert.equal(response.plugins[0].hidden, true);
});

test("explicit plugin commands and nested transport envelopes retain normalization and local lists", async () => {
  for (const message of [
    { channel: "list-plugins", options: { includeHidden: false, includeRemote: false } },
    { method: "plugin/list", options: { includeHidden: false, includeRemote: false } },
    { path: "/api/plugins/list", options: { includeHidden: false, includeRemote: false } },
    { type: "invoke", payload: { request: { method: "list-plugins", options: { includeHidden: false } } } },
    { type: "mcp-request", request: { method: "plugin/list", params: { includeRemote: false } } },
    { body: JSON.stringify({ method: "plugin/list" }) },
    JSON.stringify({ method: "list-plugins", options: { includeHidden: false } }),
  ]) {
    const { window, localCalls, calls } = setup();
    const response = await window.electronBridge.sendMessageFromView(message);
    assert.equal(localCalls.length, 1, JSON.stringify(message));
    assert.equal(localCalls[0][0], "/plugins/list");
    assert.equal(response.plugins[0].hidden, false);
    assert.equal(response.plugins.some((plugin) => plugin.id === "local"), true);
    assert.doesNotMatch(JSON.stringify(calls[0]), /"include(?:Hidden|Remote)":false/);
  }
  const { window, localCalls, calls } = setup();
  await window.electronBridge.sendMessageFromView("list-plugins", { includeHidden: false });
  await window.electronBridge.sendMessageFromView("invoke", { method: "plugin/list" });
  assert.equal(localCalls.length, 2);
  assert.equal(calls[0][1].includeHidden, true);
});

test("read and mutation commands patch results while only mutations trigger a refresh", async () => {
  const { window, localCalls } = setup();
  const read = await window.electronBridge.sendMessageFromView({ method: "plugin/read" });
  assert.equal(read.plugins[0].hidden, false);
  assert.equal(localCalls.length, 0);
  for (const method of ["plugin/install", "plugin/uninstall", "install-plugin", "uninstall-plugin"]) {
    await window.electronBridge.sendMessageFromView({ type: "invoke", payload: { method } });
    await new Promise((resolve) => setImmediate(resolve));
  }
  assert.equal(localCalls.length, 4);
});

test("late bridge retries and reinjection preserve narrow request matching", async () => {
  const { window, timers, electronBridge, nativePromise, calls, sandbox } = setup({ lateBridge: true });
  assert.equal(timers.size, 1);
  const retry = [...timers.values()][0];
  assert.equal(retry.delay, 50);
  window.electronBridge = electronBridge;
  retry.callback();
  const wrapped = window.electronBridge.sendMessageFromView;
  vm.runInNewContext(source, sandbox);
  assert.equal(window.electronBridge.sendMessageFromView, wrapped);
  assert.equal(window.electronBridge.sendMessageFromView({ method: "turn/start", prompt: "plugin" }), nativePromise);
  const result = await window.electronBridge.sendMessageFromView({ method: "list-plugins", includeHidden: false });
  assert.equal(calls[1][0].includeHidden, true);
  assert.equal(result.plugins[0].hidden, false);
});

test("fetch matching ignores hostnames, query strings, fragments and chat payload content", async () => {
  const { interceptor, localCalls } = setup();
  const nativePromise = Promise.resolve({
    headers: { get() { throw new Error("unrelated response must remain untouched"); } },
  });
  for (const [url, body] of [
    ["https://plugins.example/chat", { prompt: "plugin" }],
    ["//plugins/chat", { prompt: "marketplace" }],
    ["https://example.com/chat?redirect=/plugins/list#marketplace", { prompt: "list-plugins" }],
    ["https://example.com/plugin-docs", { method: "turn/start", payload: { method: "list-plugins" } }],
    ["https://example.com/rpc", { method: "tools/call", params: { method: "plugin/list" } }],
    ["https://example.com/rpc", { description: "list-plugins", metadata: { method: "list-plugins" } }],
  ]) {
    const result = interceptor(() => nativePromise, url, { body: JSON.stringify(body) });
    assert.equal(result, nativePromise, url);
  }
  assert.equal(localCalls.length, 0);
});

test("plugin fetch routes patch JSON and RPC list bodies still wait for the local refresh", async () => {
  const { interceptor, localCalls } = setup();
  const native = () => Promise.resolve(new Response(JSON.stringify({ plugins: [{ id: "remote", hidden: true }] }), {
    headers: { "content-type": "application/json", "content-length": "1" },
  }));
  for (const url of ["https://example.com/plugins/list", "/api/marketplace/list", "/api/list-plugins"]) {
    const result = await interceptor(native, url);
    assert.equal(result.headers.has("content-length"), false);
    const data = await result.json();
    assert.equal(data.plugins[0].hidden, false);
    assert.equal(data.plugins.some((plugin) => plugin.id === "local"), true);
  }
  assert.equal(localCalls.length, 3);
  const nativeResponse = await native();
  const result = await interceptor(() => Promise.resolve(nativeResponse), "/rpc", {
    body: JSON.stringify({ type: "invoke", payload: { method: "plugin/list" } }),
  });
  assert.equal(result, nativeResponse);
  assert.equal(localCalls.length, 4);
});
