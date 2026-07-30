(() => {
  const enabled = "__CODEY_SLIM_VOICE__" === "true";
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
  const reactInternalKeyPattern = /^__(?:reactProps|reactFiber|reactInternalInstance)\$.*/;
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

    const nativeFetch = window.fetch;
    if (typeof nativeFetch === "function") {
      const guardedFetch = function guardedFetch(input) {
        const url = typeof input === "string" || input instanceof URL
          ? String(input)
          : String(input?.url ?? "");
        if (dictationRequestPattern.test(url)) return Promise.reject(disabledVoiceError());
        return Reflect.apply(nativeFetch, this, arguments);
      };
      window.fetch = guardedFetch;
      restoreResourceGuards.push(() => {
        if (window.fetch === guardedFetch) window.fetch = nativeFetch;
      });
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

  const reactTraversalKeys = new Set(["return", "child", "sibling", "stateNode", "_owner"]);
  const reactAncestorTraversalKeys = new Set([...reactTraversalKeys, "children"]);

  const containsMatchingValue = (
    value,
    predicate,
    depth = 0,
    seen = new WeakSet(),
    ignoredKeys = reactTraversalKeys,
  ) => {
    if (typeof value === "string") return predicate(value);
    if (!value || typeof value !== "object" || depth > 7 || seen.has(value)) return false;
    seen.add(value);
    for (const [key, child] of Object.entries(value)) {
      if (ignoredKeys.has(key)) continue;
      if (containsMatchingValue(child, predicate, depth + 1, seen, ignoredKeys)) return true;
    }
    return false;
  };

  const hasMatchingReactValue = (control, predicate) =>
    Object.keys(control)
      .filter((key) => reactInternalKeyPattern.test(key))
      .some((key) => {
        try {
          const internal = control[key];
          if (containsMatchingValue(internal?.memoizedProps ?? internal, predicate)) return true;

          let ancestor = internal?.memoizedProps ? internal.return : null;
          for (let depth = 0; ancestor && depth < 8; depth += 1) {
            if (
              containsMatchingValue(
                ancestor.memoizedProps,
                predicate,
                0,
                new WeakSet(),
                reactAncestorTraversalKeys,
              )
            ) {
              return true;
            }
            ancestor = ancestor.return;
          }
          return false;
        } catch {
          return false;
        }
      });

  // Verdicts are deliberately not memoised. `restoreRepurposedVoiceControls`
  // depends on seeing a control stop being a voice control, and the walk above
  // reads every __reactProps$/__reactFiber$ key plus eight ancestor fibers, so
  // no cheap identity token covers what the verdict actually depends on.
  const isVoiceControl = (control) => {
    if (!(control instanceof HTMLElement)) return false;
    const descriptor = [
      control.getAttribute("aria-label"),
      control.getAttribute("title"),
      control.textContent,
    ].filter(Boolean).join(" ").replace(/\s+/g, " ").trim();
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

  const controlsWithin = (root, selector) => {
    const controls = [];
    if (root instanceof HTMLElement && root.matches?.(selector)) controls.push(root);
    if (root && typeof root.querySelectorAll === "function") {
      controls.push(...root.querySelectorAll(selector));
    }
    return controls;
  };

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

  let controlObserver = null;
  let flushTimer = 0;
  let cancelPendingFlush = null;
  const pendingRoots = new Set();
  const pendingRootLimit = 64;

  const addPendingRoot = (root) => {
    if (!(root instanceof HTMLElement) || pendingRoots.has(root)) return;
    const documentRoot = document.documentElement || document.body;
    if (documentRoot && pendingRoots.has(documentRoot)) return;
    if (pendingRoots.size >= pendingRootLimit && documentRoot) {
      pendingRoots.clear();
      pendingRoots.add(documentRoot);
      return;
    }
    for (const pending of pendingRoots) {
      if (pending.contains?.(root)) return;
    }
    for (const pending of [...pendingRoots]) {
      if (root.contains?.(pending)) pendingRoots.delete(pending);
    }
    pendingRoots.add(root);
  };

  const flushPendingRoots = () => {
    flushTimer = 0;
    cancelPendingFlush = null;
    if (!pendingRoots.size) return;
    const roots = [...pendingRoots];
    pendingRoots.clear();
    for (const root of roots) {
      if (root.isConnected === false) continue;
      block(root);
    }
  };

  const blockBeforePaint = (root) => {
    if (!(root instanceof HTMLElement) || root.isConnected === false) return 0;
    const hasControlCandidate =
      root.matches?.(mutationCandidateSelector) || root.querySelector?.(mutationCandidateSelector);
    return hasControlCandidate ? block(root) : 0;
  };

  const queueMutationRoot = (root) => {
    if (!(root instanceof HTMLElement)) return;
    if (blockBeforePaint(root) > 0) return;
    addPendingRoot(root);
  };

  const schedulePendingFlush = () => {
    if (flushTimer) return;
    if (typeof window.requestAnimationFrame === "function") {
      flushTimer = window.requestAnimationFrame(flushPendingRoots);
      cancelPendingFlush = () => window.cancelAnimationFrame?.(flushTimer);
      return;
    }
    if (typeof window.setTimeout !== "function") {
      flushPendingRoots();
      return;
    }
    flushTimer = window.setTimeout(flushPendingRoots, 0);
    cancelPendingFlush = () => window.clearTimeout?.(flushTimer);
  };

  if (typeof MutationObserver === "function" && document.documentElement) {
    const mutationRoot = (node) => {
      if (node instanceof HTMLElement) return node;
      return node?.parentElement instanceof HTMLElement ? node.parentElement : null;
    };
    controlObserver = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        const target = mutationRoot(mutation.target);
        if (mutation.type === "attributes") {
          const containingControl = target?.closest?.(mutationCandidateSelector);
          if (containingControl) queueMutationRoot(containingControl);
          continue;
        }
        const containingControl = target?.closest?.(mutationCandidateSelector);
        if (containingControl) queueMutationRoot(containingControl);
        for (const node of mutation.addedNodes || []) {
          queueMutationRoot(mutationRoot(node));
        }
      }
      if (pendingRoots.size) schedulePendingFlush();
    });
    controlObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["aria-label", "role", "title", "src"],
      childList: true,
      subtree: true,
    });
  }

  const stopVoiceControlEvent = (event) => {
    const control = event.target instanceof Element
      ? event.target.closest(voiceControlSelector)
      : null;
    if (!isVoiceControl(control)) return;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation?.();
  };

  const eventNames = ["pointerdown", "click", "keydown"];
  eventNames.forEach((eventName) => {
    document.addEventListener(eventName, stopVoiceControlEvent, true);
  });
  window.__codeyBlockNativeVoiceControls = block;
  window.__codeyVoiceControlShield = Object.freeze({
    enabled,
    block,
    isVoiceControl,
    observerInstalled: controlObserver !== null,
    resourceGuardsInstalled: restoreResourceGuards.length,
  });
  window.__codeyVoiceControlShieldCleanup = (options = {}) => {
    controlObserver?.disconnect();
    if (flushTimer) cancelPendingFlush?.();
    flushTimer = 0;
    cancelPendingFlush = null;
    pendingRoots.clear();
    eventNames.forEach((eventName) => {
      document.removeEventListener(eventName, stopVoiceControlEvent, true);
    });
    restoreResourceGuards.splice(0).reverse().forEach((restore) => restore());
    if (options?.restoreControls !== false) restoreBlockedElements();
    delete window.__codeyBlockNativeVoiceControls;
    delete window.__codeyVoiceControlShield;
    delete window.__codeyVoiceControlShieldCleanup;
  };
  block();
})();
