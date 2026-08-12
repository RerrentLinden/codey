import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(
  new URL("../public/prompt-optimize.js", import.meta.url),
  "utf8",
);

class FakeElement {
  constructor(tagName = "div", { visible = true, rect = null } = {}) {
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
    this.isContentEditable = false;
    this.readOnly = false;
    this.offsetWidth = 0;
    this.rect = rect;
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

  insertBefore(child, reference) {
    child.remove();
    const index = this.children.indexOf(reference);
    if (index < 0) return this.appendChild(child);
    child.parentElement = this;
    child.isConnected = true;
    this.children.splice(index, 0, child);
    return child;
  }

  remove() {
    if (!this.parentElement) return;
    const index = this.parentElement.children.indexOf(this);
    if (index >= 0) this.parentElement.children.splice(index, 1);
    this.parentElement = null;
    this.isConnected = false;
  }

  closest(selector) {
    const selectors = String(selector)
      .split(",")
      .map((value) => value.trim())
      .filter(Boolean);
    let element = this;
    while (element) {
      const matches = selectors.some((candidate) => {
        if (candidate.startsWith("#")) {
          return element.id === candidate.slice(1);
        }
        const attribute = candidate.match(
          /^\[([^=\]]+)(?:=['"]?([^'"\]]+)['"]?)?\]$/,
        );
        if (attribute) {
          const actual = element.getAttribute(attribute[1]);
          return attribute[2] === undefined
            ? actual !== null
            : actual === attribute[2];
        }
        return element.tagName === candidate.toUpperCase();
      });
      if (matches) return element;
      element = element.parentElement;
    }
    return null;
  }

  querySelectorAll() {
    return [];
  }

  getBoundingClientRect() {
    if (this.visible && this.rect) return { ...this.rect };
    return this.visible
      ? {
          bottom: 300,
          height: 120,
          left: 100,
          right: 800,
          top: 160,
          width: 700,
        }
      : { bottom: 0, height: 0, left: 0, right: 0, top: 0, width: 0 };
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
    if (name === "id") this.id = String(value);
    if (name === "contenteditable") {
      this.isContentEditable = String(value) === "true";
    }
  }

  focus() {}
}

let latestMutationObserver = null;

