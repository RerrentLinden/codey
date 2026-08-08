// Prompt-optimization button injected next to the Codex composer.
// The button is a fixed-position pill tracked to the composer input's
// top-right corner, so it never mutates Codex's own layout. The enabled flag
// is read from /settings/get at runtime, so the console switch applies
// without a Codex restart. All API traffic goes through the Codey bridge and
// never carries the configured API key into this page.
(() => {
  const moduleLoaded = window.__codeyPromptOptimizeModuleLoaded === true;
  window.__codeyPromptOptimizeModuleLoaded = true;
  if (moduleLoaded && window.__codeyPromptOptimize) {
    return;
  }

  const settingsPath = "/settings/get";
  const optimizePath = "/api/optimize_prompt";
  const buttonId = "codey-prompt-optimize-button";
  const styleId = "codey-prompt-optimize-style";
  const errorId = "codey-prompt-optimize-error";
  const configChangedEvent = "codey:config-changed";
  const optimizeTimeoutMs = 75_000;
  const scanDelayMs = 250;
  const repositionDelayMs = 100;
  const errorDismissMs = 6_000;
  const zIndex = 2147483640;
  const composerAnchorSelector = "[data-above-composer-conversation-id]";

  let enabled = false;
  let ready = false;
  let inputElement = null;
  let button = null;
  let errorPopover = null;
  let busy = false;
  let scanTimer = 0;
  let repositionTimer = 0;
  let configLoadTimer = 0;
  let configLoadBackoffMs = 120;
  let configLoadAttempts = 0;
  let observer = null;
  let trackedScrollRoot = null;

  const MAX_CONFIG_LOAD_ATTEMPTS = 10;

  const callBridge = (path, payload = {}) => {
    if (typeof window.__codexSessionDeleteBridge === "function") {
      return window.__codexSessionDeleteBridge(path, payload);
    }
    return Promise.resolve({ status: "failed", message: "Codey bridge 尚未就绪" });
  };

  const withTimeout = (promise, ms, message) => {
    let timer = 0;
    const timeout = new Promise((resolve) => {
      timer = setTimeout(() => resolve({ status: "failed", message }), ms);
    });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
  };

  const addStyle = () => {
    if (document.getElementById(styleId)) return;
    const style = document.createElement("style");
    style.id = styleId;
    style.textContent = `
      #${buttonId} {
        -webkit-app-region: no-drag !important;
        pointer-events: auto !important;
        position: fixed !important;
        z-index: ${zIndex} !important;
        display: none;
        align-items: center;
        gap: 5px;
        box-sizing: border-box;
        height: 26px;
        padding: 0 9px;
        border: 0;
        border-radius: 999px;
        background: rgba(30, 30, 30, .92);
        color: #f5f5f5;
        font: 12px/1 system-ui, -apple-system, "Segoe UI", sans-serif;
        cursor: pointer;
        user-select: none;
        box-shadow: 0 1px 4px rgba(0, 0, 0, .35);
        opacity: .88;
        transition: opacity .15s ease, transform .15s ease;
      }
      #${buttonId}:hover { opacity: 1; }
      #${buttonId}:active { transform: translateY(1px); }
      #${buttonId}[data-busy="true"] { opacity: .55; pointer-events: none; }
      #${buttonId} svg { flex: 0 0 auto; width: 13px; height: 13px; }
      #${errorId} {
        position: fixed !important;
        z-index: ${zIndex} !important;
        display: none;
        box-sizing: border-box;
        max-width: 300px;
        padding: 7px 10px;
        border-radius: 8px;
        background: rgba(200, 40, 40, .95);
        color: #fff;
        font: 12px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif;
        box-shadow: 0 2px 8px rgba(0, 0, 0, .4);
      }
    `;
    document.documentElement.appendChild(style);
  };

  const createButton = () => {
    const element = document.createElement("button");
    element.id = buttonId;
    element.type = "button";
    element.dataset.codeyPromptOptimize = "true";
    element.setAttribute("aria-label", "优化提示词");
    element.innerHTML = `
      <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill="none"
        stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9z"></path>
        <path d="M19 15l.9 2.4L22 18l-2.1.9L19 21l-.9-2.1L16 18l2.1-.6z"></path>
      </svg>
      <span>优化</span>
    `;
    element.addEventListener("click", handleClick, true);
    return element;
  };

  const createErrorPopover = () => {
    const element = document.createElement("div");
    element.id = errorId;
    element.setAttribute("role", "status");
    element.setAttribute("aria-live", "polite");
    document.documentElement.appendChild(element);
    return element;
  };

  const isComposerInput = (element) => {
    if (!element) return false;
    if (element.tagName === "TEXTAREA") return true;
    return element.isContentEditable === true;
  };

  const isVisible = (element) => {
    if (!isComposerInput(element)) return false;
    if (element.closest?.("[hidden], [aria-hidden='true']")) return false;
    if (element.disabled) return false;
    const style = window.getComputedStyle(element);
    if (style.display === "none" || style.visibility === "hidden") return false;
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };

  const findComposerInput = () => {
    const seen = new Set();
    for (const anchor of document.querySelectorAll(composerAnchorSelector)) {
      if (seen.has(anchor)) continue;
      seen.add(anchor);
      const scope = anchor.parentElement || anchor;
      const candidates = [
        ...scope.querySelectorAll("textarea, [contenteditable]"),
      ];
      for (const candidate of candidates) {
        if (isVisible(candidate)) return candidate;
      }
    }
    // Fallback: the largest visible textarea inside the main content area is
    // almost certainly the composer.
    let best = null;
    let bestArea = 0;
    for (const candidate of document.querySelectorAll("main textarea")) {
      if (!isVisible(candidate)) continue;
      const rect = candidate.getBoundingClientRect();
      const area = rect.width * rect.height;
      if (area > bestArea) {
        best = candidate;
        bestArea = area;
      }
    }
    return best;
  };

  const updateButtonPosition = () => {
    if (!button || !inputElement) return;
    const rect = inputElement.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) {
      button.style.display = "none";
      return;
    }
    const margin = 6;
    const buttonHeight = button.offsetHeight || 26;
    const buttonWidth = button.offsetWidth || 64;
    // 按钮位于输入框外部右上方：不遮输入框内的文字，也不挡输入框
    // 右下角的发送按键；上移 12px 避开输入框上缘的相邻控件。
    button.style.display = "inline-flex";
    button.style.top = `${Math.max(margin, rect.top - buttonHeight - margin * 2)}px`;
    button.style.left = `${Math.max(
      margin,
      Math.min(rect.right - buttonWidth - margin, window.innerWidth - buttonWidth - margin),
    )}px`;
    if (errorPopover && errorPopover.style.display !== "none") {
      positionErrorPopover();
    }
  };

  const positionErrorPopover = () => {
    if (!errorPopover || !button) return;
    const buttonRect = button.getBoundingClientRect();
    errorPopover.style.top = `${buttonRect.bottom + 6}px`;
    errorPopover.style.left = `${Math.max(
      8,
      Math.min(buttonRect.left, window.innerWidth - errorPopover.offsetWidth - 8),
    )}px`;
  };

  const showError = (message) => {
    if (!errorPopover) errorPopover = createErrorPopover();
    errorPopover.textContent = message;
    errorPopover.style.display = "block";
    positionErrorPopover();
    clearTimeout(errorPopover._dismissTimer);
    errorPopover._dismissTimer = setTimeout(() => {
      errorPopover.style.display = "none";
    }, errorDismissMs);
  };

  const readComposerText = () => {
    if (!inputElement) return "";
    if (inputElement.tagName === "TEXTAREA") {
      return inputElement.value;
    }
    return inputElement.innerText || "";
  };

  const replaceComposerText = (text) => {
    if (inputElement.tagName === "TEXTAREA") {
      const prototype = window.HTMLTextAreaElement?.prototype;
      const setter = prototype && Object.getOwnPropertyDescriptor(prototype, "value")?.set;
      if (setter) {
        setter.call(inputElement, text);
      } else {
        inputElement.value = text;
      }
      inputElement.dispatchEvent(new Event("input", { bubbles: true }));
      return;
    }
    inputElement.innerText = text;
    inputElement.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      inputType: "insertText",
      data: text,
    }));
  };

  const handleClick = (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (busy) return;
    const text = readComposerText().trim();
    if (!text) {
      showError("请先在输入框输入要优化的内容");
      return;
    }
    busy = true;
    button.dataset.busy = "true";
    const bridgeCall = callBridge(optimizePath, { text });
    const result = withTimeout(bridgeCall, optimizeTimeoutMs, "优化请求超时，请稍后重试");
    result
      .then((value) => {
        if (value?.status === "failed") {
          throw new Error(value.message || "优化失败");
        }
        const optimized = typeof value?.optimized === "string" ? value.optimized : "";
        if (!optimized) {
          throw new Error("优化结果为空");
        }
        replaceComposerText(optimized);
        if (inputElement?.focus) inputElement.focus();
      })
      .catch((error) => {
        const message = error instanceof Error ? error.message : String(error || "优化失败");
        showError(message);
      })
      .finally(() => {
        busy = false;
        button.dataset.busy = "false";
      });
  };

  const refreshButton = () => {
    if (!enabled) {
      if (button) button.style.display = "none";
      return;
    }
    const input = findComposerInput();
    if (input === inputElement && input) {
      updateButtonPosition();
      return;
    }
    inputElement = input || null;
    if (!inputElement) {
      if (button) button.style.display = "none";
      return;
    }
    if (!button) {
      button = createButton();
      document.documentElement.appendChild(button);
    }
    updateButtonPosition();
  };

  const scheduleScan = () => {
    clearTimeout(scanTimer);
    scanTimer = setTimeout(refreshButton, scanDelayMs);
  };

  const scheduleReposition = () => {
    clearTimeout(repositionTimer);
    repositionTimer = setTimeout(updateButtonPosition, repositionDelayMs);
  };

  const loadConfig = () => {
    configLoadAttempts += 1;
    callBridge(settingsPath, {})
      .then((config) => {
        configLoadAttempts = 0;
        configLoadBackoffMs = 120;
        let nextEnabled = false;
        try {
          const optimization = config?.promptOptimization;
          nextEnabled = optimization?.enabled === true
            && optimization.apiKeyConfigured === true;
          if (nextEnabled !== enabled) {
            enabled = nextEnabled;
            refreshButton();
          }
          if (enabled) refreshButton();
        } catch (error) {
          // A script-side error must not look like a missing bridge; report
          // it once and leave the switch in its last known state.
          if (typeof console !== "undefined" && typeof console.error === "function") {
            console.error("Codey 提示词优化脚本异常：", error);
          }
        }
        ready = true;
      })
      .catch(() => {
        // The bridge may not be ready during early startup; retry with
        // bounded backoff so the switch still applies once it is.
        if (configLoadAttempts >= MAX_CONFIG_LOAD_ATTEMPTS) return;
        clearTimeout(configLoadTimer);
        configLoadTimer = setTimeout(loadConfig, configLoadBackoffMs);
        configLoadBackoffMs = Math.min(configLoadBackoffMs * 2, 2_000);
      });
  };

  const installObserver = () => {
    observer = new MutationObserver(() => {
      if (enabled) scheduleScan();
    });
    observer.observe(document.documentElement, {
      childList: true,
      subtree: true,
      attributes: false,
    });
  };

  window.addEventListener(configChangedEvent, () => {
    ready = false;
    loadConfig();
  });
  window.addEventListener("scroll", scheduleReposition, true);
  window.addEventListener("resize", scheduleReposition);
  document.addEventListener("input", (event) => {
    if (event.target === inputElement) scheduleReposition();
  }, true);

  addStyle();
  installObserver();
  loadConfig();

  window.__codeyPromptOptimize = {
    snapshot: () => ({ ready: ready, enabled: enabled }),
  };
})();
