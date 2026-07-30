import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const template = readFileSync(
  new URL("../public/voice-control-shield.js", import.meta.url),
  "utf8",
);

class FakeElement {
  constructor(text = "", isControl = true, children = [], isImage = false) {
    this.textContent = text;
    this.attributes = new Map();
    this.children = children;
    this.disabled = false;
    this.isConnected = true;
    this.isControl = isControl;
    this.isImage = isImage;
    this.parentElement = null;
    this.removedStyleProperties = [];
    this.style = {
      getPropertyPriority: (name) => {
        const value = String(this.style[name] ?? "");
        return value.endsWith(":important") ? "important" : "";
      },
      getPropertyValue: (name) => String(this.style[name] ?? "").replace(/:important$/, ""),
      removeProperty: (name) => {
        this.removedStyleProperties.push(name);
        delete this.style[name];
      },
      setProperty: (name, value, priority) => {
        this.style[name] = priority ? `${value}:${priority}` : value;
      },
    };
    children.forEach((child) => {
      child.parentElement = this;
    });
  }

  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  closest(selector) {
    let current = this;
    while (current) {
      if (current.matches?.(selector)) return current;
      current = current.parentElement;
    }
    return null;
  }

  contains(node) {
    let current = node;
    while (current) {
      if (current === this) return true;
      current = current.parentElement;
    }
    return false;
  }

