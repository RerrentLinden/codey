import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(
  new URL("../public/prompt-optimize.js", import.meta.url),
  "utf8",
);

class FakeElement {
  constructor(tagName = "div", { visible = true } = {}) {
    this.tagName = tagName.toUpperCase();
    this.children = [];
    this.dataset = {};
    this.id = "";
    this.parentElement = null;
    this.style = {};
    this.textContent = "";
    this.attributes = new Map();
    this.listeners = new Map();
    this.visible = visible;
    this.isConnected = false;
    this.value = "";
    this.innerText = "";
    this.disabled = false;
    this.offsetWidth = 0;
  }

  addEventListener(type, handler) {
    if (typeof handler !== "function") return;
    const handlers = this.listeners.get(type) || [];
    handlers.push(handler);
    this.listeners.set(type, handlers);
  }

  dispatchEvent(event) {
    if (!event?.type) return true;
    if (!event.target) event.target = this;
    event.currentTarget = this;
    for (const handler of [...(this.listeners.get(event.type) || [])]) {
      handler.call(this, event);
    }
    return true;
  }

  appendChild(child) {
    child.remove();
    child.parentElement = this;
    child.isConnected = true;
    this.children.push(child);
    return child;
  }

  remove() {
    if (!this.parentElement) return;
    const index = this.parentElement.children.indexOf(this);
    if (index >= 0) this.parentElement.children.splice(index, 1);
    this.parentElement = null;
    this.isConnected = false;
  }

  closest() {
    return null;
  }

  querySelectorAll() {
    return [];
  }

  getBoundingClientRect() {
    return this.visible
      ? { bottom: 300, height: 120, left: 100, right: 800, top: 160, width: 700 }
      : { bottom: 0, height: 0, left: 0, right: 0, top: 0, width: 0 };
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
    if (name === "id") this.id = String(value);
  }

  focus() {}
}

class FakeMutationObserver {
  constructor(callback) {
    this.callback = callback;
    this.observed = false;
  }

  observe() {
    this.observed = true;
  }
}

class FakeEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.bubbles = init.bubbles ?? false;
    this.target = null;
  }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 10));

const createEnvironment = (options = {}) => {
  const calls = [];
  const inputEvents = [];
  const windowListeners = new Map();
  let config = {
    promptOptimization: {
      enabled: options.enabled ?? true,
      apiKeyConfigured: options.apiKeyConfigured ?? true,
    },
  };
  const optimizeResult =
    options.optimizeResult ?? { optimized: "优化后的提示词" };

  const documentElement = new FakeElement("html");
  const body = new FakeElement("body");
  const anchor = new FakeElement("div");
  const scope = new FakeElement("div");
  anchor.parentElement = scope;
  const textarea = new FakeElement("textarea");
  scope.querySelectorAll = (selector) =>
    selector === "textarea, [contenteditable]" ? [textarea] : [];

  const findById = (root, id) => {
    if (!root || typeof root.children?.forEach !== "function") return null;
    if (root.id === id) return root;
    for (const child of root.children) {
      const found = findById(child, id);
      if (found) return found;
    }
    return null;
  };

  const document = {
    body,
    documentElement,
    createElement: (tagName) => new FakeElement(tagName),
    getElementById: (id) => findById(documentElement, id),
    querySelector: () => null,
    querySelectorAll: (selector) => {
      if (selector === "[data-above-composer-conversation-id]") {
        return options.anchors === false ? [] : [anchor];
      }
      if (selector === "main textarea") {
        return options.fallbackTextareas === false ? [] : [textarea];
      }
      return [];
    },
    addEventListener() {},
  };

  const window = {
    innerWidth: 1280,
    addEventListener(type, handler) {
      const handlers = windowListeners.get(type) || [];
      handlers.push(handler);
      windowListeners.set(type, handlers);
    },
    getComputedStyle: () => ({ display: "block", visibility: "visible" }),
  };

  const sandbox = {
    document,
    window,
    MutationObserver: FakeMutationObserver,
    Event: FakeEvent,
    InputEvent: FakeEvent,
    setTimeout,
    clearTimeout,
    HTMLElement: class HTMLElement {},
    HTMLTextAreaElement: class HTMLTextAreaElement {},
  };
  sandbox.window.__codexSessionDeleteBridge = async (path, payload) => {
    calls.push({ path, payload });
    if (path === "/settings/get") return config;
    if (path === "/api/optimize_prompt") return optimizeResult;
    return {};
  };
  const context = vm.createContext(sandbox);
  vm.runInContext(source, context);

  return {
    calls,
    inputEvents,
    textarea,
    scope,
    getElementById: (id) => findById(documentElement, id),
    snapshot: () => context.window.__codeyPromptOptimize.snapshot(),
    setConfig: (next) => {
      config = next;
    },
    emitConfigChanged: () => {
      for (const handler of windowListeners.get("codey:config-changed") || []) {
        handler.call(window);
      }
    },
  };
};

