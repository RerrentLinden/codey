import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const source = await readFile(
  new URL("../public/codey-bridge.js", import.meta.url),
  "utf8",
);

function createRuntime() {
  const observers = [];
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
  const window = {};
  window.window = window;
  vm.runInNewContext(source, { document, MutationObserver: FakeMutationObserver, window });
  return { observers, window };
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