class FakeMutationObserver {
  constructor(callback) {
    this.callback = callback;
    this.observed = false;
    latestMutationObserver = this;
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
  const documentListeners = new Map();
  const windowListeners = new Map();
  let config = {
    promptOptimization: {
      enabled: options.enabled ?? true,
      apiKeyConfigured: options.apiKeyConfigured ?? true,
    },
  };
  const optimizeResult = options.optimizeResult ?? {
    optimized: "优化后的提示词",
  };

  const documentElement = new FakeElement("html");
  const body = new FakeElement("body");
  const anchor = new FakeElement("div");
  const scope = new FakeElement("div");
  const textarea = new FakeElement("textarea");
  textarea.value = options.initialText ?? "";
  const newChatInput = new FakeElement("div");
  newChatInput.setAttribute("contenteditable", "true");
  newChatInput.setAttribute("role", "textbox");
  const toolbar = new FakeElement("div");
  const accessButton = new FakeElement("button", {
    rect: {
      bottom: 290,
      height: 36,
      left: 120,
      right: 240,
      top: 254,
      width: 120,
    },
  });
  accessButton.textContent = "完全访问";
  const modelButton = new FakeElement("button", {
    rect: {
      bottom: 290,
      height: 36,
      left: 560,
      right: 720,
      top: 254,
      width: 160,
    },
  });
  modelButton.textContent = "5.6 Sol 极高";
  modelButton.setAttribute("aria-haspopup", "menu");
  const microphoneButton = new FakeElement("button", {
    rect: {
      bottom: 290,
      height: 36,
      left: 730,
      right: 766,
      top: 254,
      width: 36,
    },
  });
  const sendButton = new FakeElement("button", {
    rect: {
      bottom: 290,
      height: 36,
      left: 776,
      right: 812,
      top: 254,
      width: 36,
    },
  });
  const dialog = new FakeElement("div");
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  const dialogInput = new FakeElement("textarea", {
    rect: {
      bottom: 700,
      height: 180,
      left: 120,
      right: 920,
      top: 520,
      width: 800,
    },
  });
  dialogInput.value = options.dialogInitialText ?? "Git 提交信息";
  const dialogToolbar = new FakeElement("div");
  const dialogControl = new FakeElement("button", {
    rect: {
      bottom: 690,
      height: 36,
      left: 960,
      right: 1160,
      top: 654,
      width: 200,
    },
  });
  dialogControl.textContent = "提交并推送";
  dialogControl.setAttribute("aria-haspopup", "menu");
  documentElement.appendChild(body);
  body.appendChild(scope);
  scope.appendChild(anchor);
  scope.appendChild(textarea);
  scope.appendChild(newChatInput);
  scope.appendChild(toolbar);
  toolbar.appendChild(accessButton);
  toolbar.appendChild(modelButton);
  toolbar.appendChild(microphoneButton);
  toolbar.appendChild(sendButton);
  if (options.dialogComposer || options.dialogControl) {
    scope.appendChild(dialog);
  }
  if (options.dialogComposer) {
    dialog.appendChild(dialogInput);
  }
  if (options.dialogControl) {
    dialog.appendChild(dialogToolbar);
    dialogToolbar.appendChild(dialogControl);
  }
  let fallbackInputs = options.newChatComposer ? [newChatInput] : [textarea];
  if (options.dialogComposer) {
    fallbackInputs = options.onlyDialogComposer
      ? [dialogInput]
      : [...fallbackInputs, dialogInput];
  }
  scope.querySelectorAll = (selector) => {
    if (selector === "textarea, [contenteditable='true'], [role='textbox']") {
      return [textarea];
    }
    if (selector === "button, [role='button']") {
      const controls = [
        accessButton,
        modelButton,
        microphoneButton,
        sendButton,
      ];
      if (options.dialogControl) controls.push(dialogControl);
      return controls;
    }
    return [];
  };

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
      if (
        selector ===
        "main textarea, main [contenteditable='true'], main [role='textbox'], textarea, [contenteditable='true'][role='textbox']"
      ) {
        return options.fallbackTextareas === false ? [] : fallbackInputs;
      }
      return [];
    },
    addEventListener(type, handler) {
      const handlers = documentListeners.get(type) || [];
      handlers.push(handler);
      documentListeners.set(type, handlers);
    },
  };

  const window = {
    innerHeight: 800,
    innerWidth: 1280,
    addEventListener(type, handler) {
      const handlers = windowListeners.get(type) || [];
      handlers.push(handler);
      windowListeners.set(type, handlers);
    },
    getComputedStyle: () => ({ display: "block", visibility: "visible" }),
  };

  const testSetTimeout = (callback, delay, ...args) => {
    const timer = setTimeout(callback, delay, ...args);
    timer.unref?.();
    return timer;
  };
  const sandbox = {
    document,
    window,
    MutationObserver: FakeMutationObserver,
    Event: FakeEvent,
    InputEvent: FakeEvent,
    setTimeout: testSetTimeout,
    clearTimeout,
    HTMLElement: class HTMLElement {},
    HTMLTextAreaElement: class HTMLTextAreaElement {},
  };
  const bridge = async (path, payload) => {
    calls.push({ path, payload });
    if (path === "/settings/get") return config;
    if (path === "/api/optimize_prompt") return optimizeResult;
    return {};
  };
  if (options.bridgeReady !== false) {
    sandbox.window.__codexSessionDeleteBridge = bridge;
  }
  const context = vm.createContext(sandbox);
  vm.runInContext(source, context);

  return {
    calls,
    dialog,
    dialogControl,
    dialogInput,
    inputEvents,
    newChatInput,
    textarea,
    toolbar,
    accessButton,
    modelButton,
    scope,
    getElementById: (id) => findById(documentElement, id),
    snapshot: () => context.window.__codeyPromptOptimize.snapshot(),
    setConfig: (next) => {
      config = next;
    },
    setBridgeReady: () => {
      context.window.__codexSessionDeleteBridge = bridge;
    },
    emitConfigChanged: () => {
      for (const handler of windowListeners.get("codey:config-changed") || []) {
        handler.call(window);
      }
    },
    emitMutation: () =>
      latestMutationObserver?.callback([{ target: documentElement }]),
    emitInput: (target = textarea) => {
      for (const handler of documentListeners.get("input") || []) {
        handler.call(document, { type: "input", target });
      }
    },
    setFallbackInputs: (inputs) => {
      fallbackInputs = inputs;
    },
  };
};

test("retries config loading when the bridge becomes ready after injection", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    bridgeReady: false,
  });
  env.setBridgeReady();
  await new Promise((resolve) => setTimeout(resolve, 180));

  assert.ok(env.getElementById("codey-prompt-optimize-button"));
  assert.equal(env.snapshot().ready, true);
});

test("mounts the optimize button when enabled and an API key is configured", async () => {
  const env = createEnvironment({ enabled: true, apiKeyConfigured: true });
  await flush();

  const button = env.getElementById("codey-prompt-optimize-button");
  assert.ok(button, "button should be mounted");
  assert.equal(button.dataset.codeyPromptOptimize, "true");
  assert.equal(button.dataset.codeyPromptOptimizeLayout, "model-picker");
  assert.equal(button.style.display, "inline-flex");
  assert.equal(button.disabled, true);
  assert.equal(button.getAttribute("aria-disabled"), "true");
  assert.equal(button.parentElement, env.toolbar);
  assert.deepEqual(env.toolbar.children.slice(0, 3), [
    env.accessButton,
    button,
    env.modelButton,
  ]);
  assert.equal(env.snapshot().enabled, true);
  assert.equal(env.snapshot().ready, true);
  assert.equal(env.snapshot().buttonDisabled, true);
});