test("mounts the optimize button when enabled and an API key is configured", async () => {
  const env = createEnvironment({ enabled: true, apiKeyConfigured: true });
  await flush();

  const button = env.getElementById("codey-prompt-optimize-button");
  assert.ok(button, "button should be mounted");
  assert.equal(button.dataset.codeyPromptOptimize, "true");
  assert.equal(button.style.display, "inline-flex");
  assert.equal(env.snapshot().enabled, true);
  assert.equal(env.snapshot().ready, true);
});

test("does not mount the button when the feature is disabled", async () => {
  const env = createEnvironment({ enabled: false, apiKeyConfigured: true });
  await flush();

  assert.equal(env.getElementById("codey-prompt-optimize-button"), null);
  assert.equal(env.snapshot().enabled, false);
});

test("does not mount the button when no API key is configured yet", async () => {
  const env = createEnvironment({ enabled: true, apiKeyConfigured: false });
  await flush();

  assert.equal(env.getElementById("codey-prompt-optimize-button"), null);
});

test("keeps the button hidden when no composer input is found", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    anchors: false,
    fallbackTextareas: false,
  });
  await flush();

  assert.equal(env.getElementById("codey-prompt-optimize-button"), null);
});

test("clicking the button calls the bridge and replaces the composer text", async () => {
  const env = createEnvironment({ enabled: true, apiKeyConfigured: true });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");
  env.textarea.value = "写一个关于 Rust 的博客";

  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });
  await flush();

  const optimizeCall = env.calls.find(
    (call) => call.path === "/api/optimize_prompt",
  );
  assert.ok(optimizeCall, "optimize_prompt should be called through the bridge");
  assert.equal(optimizeCall.payload.text, "写一个关于 Rust 的博客");
  assert.equal(env.textarea.value, "优化后的提示词");
  assert.equal(button.dataset.busy, "false");
});

test("failed optimization keeps the original text and shows the error", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    optimizeResult: { status: "failed", message: "API Key 无效" },
  });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");
  env.textarea.value = "原文";

  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });
  await flush();

  assert.equal(env.textarea.value, "原文");
  const popover = env.getElementById("codey-prompt-optimize-error");
  assert.ok(popover, "error popover should be created");
  assert.equal(popover.textContent, "API Key 无效");
});

test("clicking with an empty composer shows a hint without calling the bridge", async () => {
  const env = createEnvironment({ enabled: true, apiKeyConfigured: true });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");

  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });
  await flush();

  assert.equal(
    env.calls.some((call) => call.path === "/api/optimize_prompt"),
    false,
  );
  assert.equal(
    env.getElementById("codey-prompt-optimize-error").textContent,
    "请先在输入框输入要优化的内容",
  );
});

test("re-applies the switch when the console saves config", async () => {
  const env = createEnvironment({ enabled: false, apiKeyConfigured: true });
  await flush();
  assert.equal(env.getElementById("codey-prompt-optimize-button"), null);

  env.setConfig({
    promptOptimization: { enabled: true, apiKeyConfigured: true },
  });
  env.emitConfigChanged();
  await flush();

  assert.ok(env.getElementById("codey-prompt-optimize-button"));
  assert.equal(env.snapshot().enabled, true);
});
