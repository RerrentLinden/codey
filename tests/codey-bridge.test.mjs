import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const source = await readFile(
  new URL("../public/codey-bridge.js", import.meta.url),
  "utf8",
);

function createRuntime({ fetch = undefined, statsig = undefined } = {}) {
  const observers = [];
  class FakeHTMLElement {}
  class FakeMutationObserver {
    constructor(callback) {
      this.callback = callback;
      this.disconnects = 0;
      this.options = null;
      observers.push(this);
    }

    disconnect() {
      this.disconnects += 1;
    }

    observe(_target, options) {
      this.options = options;
    }
  }
  const document = { documentElement: {} };
  const window = { __STATSIG__: statsig, fetch };
  window.window = window;
  vm.runInNewContext(source, {
    document,
    HTMLElement: FakeHTMLElement,
    MutationObserver: FakeMutationObserver,
    window,
  });
  return { FakeHTMLElement, observers, window };
}

test("mutation dispatcher unions subscriptions and tears down only when empty", () => {
  const runtime = createRuntime();
  const calls = [];
  const dispatcher = runtime.window.__codeyMutationDispatcher;
  const unsubscribePet = dispatcher.subscribe(
    (mutations) => calls.push(["pet", ...mutations]),
    {
      attributes: true,
      attributeFilter: ["aria-label", "role", "title"],
      childList: true,
    },
  );
  const unsubscribeSecurity = dispatcher.subscribe(
    (mutations) => calls.push(["security", ...mutations]),
    { childList: true },
  );
  const unsubscribeVoice = dispatcher.subscribe(
    (mutations) => calls.push(["voice", ...mutations]),
    {
      attributes: true,
      attributeFilter: ["aria-label", "role", "title", "src"],
      childList: true,
    },
  );

  const activeObserver = runtime.observers.at(-1);
  assert.deepEqual(
    [...activeObserver.options.attributeFilter],
    ["aria-label", "role", "title", "src"],
  );
  assert.equal(activeObserver.options.attributes, true);
  assert.equal(activeObserver.options.childList, true);
  assert.equal(activeObserver.options.subtree, true);
  assert.equal(dispatcher.snapshot().observerInstalled, true);
  assert.equal(dispatcher.snapshot().subscriberCount, 3);

  activeObserver.callback(["mutation"]);
  assert.deepEqual(calls, [
    ["pet", "mutation"],
    ["security", "mutation"],
    ["voice", "mutation"],
  ]);

  unsubscribeVoice();
  unsubscribeVoice();
  assert.deepEqual(
    [...runtime.observers.at(-1).options.attributeFilter],
    ["aria-label", "role", "title"],
  );
  assert.equal(dispatcher.snapshot().subscriberCount, 2);

  unsubscribePet();
  assert.equal(dispatcher.snapshot().subscriberCount, 1);
  assert.equal(runtime.observers.at(-1).options.childList, true);
  assert.equal(runtime.observers.at(-1).options.attributes, false);

  const finalObserver = runtime.observers.at(-1);
  unsubscribeSecurity();
  assert.equal(finalObserver.disconnects, 1);
  assert.equal(dispatcher.snapshot().observerInstalled, false);
  assert.equal(dispatcher.snapshot().subscriberCount, 0);
});

test("shared control lookup includes a matching root and its descendants", () => {
  const runtime = createRuntime();
  const child = {};
  const root = new runtime.FakeHTMLElement();
  root.matches = (selector) => selector === "button";
  root.querySelectorAll = (selector) => selector === "button" ? [child] : [];

  const controls = runtime.window.__codeyMutationDispatcher.controlsWithin(root, "button");
  assert.equal(controls.length, 2);
  assert.equal(controls[0], root);
  assert.equal(controls[1], child);
});

test("shared control descriptor normalizes accessible labels and text", () => {
  const runtime = createRuntime();
  const control = {
    getAttribute(name) {
      return name === "aria-label" ? "  Voice " : name === "title" ? "Start" : null;
    },
    textContent: " now\nplease ",
  };

  assert.equal(
    runtime.window.__codeyMutationDispatcher.controlDescriptor(control),
    "Voice Start now please",
  );
});

test("shared helpers deduplicate Statsig clients and inspect React values", () => {
  const first = {};
  const second = {};
  const runtime = createRuntime({
    statsig: {
      firstInstance: first,
      instance: () => first,
      instances: { first, second },
    },
  });
  const control = new runtime.FakeHTMLElement();
  control.__reactProps$test = {
    children: { props: { id: "settings.personalization.pets" } },
  };
  const shared = runtime.window.__codeySharedRuntime;

  assert.deepEqual([...shared.statsigClients()], [first, second]);
  assert.equal(shared.reactInternals(control).length, 1);
  assert.equal(
    shared.objectGraphIncludes(
      shared.reactInternals(control)[0],
      (value) => value === "settings.personalization.pets",
      { ignoredKeys: new Set(["return"]), maxDepth: 7 },
    ),
    true,
  );
});

test("fetch interceptors share one stable wrapper and unregister independently", async () => {
  const nativeCalls = [];
  const nativeFetch = async (input) => {
    nativeCalls.push(input);
    return input;
  };
  const runtime = createRuntime({ fetch: nativeFetch });
  const shared = runtime.window.__codeySharedRuntime;
  const order = [];
  const unregisterInner = shared.registerFetchInterceptor(
    "inner",
    (next, input) => {
      order.push("inner");
      return next(`${input}:inner`);
    },
    10,
  );
  const wrapper = runtime.window.fetch;
  const unregisterOuter = shared.registerFetchInterceptor(
    "outer",
    (next, input) => {
      order.push("outer");
      return next(`${input}:outer`);
    },
    20,
  );

  assert.equal(runtime.window.fetch, wrapper);
  assert.equal(await runtime.window.fetch("request"), "request:outer:inner");
  assert.deepEqual(order, ["outer", "inner"]);
  assert.deepEqual(nativeCalls, ["request:outer:inner"]);
  assert.equal(shared.fetchSnapshot().interceptorCount, 2);

  unregisterOuter();
  assert.equal(runtime.window.fetch, wrapper);
  unregisterInner();
  assert.equal(runtime.window.fetch, nativeFetch);
  assert.equal(shared.fetchSnapshot().installed, false);
});