  matches(selector = "") {
    const selectors = selector.split(",").map((part) => part.trim()).filter(Boolean);
    if (selectors.includes("img") && this.isImage) return true;
    if (selectors.length === 1 && selectors[0] === "img") return this.isImage;
    return this.isControl;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  querySelectorAll(selector = "") {
    const matches = [];
    const visit = (node) => {
      for (const child of node.children) {
        if (child.matches?.(selector)) matches.push(child);
        visit(child);
      }
    };
    visit(this);
    return matches;
  }
}

function loadShield(enabled) {
  const semantic = new FakeElement();
  semantic.__reactProps$test = {
    children: { props: { id: "codex.command.composer.startDictation" } },
  };
  const settings = new FakeElement();
  settings.__reactFiber$test = {
    memoizedProps: { id: "settings.general.globalDictationHotkey.label" },
  };
  const localized = new FakeElement("开始听写");
  const localizedNewVoiceChat = new FakeElement();
  localizedNewVoiceChat.setAttribute("aria-label", "开始新的语音聊天");
  const localizedTraditionalNewVoiceChat = new FakeElement();
  localizedTraditionalNewVoiceChat.setAttribute("aria-label", "開始新的語音聊天");
  const gptVoiceComposer = new FakeElement();
  gptVoiceComposer.__reactProps$test = {
    children: { props: { id: "composer.realtime.start" } },
  };
  const gptVoiceIcon = new FakeElement();
  gptVoiceIcon.__reactFiber$test = {
    memoizedProps: { className: "composer-icon-button" },
    return: {
      memoizedProps: {
        tooltipContent: { props: { id: "composer.realtime.start" } },
      },
      return: null,
    },
  };
  const gptVoiceSettings = new FakeElement();
  gptVoiceSettings.__reactFiber$test = {
    memoizedProps: { id: "settings.general.realtimeVoiceHotkey.label" },
  };
  const gptVoiceBannerAction = new FakeElement("Start Voice");
  gptVoiceBannerAction.__reactProps$test = {
    children: { props: { id: "realtimeVoice.homeAnnouncement.action" } },
  };
  const gptVoiceBannerDismiss = new FakeElement();
  const gptVoicePromotion = new FakeElement(
    "Try ChatGPT Voice Coordinate tasks, connect tools, and explore ideas Start Voice",
    false,
  );
  gptVoicePromotion.querySelectorAll = (selector) =>
    selector.includes("button") ? [gptVoiceBannerAction, gptVoiceBannerDismiss] : [];
  const gptVoicePromotionVisual = new FakeElement("", false);
  gptVoicePromotionVisual.parentElement = gptVoicePromotion;
  gptVoiceBannerAction.parentElement = gptVoicePromotion;
  gptVoiceBannerDismiss.parentElement = gptVoicePromotion;
  const gptVoicePromotionAsset = new FakeElement("", false, [], true);
  gptVoicePromotionAsset.setAttribute(
    "src",
    "https://persistent.oaistatic.com/voice/bidi-homepage-banner-orb.21107572.webp",
  );
  gptVoicePromotionAsset.parentElement = gptVoicePromotionVisual;
  const unrelated = new FakeElement("打开设置");
  const unrelatedIcon = new FakeElement();
  unrelatedIcon.__reactFiber$test = {
    memoizedProps: { className: "composer-icon-button" },
    return: {
      memoizedProps: {
        children: { props: { id: "composer.realtime.start" } },
      },
      return: null,
    },
  };
  const controls = [
    semantic,
    settings,
    localized,
    localizedNewVoiceChat,
    localizedTraditionalNewVoiceChat,
    gptVoiceComposer,
    gptVoiceIcon,
    gptVoiceSettings,
    gptVoiceBannerAction,
    gptVoiceBannerDismiss,
    unrelated,
    unrelatedIcon,
  ];
  const listeners = new Map();
  let mutationCallback = null;
  let observerOptions = null;
  let observerDisconnects = 0;
  class FakeMutationObserver {
    constructor(callback) {
      mutationCallback = callback;
    }

    observe(_target, options) {
      observerOptions = options;
    }

    disconnect() {
      observerDisconnects += 1;
    }
  }
  const documentElement = new FakeElement("", false);
  const document = {
    body: null,
    documentElement,
    querySelectorAll: (selector) => selector === "img" ? [gptVoicePromotionAsset] : controls,
    addEventListener: (name, listener) => listeners.set(name, listener),
    removeEventListener: (name) => listeners.delete(name),
  };
  const mediaCalls = [];
  const enumerateDeviceCalls = [];
  const fetchCalls = [];
  const webSocketCalls = [];
  const nativeGetUserMedia = (constraints) => {
    mediaCalls.push(constraints);
    return Promise.resolve("native-media");
  };
  const nativeEnumerateDevices = () => {
    enumerateDeviceCalls.push(true);
    return Promise.resolve([
      { deviceId: "microphone", kind: "audioinput" },
      { deviceId: "speakers", kind: "audiooutput" },
      { deviceId: "camera", kind: "videoinput" },
    ]);
  };
  const nativeFetch = (input) => {
    fetchCalls.push(input);
    return Promise.resolve("native-fetch");
  };
  class NativeWebSocket {
    constructor(url) {
      this.url = url;
      webSocketCalls.push(url);
    }
  }
  const rtcPeerConnectionCalls = [];
  class NativeRTCPeerConnection {
    constructor(configuration) {
      rtcPeerConnectionCalls.push(configuration);
    }
  }
  const window = {
    fetch: nativeFetch,
    navigator: {
      mediaDevices: {
        enumerateDevices: nativeEnumerateDevices,
        getUserMedia: nativeGetUserMedia,
      },
    },
    RTCPeerConnection: NativeRTCPeerConnection,
    WebSocket: NativeWebSocket,
  };
  window.window = window;
  const pendingTimers = new Map();
  const pendingAnimationFrames = new Map();
  let nextTimerId = 1;
  let nextAnimationFrameId = 1;
  let scheduledFlushes = 0;
  window.setTimeout = (callback) => {
    const id = nextTimerId;
    nextTimerId += 1;
    scheduledFlushes += 1;
    pendingTimers.set(id, callback);
    return id;
  };
  window.clearTimeout = (id) => {
    pendingTimers.delete(id);
  };
  window.requestAnimationFrame = (callback) => {
    const id = nextAnimationFrameId;
    nextAnimationFrameId += 1;
    scheduledFlushes += 1;
    pendingAnimationFrames.set(id, callback);
    return id;
  };
  window.cancelAnimationFrame = (id) => {
    pendingAnimationFrames.delete(id);
  };
  const sandbox = {
    document,
    Element: FakeElement,
    HTMLElement: FakeElement,
    MutationObserver: FakeMutationObserver,
    URL,
    window,
  };
  const inject = (nextEnabled = enabled) => {
    vm.runInNewContext(
      template.replace("__CODEY_SLIM_VOICE__", nextEnabled ? "true" : "false"),
      sandbox,
    );
  };
  const runPendingAnimationFrames = () => {
    const callbacks = [...pendingAnimationFrames.values()];
    pendingAnimationFrames.clear();
    callbacks.forEach((callback) => callback());
  };
  const runPendingTimers = () => {
    const callbacks = [...pendingTimers.values()];
    pendingTimers.clear();
    callbacks.forEach((callback) => callback());
  };
  inject(enabled);
  return {
    documentElement,
    enumerateDeviceCalls,
    fetchCalls,
    gptVoiceBannerAction,
    gptVoiceComposer,
    gptVoiceIcon,
    gptVoicePromotion,
    gptVoiceSettings,
    listeners,
    localized,
    localizedNewVoiceChat,
    localizedTraditionalNewVoiceChat,
    mediaCalls,
    mutationCallback,
    nativeEnumerateDevices,
    nativeFetch,
    nativeGetUserMedia,
    NativeRTCPeerConnection,
    NativeWebSocket,
    observerOptions,
    get observerDisconnects() {
      return observerDisconnects;
    },
    get pendingAnimationFrameCount() {
      return pendingAnimationFrames.size;
    },
    get pendingFlushCount() {
      return pendingTimers.size + pendingAnimationFrames.size;
    },
    get pendingTimerCount() {
      return pendingTimers.size;
    },
    rtcPeerConnectionCalls,
    runPendingAnimationFrames,
    runPendingTimers,
    get scheduledFlushes() {
      return scheduledFlushes;
    },
    semantic,
    settings,
    inject,
    unrelated,
    unrelatedIcon,
    webSocketCalls,
    window,
  };
}

test("voice slim mode blocks composer, settings, and localized voice controls", () => {
  const runtime = loadShield(true);

  for (const control of [
    runtime.semantic,
    runtime.settings,
    runtime.localized,
    runtime.localizedNewVoiceChat,
    runtime.localizedTraditionalNewVoiceChat,
    runtime.gptVoiceComposer,
    runtime.gptVoiceIcon,
    runtime.gptVoiceSettings,
    runtime.gptVoicePromotion,
  ]) {
    assert.equal(control.getAttribute("data-codey-voice-control-blocked"), "true");
    assert.equal(control.disabled, true);
    assert.equal(control.style.display, "none:important");
  }
  assert.equal(
    runtime.gptVoiceBannerAction.getAttribute("data-codey-voice-control-blocked"),
    null,
  );
  assert.equal(runtime.unrelated.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.unrelatedIcon.getAttribute("data-codey-voice-control-blocked"), null);

  runtime.gptVoiceIcon.setAttribute("aria-label", "Send");
  runtime.window.__codeyBlockNativeVoiceControls();
  assert.equal(runtime.gptVoiceIcon.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.gptVoiceIcon.getAttribute("aria-hidden"), null);
  assert.equal(runtime.gptVoiceIcon.disabled, false);
  assert.equal(runtime.gptVoiceIcon.style.display, undefined);

  let prevented = false;
  let stopped = false;
  runtime.listeners.get("click")({
    target: runtime.semantic,
    preventDefault: () => { prevented = true; },
    stopPropagation: () => { stopped = true; },
    stopImmediatePropagation: () => {},
  });
  assert.equal(prevented, true);
  assert.equal(stopped, true);
});

test("voice slim mode blocks inserted voice controls before a deferred flush", () => {
  const runtime = loadShield(true);
  const dynamic = new FakeElement("语音");
  const menu = new FakeElement("", false, [dynamic]);

  runtime.mutationCallback([{
    addedNodes: [menu],
    target: runtime.documentElement,
    type: "childList",
  }]);

  assert.equal(dynamic.getAttribute("data-codey-voice-control-blocked"), "true");
  assert.equal(dynamic.getAttribute("aria-hidden"), "true");
  assert.equal(dynamic.getAttribute("tabindex"), "-1");
  assert.equal(dynamic.getAttribute("inert"), "");
  assert.equal(dynamic.style.display, "none:important");
  assert.equal(dynamic.disabled, true);
  assert.equal(runtime.pendingFlushCount, 0);
  assert.equal(runtime.observerOptions.attributes, true);
  assert.deepEqual(
    [...runtime.observerOptions.attributeFilter],
    ["aria-label", "role", "title", "src"],
  );
  assert.equal(runtime.observerOptions.childList, true);
  assert.equal(runtime.observerOptions.subtree, true);
});

test("voice slim mode preserves hidden controls across enabled reinjection", () => {
  const runtime = loadShield(true);
  const styleRestoresAfterFirstInject = runtime.semantic.removedStyleProperties.length;

  runtime.inject(true);

  assert.equal(runtime.semantic.getAttribute("data-codey-voice-control-blocked"), "true");
  assert.equal(runtime.semantic.style.display, "none:important");
  assert.equal(runtime.semantic.disabled, true);
  assert.equal(runtime.semantic.removedStyleProperties.length, styleRestoresAfterFirstInject);
  assert.equal(runtime.observerDisconnects, 1);
  assert.equal(runtime.window.__codeyVoiceControlShield.enabled, true);

  runtime.inject(false);

  assert.equal(runtime.semantic.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.semantic.getAttribute("aria-hidden"), null);
  assert.equal(runtime.semantic.getAttribute("tabindex"), null);
  assert.equal(runtime.semantic.getAttribute("inert"), null);
  assert.equal(runtime.semantic.style.display, undefined);
  assert.equal(runtime.semantic.disabled, false);
  assert.equal(runtime.window.__codeyVoiceControlShield.enabled, false);
  assert.equal(runtime.window.navigator.mediaDevices.getUserMedia, runtime.nativeGetUserMedia);
});

test("enabled reinjection restores preserved controls that React repurposed", () => {
  const runtime = loadShield(true);
  runtime.semantic.__reactProps$test = {
    children: { props: { id: "codex.command.somethingElse" } },
  };

  runtime.inject(true);

  assert.equal(runtime.semantic.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.semantic.getAttribute("aria-hidden"), null);
  assert.equal(runtime.semantic.style.display, undefined);
  assert.equal(runtime.semantic.disabled, false);
});

test("disabling voice slim mode restores native voice controls", () => {
  const runtime = loadShield(false);

  assert.equal(runtime.window.__codeyVoiceControlShield.enabled, false);
  assert.equal(runtime.window.__codeyVoiceControlShield.resourceGuardsInstalled, 0);
  assert.equal(runtime.semantic.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.localized.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.window.__codeyBlockNativeVoiceControls(), 0);
  assert.equal(runtime.window.navigator.mediaDevices.getUserMedia, runtime.nativeGetUserMedia);
  assert.equal(runtime.window.navigator.mediaDevices.enumerateDevices, runtime.nativeEnumerateDevices);
  assert.equal(runtime.window.RTCPeerConnection, runtime.NativeRTCPeerConnection);
  assert.equal(runtime.window.fetch, runtime.nativeFetch);
  assert.equal(runtime.window.WebSocket, runtime.NativeWebSocket);
});

test("voice slim mode prevents GPT Voice before WebRTC while preserving other peer connections", async () => {
  const runtime = loadShield(true);

  await assert.rejects(
    runtime.window.navigator.mediaDevices.getUserMedia({ audio: true }),
    (error) => error?.name === "NotAllowedError",
  );
  assert.deepEqual(runtime.mediaCalls, []);

  assert.equal(
    await runtime.window.navigator.mediaDevices.getUserMedia({ video: true }),
    "native-media",
  );
  assert.deepEqual(runtime.mediaCalls, [{ video: true }]);

  const devices = await runtime.window.navigator.mediaDevices.enumerateDevices();
  assert.equal(devices.map((device) => device.kind).join(","), "videoinput");
  assert.equal(runtime.enumerateDeviceCalls.length, 1);

  await assert.rejects(
    async () => {
      await runtime.window.navigator.mediaDevices.getUserMedia({ audio: true });
      return new runtime.window.RTCPeerConnection({ voice: true });
    },
    (error) => error?.name === "NotAllowedError",
  );
  assert.equal(runtime.rtcPeerConnectionCalls.length, 0);
  assert.ok(
    new runtime.window.RTCPeerConnection({ dataOnly: true })
      instanceof runtime.NativeRTCPeerConnection,
  );
  assert.equal(runtime.rtcPeerConnectionCalls.length, 1);

  await assert.rejects(
    runtime.window.fetch("https://chatgpt.com/backend-api/codex/dictation-stream-connect-info"),
    (error) => error?.name === "NotAllowedError",
  );
  assert.equal(await runtime.window.fetch("https://chatgpt.com/backend-api/models"), "native-fetch");
  assert.deepEqual(runtime.fetchCalls, ["https://chatgpt.com/backend-api/models"]);

  assert.throws(
    () => new runtime.window.WebSocket("wss://chatgpt.com/dictation/stream"),
    (error) => error?.name === "NotAllowedError",
  );
  const socket = new runtime.window.WebSocket("wss://chatgpt.com/other-stream");
  assert.equal(socket.url, "wss://chatgpt.com/other-stream");
  assert.deepEqual(runtime.webSocketCalls, ["wss://chatgpt.com/other-stream"]);
  assert.equal(runtime.window.__codeyVoiceControlShield.resourceGuardsInstalled, 4);

  runtime.window.__codeyVoiceControlShieldCleanup();
  assert.equal(runtime.window.navigator.mediaDevices.getUserMedia, runtime.nativeGetUserMedia);
  assert.equal(runtime.window.navigator.mediaDevices.enumerateDevices, runtime.nativeEnumerateDevices);
  assert.equal(runtime.window.RTCPeerConnection, runtime.NativeRTCPeerConnection);
  assert.equal(runtime.window.fetch, runtime.nativeFetch);
  assert.equal(runtime.window.WebSocket, runtime.NativeWebSocket);
  assert.equal(runtime.semantic.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.semantic.disabled, false);
  assert.equal(runtime.semantic.style.display, undefined);
  assert.equal(runtime.gptVoicePromotion.getAttribute("data-codey-voice-control-blocked"), null);
  assert.equal(runtime.gptVoicePromotion.style.display, undefined);
});
