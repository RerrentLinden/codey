(() => {
  if (
    window.__codeyBridgeHelpersInstalled
    && window.__codeyMutationDispatcher?.createShieldLifecycle
  ) return;
  window.__codeyBridgeHelpersInstalled = true;

  const mutationSubscribers = new Map();
  let mutationObserver = null;
  let nextMutationSubscriberId = 1;

  const dispatchMutations = (mutations) => {
    for (const subscriber of [...mutationSubscribers.values()]) {
      try {
        subscriber.callback(mutations);
      } catch (error) {
        window.console?.error?.("[Codey] mutation subscriber failed", error);
      }
    }
  };

  const syncMutationObserver = () => {
    mutationObserver?.disconnect();
    mutationObserver = null;
    if (
      !mutationSubscribers.size
      || typeof MutationObserver !== "function"
      || !document.documentElement
    ) {
      return;
    }

    let attributes = false;
    let childList = false;
    let observeAllAttributes = false;
    const attributeFilter = new Set();
    for (const subscriber of mutationSubscribers.values()) {
      childList ||= subscriber.childList;
      attributes ||= subscriber.attributes;
      if (!subscriber.attributes) continue;
      if (subscriber.attributeFilter === null) {
        observeAllAttributes = true;
      } else {
        subscriber.attributeFilter.forEach((attribute) => attributeFilter.add(attribute));
      }
    }
    if (!attributes && !childList) return;

    const options = { attributes, childList, subtree: true };
    if (attributes && !observeAllAttributes && attributeFilter.size) {
      options.attributeFilter = [...attributeFilter];
    }
    mutationObserver = new MutationObserver(dispatchMutations);
    mutationObserver.observe(document.documentElement, options);
  };

  const subscribeMutations = (callback, options = {}) => {
    if (typeof callback !== "function") return () => {};
    const id = nextMutationSubscriberId;
    nextMutationSubscriberId += 1;
    const attributes = options.attributes === true;
    mutationSubscribers.set(id, {
      callback,
      attributes,
      childList: options.childList === true,
      attributeFilter: attributes && Array.isArray(options.attributeFilter)
        ? [...new Set(options.attributeFilter.map(String))]
        : null,
    });
    syncMutationObserver();

    let subscribed = true;
    return () => {
      if (!subscribed) return;
      subscribed = false;
      mutationSubscribers.delete(id);
      syncMutationObserver();
    };
  };

  const createShieldLifecycle = ({
    attributeFilter,
    block,
    eventSelector,
    isControl,
    mutationSelector = eventSelector,
  }) => {
    let flushTimer = 0;
    let cancelPendingFlush = null;
    let active = true;
    const pendingRoots = new Set();
    const pendingRootLimit = 64;
    const documentRoot = document.documentElement || document.body;

    const addPendingRoot = (root) => {
      if (!(root instanceof HTMLElement) || pendingRoots.has(root)) return;
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
        root.matches?.(mutationSelector) || root.querySelector?.(mutationSelector);
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

    const mutationRoot = (node) => {
      if (node instanceof HTMLElement) return node;
      return node?.parentElement instanceof HTMLElement ? node.parentElement : null;
    };
    const unsubscribeMutations = document.documentElement
      ? subscribeMutations((mutations) => {
        for (const mutation of mutations) {
          const target = mutationRoot(mutation.target);
          const containingControl = target?.closest?.(mutationSelector);
          if (containingControl) queueMutationRoot(containingControl);
          if (mutation.type === "attributes") continue;
          for (const node of mutation.addedNodes || []) {
            queueMutationRoot(mutationRoot(node));
          }
        }
        if (pendingRoots.size) schedulePendingFlush();
      }, {
        attributes: true,
        attributeFilter,
        childList: true,
      })
      : null;

    const stopControlEvent = (event) => {
      const control = event.target instanceof Element
        ? event.target.closest(eventSelector)
        : null;
      if (!isControl(control)) return;
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation?.();
    };
    const eventNames = ["pointerdown", "click", "keydown"];
    eventNames.forEach((eventName) => {
      document.addEventListener(eventName, stopControlEvent, true);
    });

    return Object.freeze({
      cleanup: () => {
        if (!active) return;
        active = false;
        unsubscribeMutations?.();
        if (flushTimer) cancelPendingFlush?.();
        flushTimer = 0;
        cancelPendingFlush = null;
        pendingRoots.clear();
        eventNames.forEach((eventName) => {
          document.removeEventListener(eventName, stopControlEvent, true);
        });
      },
      observerInstalled: unsubscribeMutations !== null,
    });
  };

  window.__codeyMutationDispatcher = Object.freeze({
    createShieldLifecycle,
    snapshot: () => Object.freeze({
      observerInstalled: mutationObserver !== null,
      subscriberCount: mutationSubscribers.size,
    }),
    subscribe: subscribeMutations,
  });
  window.__codeyCall = (path, payload = {}) => {
    if (typeof window.__codexSessionDeleteBridge === "function") {
      return window.__codexSessionDeleteBridge(path, payload);
    }
    return Promise.resolve({ status: "failed", message: "Codey bridge unavailable" });
  };
  window.__codeyRefreshSession = (detail = {}) => window.dispatchEvent(new CustomEvent("codey-session-refresh", { detail }));
})();
