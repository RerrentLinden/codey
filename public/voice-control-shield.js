(() => {
  const enabled = ["__CODEY_SLIM_VOICE__"][0] === "true";
  const preserveBlockedControls = enabled && window.__codeyVoiceControlShield?.enabled === true;
  window.__codeyVoiceControlShieldCleanup?.({ restoreControls: !preserveBlockedControls });

  const voiceControlIds = new Set([
    "codex.command.composer.startDictation",
    "codex.command.composer.startVoiceMode",
    "codex.command.realtimeVoice",
    "codex.commandDescription.composer.startDictation",
    "codex.commandDescription.composer.startVoiceMode",
    "codex.commandDescription.globalDictationHold",
    "codex.commandDescription.globalDictationToggle",
    "codex.commandDescription.realtimeVoice",
    "codex.commandMenuTitle.composer.startDictation",
    "codex.command.globalDictationHold",
    "codex.command.globalDictationToggle",
    "composer.startDictation",
    "composer.startVoiceMode",
    "globalDictationHold",
    "globalDictationToggle",
    "settings.general.globalDictationHotkey",
    "settings.general.globalDictationToggleHotkey",
    "settings.general.globalDictationKeepVisible",
    "settings.general.dictation",
    "settings.nav.voice",
    "settings.section.voice",
    "settings.general.voice",
    "settings.general.realtimeVoice",
    "settings.general.realtimeVoiceHotkey",
    "settings.general.realtimeVoiceScreenContext",
    "realtimeVoice",
  ]);
  const voiceControlIdPrefixes = [
    "codex.command.realtimeVoice.",
    "codex.commandDescription.realtimeVoice.",
    "composer.realtime.",
    "realtimeVoice.",
    "settings.general.globalDictationHotkey.",
    "settings.general.globalDictationToggleHotkey.",
    "settings.general.globalDictation.",
    "settings.general.globalDictationHistory.",
    "settings.general.dictationDictionary.",
    "settings.general.realtimeVoice.",
    "settings.general.realtimeVoiceHotkey.",
    "settings.general.realtimeVoiceScreenContext.",
    "composer.dictation.",
    "settings.voice.",
  ];
  const gptVoicePromotionIdPrefixes = ["realtimeVoice.homeAnnouncement."];
  const fallbackLabelPattern = /^(?:try (?:chatgpt|codex) voice|voice|voice chat|voice chat hotkey|voice mode|start (?:new )?voice(?: chat| mode)?|stop voice chat|end voice chat|cancel voice chat|open the voice (?:chat )?control window|start or stop voice (?:chat|mode)|mute (?:your microphone|voice chat)|unmute (?:your microphone|voice chat)|dictate|dictation|start dictation|stop dictation|transcribing|dismiss dictation|retry dictation|click to dictate or hold|hold(?:-| )to(?:-| )dictate|toggle dictation|global dictation|体验\s*(?:chatgpt|codex)?\s*语音|试试\s*(?:chatgpt|codex)?\s*语音|语音|语音聊天|开始(?:新(?:的)?\s*)?语音(?:聊天|模式)?|停止语音聊天|结束语音聊天|打开语音控制窗口|听写|开始听写|停止听写|正在转录|关闭听写|重试听写|全局听写|按住听写|切换听写|體驗\s*(?:chatgpt|codex)?\s*語音|試試\s*(?:chatgpt|codex)?\s*語音|語音|語音聊天|開始(?:新(?:的)?\s*)?語音(?:聊天|模式)?|停止語音聊天|結束語音聊天|開啟語音控制視窗|聽寫|開始聽寫|停止聽寫|正在轉錄|關閉聽寫|重試聽寫|全域聽寫)(?:\s*[(:（].*)?$/i;
  const preservedComposerActionPattern =
    /^(?:send|send message|submit|stop|发送|发送消息|提交|停止|傳送|傳送訊息|提交|停止)$/i;
  const dictationRequestPattern = /(?:\/codex\/dictation-stream-connect-info|\/dictation\/stream)(?:[/?#]|$)/i;
  const gptVoicePromotionAssetPattern = /(?:^|\/)[^/?#]*(?:bidi[^/?#]*banner|voice[^/?#]*banner)[^/?#]*\.(?:avif|gif|jpe?g|png|webp)(?:[?#]|$)/i;
  const voiceControlSelector =
    "button, [role=button], [role=menuitem], [role=option], [role=switch], input, label";
  const mutationCandidateSelector = `${voiceControlSelector}, img`;
  const blockedElementStateKey = "__codeyVoiceControlOriginalState";
  const restoreResourceGuards = [];
  const blockedElementStates = new Map();

  const disabledVoiceError = () => {
    const error = new Error("Codex voice is disabled by Codey");
    error.name = "NotAllowedError";
    return error;
  };

  if (enabled) {
    const mediaDevices = window.navigator?.mediaDevices;
    const nativeGetUserMedia = mediaDevices?.getUserMedia;
    if (typeof nativeGetUserMedia === "function") {
      const guardedGetUserMedia = function guardedGetUserMedia(constraints) {
        if (constraints?.audio) return Promise.reject(disabledVoiceError());
        return Reflect.apply(nativeGetUserMedia, mediaDevices, arguments);
      };
      try {
        mediaDevices.getUserMedia = guardedGetUserMedia;
        restoreResourceGuards.push(() => {
          if (mediaDevices.getUserMedia === guardedGetUserMedia) {
            mediaDevices.getUserMedia = nativeGetUserMedia;
          }
        });
      } catch {}
    }

    const nativeEnumerateDevices = mediaDevices?.enumerateDevices;
    if (typeof nativeEnumerateDevices === "function") {
      const guardedEnumerateDevices = async function guardedEnumerateDevices() {
        const devices = await Reflect.apply(nativeEnumerateDevices, mediaDevices, arguments);
        return Array.from(devices ?? []).filter(
          (device) => device?.kind !== "audioinput" && device?.kind !== "audiooutput",
        );
      };
      try {
        mediaDevices.enumerateDevices = guardedEnumerateDevices;
        restoreResourceGuards.push(() => {
          if (mediaDevices.enumerateDevices === guardedEnumerateDevices) {
            mediaDevices.enumerateDevices = nativeEnumerateDevices;
          }
        });
      } catch {}
    }

    const registerFetchInterceptor = window.__codeySharedRuntime?.registerFetchInterceptor;
    if (typeof registerFetchInterceptor === "function") {
      const unregisterFetchInterceptor = registerFetchInterceptor(
        "voice-control-shield",
        (next, ...args) => {
          const input = args[0];
          const url = typeof input === "string" || input instanceof URL
            ? String(input)
            : String(input?.url ?? "");
          if (dictationRequestPattern.test(url)) return Promise.reject(disabledVoiceError());
          return next(...args);
        },
        20,
      );
      restoreResourceGuards.push(unregisterFetchInterceptor);
    }

    const NativeWebSocket = window.WebSocket;
    if (typeof NativeWebSocket === "function") {
      const GuardedWebSocket = new Proxy(NativeWebSocket, {
        construct(target, argumentsList, newTarget) {
          if (dictationRequestPattern.test(String(argumentsList[0] ?? ""))) {
            throw disabledVoiceError();
          }
          return Reflect.construct(target, argumentsList, newTarget);
        },
      });
      window.WebSocket = GuardedWebSocket;
      restoreResourceGuards.push(() => {
        if (window.WebSocket === GuardedWebSocket) window.WebSocket = NativeWebSocket;
      });
    }
  }

  const isVoiceControlId = (value) =>
    voiceControlIds.has(value) ||
    voiceControlIdPrefixes.some((prefix) => value.startsWith(prefix));

  const reactAncestorTraversalKeys =
    new Set(["return", "child", "sibling", "stateNode", "_owner", "children"]);

  const hasMatchingReactValue = (control, predicate) =>
    window.__codeySharedRuntime.reactInternalGraphIncludes(control, predicate, {
      ancestorDepth: 8,
      ancestorIgnoredKeys: reactAncestorTraversalKeys,
    });

  // Verdicts are deliberately not memoised. `restoreRepurposedVoiceControls`
  // depends on seeing a control stop being a voice control, and the shared walk
  // reads every __reactProps$/__reactFiber$ key plus eight ancestor fibers, so
  // no cheap identity token covers what the verdict actually depends on.
  const isVoiceControl = (control) => {
    if (!(control instanceof HTMLElement)) return false;
    const descriptor = window.__codeyMutationDispatcher.controlDescriptor(control);
    if (preservedComposerActionPattern.test(descriptor)) return false;
    if (fallbackLabelPattern.test(descriptor)) return true;

    return hasMatchingReactValue(control, isVoiceControlId);
  };

  const isGptVoicePromotionControl = (control) =>
    control instanceof HTMLElement
      && hasMatchingReactValue(
        control,
        (value) => gptVoicePromotionIdPrefixes.some((prefix) => value.startsWith(prefix)),
      );

  const controlsWithin = window.__codeyMutationDispatcher.controlsWithin;

  const findGptVoicePromotionRoot = (asset) => {
    let promotion = asset;
    let current = asset?.parentElement;
    for (let depth = 0; current instanceof HTMLElement && depth < 8; depth += 1) {
      if (current === document.body || current === document.documentElement) break;
      const buttons = current.querySelectorAll?.("button, [role=button]") ?? [];
      const editors = current.querySelectorAll?.(
        "input, textarea, [contenteditable]:not([contenteditable=false])",
      ) ?? [];
      const textLength = current.textContent?.replace(/\s+/g, " ").trim().length ?? 0;
      if (buttons.length > 0 && buttons.length <= 4 && editors.length === 0 && textLength <= 1_000) {
        promotion = current;
      } else if (promotion !== asset) {
        break;
      }
      current = current.parentElement;
    }
    return promotion;
  };

  const fullyBlocked = (element) =>
    element.getAttribute("data-codey-voice-control-blocked") === "true"
      && element.getAttribute("aria-hidden") === "true"
      && element.getAttribute("tabindex") === "-1"
      && element.getAttribute("inert") !== null
      && String(element.style.display || "").startsWith("none")
      && (!("disabled" in element) || element.disabled);

  const captureBlockedElementState = (element) => ({
    attributes: new Map(
      ["data-codey-voice-control-blocked", "aria-hidden", "tabindex", "inert"]
        .map((name) => [name, element.getAttribute(name)]),
    ),
    disabled: "disabled" in element ? element.disabled : undefined,
    display: element.style.getPropertyValue?.("display") ?? element.style.display ?? "",
    displayPriority: element.style.getPropertyPriority?.("display") ?? "",
  });

  const setStoredBlockedElementState = (element, state) => {
    try {
      Object.defineProperty(element, blockedElementStateKey, {
        configurable: true,
        value: state,
      });
    } catch {
      try {
        element[blockedElementStateKey] = state;
      } catch {}
    }
  };

  const clearStoredBlockedElementState = (element) => {
    try {
      delete element[blockedElementStateKey];
    } catch {}
  };

  const rememberBlockedElement = (element) => {
    if (!blockedElementStates.has(element)) {
      const state = element[blockedElementStateKey] ?? captureBlockedElementState(element);
      blockedElementStates.set(element, state);
      if (!element[blockedElementStateKey]) setStoredBlockedElementState(element, state);
    }
  };

  const blockElement = (element) => {
    rememberBlockedElement(element);
    if (fullyBlocked(element)) return;
    element.setAttribute("data-codey-voice-control-blocked", "true");
    element.setAttribute("aria-hidden", "true");
    element.setAttribute("tabindex", "-1");
    element.setAttribute("inert", "");
    element.style.setProperty("display", "none", "important");
    if ("disabled" in element && !element.disabled) element.disabled = true;
  };

  const restoreBlockedElement = (element) => {
    const state = blockedElementStates.get(element);
    if (!state) return;
    state.attributes.forEach((value, name) => {
      if (value === null) element.removeAttribute(name);
      else element.setAttribute(name, value);
    });
    element.style.removeProperty?.("display");
    if (state.display) {
      element.style.setProperty("display", state.display, state.displayPriority);
    }
    if (state.disabled !== undefined) element.disabled = state.disabled;
    blockedElementStates.delete(element);
    clearStoredBlockedElementState(element);
  };

  const restoreRepurposedVoiceControls = () => {
    blockedElementStates.forEach((_state, element) => {
      if (
        element.isConnected === false
        || (element.matches?.(voiceControlSelector) && !isVoiceControl(element))
      ) {
        restoreBlockedElement(element);
      }
    });
  };

  const block = (root = document) => {
    if (!enabled) return 0;
    restoreRepurposedVoiceControls();
    const blockedElements = new Set();
    controlsWithin(
      root,
      voiceControlSelector,
    ).forEach((control) => {
      if (!isVoiceControl(control)) {
        if (control[blockedElementStateKey]) {
          rememberBlockedElement(control);
          restoreBlockedElement(control);
        }
        return;
      }
      const target = isGptVoicePromotionControl(control)
        ? findGptVoicePromotionRoot(control)
        : control;
      blockElement(target);
      blockedElements.add(target);
    });
    controlsWithin(root, "img").forEach((asset) => {
      if (!gptVoicePromotionAssetPattern.test(String(asset.getAttribute("src") ?? ""))) return;
      const promotion = findGptVoicePromotionRoot(asset);
      blockElement(promotion);
      blockedElements.add(promotion);
    });
    return blockedElements.size;
  };

  const restoreBlockedElements = () => {
    [...blockedElementStates.keys()].forEach(restoreBlockedElement);
  };

  if (!enabled) {
    window.__codeyBlockNativeVoiceControls = () => 0;
    window.__codeyVoiceControlShield = Object.freeze({
      enabled,
      block: () => 0,
      isVoiceControl,
      resourceGuardsInstalled: 0,
    });
    window.__codeyVoiceControlShieldCleanup = () => {
      restoreBlockedElements();
      delete window.__codeyBlockNativeVoiceControls;
      delete window.__codeyVoiceControlShield;
      delete window.__codeyVoiceControlShieldCleanup;
    };
    return;
  }

  const shieldLifecycle = window.__codeyMutationDispatcher?.createShieldLifecycle({
    attributeFilter: ["aria-label", "role", "title", "src"],
    block,
    eventSelector: voiceControlSelector,
    isControl: isVoiceControl,
    mutationSelector: mutationCandidateSelector,
  });
  window.__codeyBlockNativeVoiceControls = block;
  window.__codeyVoiceControlShield = Object.freeze({
    enabled,
    block,
    isVoiceControl,
    observerInstalled: shieldLifecycle?.observerInstalled === true,
    resourceGuardsInstalled: restoreResourceGuards.length,
  });
  window.__codeyVoiceControlShieldCleanup = (options = {}) => {
    shieldLifecycle?.cleanup();
    restoreResourceGuards.splice(0).reverse().forEach((restore) => restore());
    if (options?.restoreControls !== false) restoreBlockedElements();
    delete window.__codeyBlockNativeVoiceControls;
    delete window.__codeyVoiceControlShield;
    delete window.__codeyVoiceControlShieldCleanup;
  };
  block();
})();
