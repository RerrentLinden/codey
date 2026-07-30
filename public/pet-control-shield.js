(() => {
  window.__codeyPetControlShieldCleanup?.();

  const enabled = "__CODEY_SLIM_PET__" === "true";
  const petControlIds = new Set([
    "settings.personalization.pets.openPet",
    "settings.personalization.pets.tuckAwayPet",
    "codex.profileFooter.showPet",
    "codex.profileFooter.hidePet",
    "codex.command.openPetOverlay",
    "codex.command.tuckAwayPetOverlay",
    "openAvatarOverlay",
    "tuckAwayAvatarOverlay",
    "avatar-overlay-open",
  ]);
  const fallbackLabelPattern = /^(?:wake pet|show pet|tuck away pet|hide pet|唤醒宠物|显示宠物|收起宠物|隐藏宠物|喚醒寵物|顯示寵物|收起寵物|隱藏寵物)$/i;
  const reactInternalKeyPattern = /^__(?:reactProps|reactFiber|reactInternalInstance)\$.*/;
  const controlSelector = "button, [role=button], [role=menuitem]";

  const containsPetControlId = (value, depth = 0, seen = new WeakSet()) => {
    if (typeof value === "string") return petControlIds.has(value);
    if (!value || typeof value !== "object" || depth > 7 || seen.has(value)) return false;
    seen.add(value);
    for (const [key, child] of Object.entries(value)) {
      if (["return", "child", "sibling", "stateNode", "_owner"].includes(key)) continue;
      if (containsPetControlId(child, depth + 1, seen)) return true;
    }
    return false;
  };

  const isPetControl = (control) => {
    if (!(control instanceof HTMLElement)) return false;
    const descriptor = [
      control.getAttribute("aria-label"),
      control.getAttribute("title"),
      control.textContent,
    ].filter(Boolean).join(" ").replace(/\s+/g, " ").trim();
    if (fallbackLabelPattern.test(descriptor)) return true;

    // Deliberately not memoised. React reuses host elements and swaps both
    // __reactProps$ and __reactFiber$ independently, and the walk below reads
    // every matching key, so no cheap identity token covers the whole verdict.
    // At ~3 µs per control the walk is not worth a fail-open cache on a shield
    // that has to fail closed; the observer throttling above is what keeps this
    // off the streaming hot path.
    return Object.keys(control)
      .filter((key) => reactInternalKeyPattern.test(key))
      .some((key) => {
        try {
          const internal = control[key];
          return containsPetControlId(internal?.memoizedProps ?? internal);
        } catch {
          return false;
        }
      });
  };

  const controlsWithin = (root, selector) => {
    const controls = [];
    if (root instanceof HTMLElement && root.matches?.(selector)) controls.push(root);
    if (root && typeof root.querySelectorAll === "function") {
      controls.push(...root.querySelectorAll(selector));
    }
    return controls;
  };

  const block = (root = document) => {
    if (!enabled) return 0;
    let blocked = 0;
    controlsWithin(root, controlSelector).forEach((control) => {
      if (!isPetControl(control)) return;
      const fullyBlocked = control.getAttribute("data-codey-pet-control-blocked") === "true"
        && control.getAttribute("aria-hidden") === "true"
        && control.getAttribute("tabindex") === "-1"
        && control.getAttribute("inert") !== null
        && String(control.style.display || "").startsWith("none")
        && (!("disabled" in control) || control.disabled);
      if (!fullyBlocked) {
        control.setAttribute("data-codey-pet-control-blocked", "true");
        control.setAttribute("aria-hidden", "true");
        control.setAttribute("tabindex", "-1");
        control.setAttribute("inert", "");
        control.style.setProperty("display", "none", "important");
        if ("disabled" in control && !control.disabled) control.disabled = true;
      }
      blocked += 1;
    });
    return blocked;
  };

  const blockBeforePaint = (root) => {
    if (!(root instanceof HTMLElement) || root.isConnected === false) return 0;
    const hasControlCandidate =
      root.matches?.(controlSelector) || root.querySelector?.(controlSelector);
    return hasControlCandidate ? block(root) : 0;
  };

  if (!enabled) {
    window.__codeyBlockNativePetControls = () => 0;
    window.__codeyPetControlShield = Object.freeze({ enabled, block: () => 0, isPetControl });
    window.__codeyPetControlShieldCleanup = () => {
      delete window.__codeyBlockNativePetControls;
      delete window.__codeyPetControlShield;
      delete window.__codeyPetControlShieldCleanup;
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
    if (pendingRoots.has(document.documentElement)) return;
    if (pendingRoots.size >= pendingRootLimit) {
      // Collapse instead of tracking an unbounded root set during heavy
      // streaming; one document sweep is cheaper than hundreds of subtrees.
      pendingRoots.clear();
      pendingRoots.add(document.documentElement);
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

  const queueMutationRoot = (root) => {
    if (!(root instanceof HTMLElement)) return;
    if (blockBeforePaint(root) > 0) return;
    addPendingRoot(root);
  };

  const schedulePendingFlush = () => {
    // Deliberately non-resetting: a sustained mutation stream must not be able
    // to starve the flush indefinitely. Prefer rAF so menu controls inserted
    // during a click are blocked before the next paint instead of flashing.
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
          const containingControl = target?.closest?.(controlSelector);
          if (containingControl) queueMutationRoot(containingControl);
          continue;
        }
        const containingControl = target?.closest?.(controlSelector);
        if (containingControl) queueMutationRoot(containingControl);
        for (const node of mutation.addedNodes || []) {
          queueMutationRoot(mutationRoot(node));
        }
      }
      if (pendingRoots.size) schedulePendingFlush();
    });
    controlObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["aria-label", "role", "title"],
      childList: true,
      subtree: true,
    });
  }

  const stopPetControlEvent = (event) => {
    const control = event.target instanceof Element
      ? event.target.closest(controlSelector)
      : null;
    if (!isPetControl(control)) return;
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation?.();
  };

  const eventNames = ["pointerdown", "click", "keydown"];
  eventNames.forEach((eventName) => {
    document.addEventListener(eventName, stopPetControlEvent, true);
  });
  window.__codeyBlockNativePetControls = block;
  window.__codeyPetControlShield = Object.freeze({ enabled, block, isPetControl });
  window.__codeyPetControlShieldCleanup = () => {
    controlObserver?.disconnect();
    if (flushTimer) cancelPendingFlush?.();
    flushTimer = 0;
    cancelPendingFlush = null;
    pendingRoots.clear();
    eventNames.forEach((eventName) => {
      document.removeEventListener(eventName, stopPetControlEvent, true);
    });
    delete window.__codeyBlockNativePetControls;
    delete window.__codeyPetControlShield;
    delete window.__codeyPetControlShieldCleanup;
  };
  block();
})();
