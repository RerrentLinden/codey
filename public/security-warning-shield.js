(() => {
  if (window.__codeySecurityWarningShieldInstalled) return;
  window.__codeySecurityWarningShieldInstalled = true;

  const configEventName = "codey:config-changed";
  const dismissedAttribute = "data-codey-security-warning-dismissed";
  const actionPatterns = [
    /^hide from this session$/i,
    /^don['’]t show again$/i,
    /^(?:在|于)?本次会话(?:中)?(?:隐藏|不再显示)$/,
    /^(?:隐藏|不再显示)(?:本次会话)?$/,
  ];
  const titlePatterns = [
    /full access is on/i,
    /完全访问权限.*(?:已开启|开启中|已打开)/,
  ];
  const riskPatterns = [
    /without your permission/i,
    /without your approval/i,
    /risk of data loss/i,
    /prompt injection/i,
    /未经(?:你|您)的许可/,
    /提示词?注入/,
  ];
  let enabled = false;
  let scanTimer = 0;
  let unsubscribeMutations = null;

  // innerText forces a layout flush; the action labels this shield matches are
  // plain text nodes, so textContent is both sufficient and layout-free.
  const normalizedText = (element) => String(
    element?.textContent || "",
  ).replace(/\s+/g, " ").trim();

  const matchesAny = (value, patterns) => patterns.some((pattern) => pattern.test(value));

  const warningContainerFor = (control) => {
    let candidate = control?.parentElement || null;
    for (let depth = 0; candidate && depth < 8; depth += 1) {
      if (candidate === document.body || candidate === document.documentElement) break;
      const text = normalizedText(candidate);
      if (matchesAny(text, titlePatterns) && matchesAny(text, riskPatterns)) {
        return candidate;
      }
      candidate = candidate.parentElement;
    }
    return null;
  };

  const actionControls = (root = document) => {
    const controls = [];
    if (root instanceof Element && root.matches?.("button, [role=button]")) {
      controls.push(root);
    }
    if (typeof root?.querySelectorAll === "function") {
      controls.push(...root.querySelectorAll("button, [role=button]"));
    }
    return controls;
  };

  const dismissWarnings = (root = document) => {
    if (!enabled) return 0;
    let dismissed = 0;
    for (const control of actionControls(root)) {
      if (
        control.disabled
        || control.getAttribute?.(dismissedAttribute) === "true"
        || !matchesAny(normalizedText(control), actionPatterns)
      ) {
        continue;
      }
      const container = warningContainerFor(control);
      if (!container) continue;
      control.setAttribute?.(dismissedAttribute, "true");
      container.setAttribute?.(dismissedAttribute, "true");
      control.click?.();
      if (container.isConnected !== false) {
        container.style?.setProperty?.("display", "none", "important");
      }
      dismissed += 1;
    }
    return dismissed;
  };

  const setEnabled = (next) => {
    enabled = next === true;
    if (enabled) {
      ensureObserver();
      dismissWarnings();
    } else {
      if (scanTimer) {
        window.clearTimeout?.(scanTimer);
        scanTimer = 0;
      }
      pendingRoots.clear();
      unsubscribeMutations?.();
      unsubscribeMutations = null;
    }
    return enabled;
  };

  const refreshConfig = async () => {
    if (typeof window.__codexSessionDeleteBridge !== "function") {
      return setEnabled(false);
    }
    try {
      const config = await window.__codexSessionDeleteBridge("/settings/get", {});
      return setEnabled(config?.hideFullAccessWarning === true);
    } catch {
      return setEnabled(false);
    }
  };

  const pendingRoots = new Set();
  const pendingRootLimit = 32;

  const addPendingRoot = (root) => {
    if (!(root instanceof Element)) return;
    if (pendingRoots.has(document.documentElement)) return;
    if (pendingRoots.size >= pendingRootLimit) {
      pendingRoots.clear();
      pendingRoots.add(document.documentElement);
      return;
    }
    pendingRoots.add(root);
  };

  const scheduleScan = () => {
    if (!enabled || scanTimer) return;
    scanTimer = window.setTimeout(() => {
      scanTimer = 0;
      if (!pendingRoots.size) {
        dismissWarnings();
        return;
      }
      const roots = [...pendingRoots];
      pendingRoots.clear();
      // Scanning only the inserted subtrees avoids a full-document button sweep
      // on every batch of DOM churn.
      roots.forEach((root) => {
        if (root.isConnected === false) return;
        dismissWarnings(root);
      });
    }, 40);
  };

  const ensureObserver = () => {
    if (
      unsubscribeMutations
      || !window.__codeyMutationDispatcher
      || !document.documentElement
    ) {
      return;
    }
    unsubscribeMutations = window.__codeyMutationDispatcher.subscribe((mutations) => {
      if (!enabled) return;
      let added = false;
      for (const mutation of mutations) {
        if (!(mutation.addedNodes?.length > 0)) continue;
        added = true;
        for (const node of mutation.addedNodes) {
          if (node instanceof Element) addPendingRoot(node);
        }
        // The warning label often arrives as a text node inside an existing
        // button, and an inserted element may sit below the button rather than
        // contain it, so the mutation target has to be scanned as well.
        const target = mutation.target instanceof Element
          ? mutation.target
          : mutation.target?.parentElement;
        if (target) addPendingRoot(target.closest?.("button, [role=button]") || target);
      }
      if (added) scheduleScan();
    }, { childList: true });
  };

  window.addEventListener?.(configEventName, (event) => {
    const config = event?.detail?.config || event?.detail;
    if (config && typeof config.hideFullAccessWarning === "boolean") {
      setEnabled(config.hideFullAccessWarning);
    } else {
      void refreshConfig();
    }
  });
  window.__codeySecurityWarningShield = {
    get enabled() {
      return enabled;
    },
    dismissWarnings,
    refreshConfig,
    setEnabled,
  };

  void refreshConfig();
})();