test("enables the optimize button only while the composer has content", async () => {
  const env = createEnvironment({ enabled: true, apiKeyConfigured: true });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");

  env.textarea.value = "   ";
  env.emitInput();
  assert.equal(button.disabled, true);

  env.textarea.value = "需要优化的提示词";
  env.emitInput();
  assert.equal(button.disabled, false);
  assert.equal(button.getAttribute("aria-disabled"), "false");

  env.textarea.value = "";
  env.emitInput();
  assert.equal(button.disabled, true);
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

test("ignores Git commit textboxes inside modal dialogs", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    anchors: false,
    dialogComposer: true,
    initialText: "正常对话提示词",
  });
  await flush();

  const button = env.getElementById("codey-prompt-optimize-button");
  assert.ok(button, "the normal composer should still receive the button");
  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });
  await flush();

  const optimizeCall = env.calls.find(
    (call) => call.path === "/api/optimize_prompt",
  );
  assert.equal(optimizeCall?.payload.text, "正常对话提示词");
  assert.equal(env.textarea.value, "优化后的提示词");
  assert.equal(env.dialogInput.value, "Git 提交信息");
});

test("does not use controls inside modal dialogs as insertion targets", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    dialogControl: true,
  });
  await flush();

  const button = env.getElementById("codey-prompt-optimize-button");
  assert.ok(button);
  assert.equal(button.parentElement, env.toolbar);
  assert.equal(env.dialogControl.parentElement.parentElement, env.dialog);
});

test("mounts the optimize button for a new-chat contenteditable composer", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    anchors: false,
    newChatComposer: true,
  });
  await flush();

  const button = env.getElementById("codey-prompt-optimize-button");
  assert.ok(button, "new-chat composer should receive the optimize button");
  assert.equal(button.style.display, "inline-flex");
  assert.equal(env.snapshot().hasInput, true);
});

test("rescans when a connected composer is replaced during navigation", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    anchors: false,
  });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");
  assert.ok(button);

  env.textarea.visible = false;
  const nextInput = new FakeElement("div");
  nextInput.setAttribute("contenteditable", "true");
  nextInput.setAttribute("role", "textbox");
  nextInput.innerText = "新对话里的提示词";
  env.scope.appendChild(nextInput);
  env.setFallbackInputs([env.textarea, nextInput]);
  env.emitMutation();
  await new Promise((resolve) => setTimeout(resolve, 280));

  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });
  await flush();

  assert.equal(nextInput.innerText, "优化后的提示词");
  assert.equal(env.textarea.value, "");
});

test("clicking the button calls the bridge and replaces the composer text", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    initialText: "写一个关于 Rust 的博客",
  });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");
  assert.equal(button.disabled, false);

  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });
  await flush();

  const optimizeCall = env.calls.find(
    (call) => call.path === "/api/optimize_prompt",
  );
  assert.ok(
    optimizeCall,
    "optimize_prompt should be called through the bridge",
  );
  assert.equal(optimizeCall.payload.text, "写一个关于 Rust 的博客");
  assert.equal(env.textarea.value, "优化后的提示词");
  assert.equal(button.dataset.busy, "false");
});

test("shows a disabled loading state while optimization is pending", async () => {
  let resolveOptimization;
  const optimizeResult = new Promise((resolve) => {
    resolveOptimization = resolve;
  });
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    initialText: "原始提示词",
    optimizeResult,
  });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");

  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });

  assert.equal(button.disabled, true);
  assert.equal(button.dataset.busy, "true");
  assert.equal(button.getAttribute("aria-busy"), "true");
  assert.equal(env.snapshot().buttonBusy, true);

  resolveOptimization({ optimized: "优化完成" });
  await flush();

  assert.equal(button.disabled, false);
  assert.equal(button.dataset.busy, "false");
  assert.equal(button.getAttribute("aria-busy"), "false");
  assert.equal(env.textarea.value, "优化完成");
});

test("failed optimization keeps the original text and uses the global toast", async () => {
  const env = createEnvironment({
    enabled: true,
    apiKeyConfigured: true,
    initialText: "原文",
    optimizeResult: { status: "failed", message: "API Key 无效" },
  });
  await flush();
  const button = env.getElementById("codey-prompt-optimize-button");

  button.dispatchEvent({
    type: "click",
    preventDefault() {},
    stopPropagation() {},
  });
  await flush();

  assert.equal(env.textarea.value, "原文");
  const toast = env.getElementById("codey-runtime-toast");
  assert.ok(toast, "global error toast should be created");
  assert.equal(toast.dataset.tone, "error");
  assert.equal(toast.getAttribute("role"), "alert");
  assert.equal(toast.textContent, "API Key 无效");
});

test("an empty composer keeps the button disabled without showing an error", async () => {
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
  assert.equal(button.disabled, true);
  assert.equal(env.getElementById("codey-runtime-toast"), null);
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
