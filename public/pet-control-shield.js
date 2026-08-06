(() => {
  window.__codeyPetControlShieldCleanup?.();

  const enabled = ["__CODEY_SLIM_PET__"][0] === "true";
  const petControlIds = new Set([
    "settings.appearance.pets",
    "settings.personalization.pets",
    "settings.pets",
    "settings.nav.pets",
    "settings.section.pets",
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
  const petControlIdPrefixes = [
    "settings.appearance.pets.",
    "settings.nav.pets.",
    "settings.personalization.pets.",
    "settings.section.pets.",
    "settings.pets.",
  ];
  const fallbackLabelPattern = /^(?:pet|pets|wake pet|show pet|tuck away pet|hide pet|refresh custom pets|create your own pet|open custom pets folder|宠物|唤醒宠物|显示宠物|收起宠物|隐藏宠物|刷新自定义宠物|创建自己的宠物|打开自定义宠物文件夹|寵物|喚醒寵物|顯示寵物|收起寵物|隱藏寵物|重新整理自訂寵物|建立自己的寵物|開啟自訂寵物資料夾)$/i;
  const controlSelector = "button, [role=button], [role=menuitem], [role=option], [role=tab]";
  const reactTraversalKeys = new Set(["return", "child", "sibling", "stateNode", "_owner"]);

  const isPetControlId = (value) =>
    petControlIds.has(value)
      || petControlIdPrefixes.some((prefix) => value.startsWith(prefix));

  const containsPetControlId = (value) =>
    window.__codeySharedRuntime.objectGraphIncludes(value, isPetControlId, {
      ignoredKeys: reactTraversalKeys,
      maxDepth: 7,
    });

  const isPetControl = (control) => {
    if (!(control instanceof HTMLElement)) return false;
    const descriptor = window.__codeyMutationDispatcher.controlDescriptor(control);
    if (fallbackLabelPattern.test(descriptor)) return true;

    // Deliberately not memoised. React reuses host elements and swaps both
    // __reactProps$ and __reactFiber$ independently, and the walk below reads
    // every matching key, so no cheap identity token covers the whole verdict.
    // At ~3 µs per control the walk is not worth a fail-open cache on a shield
    // that has to fail closed; the observer throttling above is what keeps this
    // off the streaming hot path.
    return window.__codeySharedRuntime.reactInternals(control)
      .some((internal) => {
        try {
          return containsPetControlId(internal?.memoizedProps ?? internal);
        } catch {
          return false;
        }
      });
  };

  const controlsWithin = window.__codeyMutationDispatcher.controlsWithin;

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

  const shieldLifecycle = window.__codeyMutationDispatcher?.createShieldLifecycle({
    attributeFilter: ["aria-label", "role", "title"],
    block,
    eventSelector: controlSelector,
    isControl: isPetControl,
  });
  window.__codeyBlockNativePetControls = block;
  window.__codeyPetControlShield = Object.freeze({ enabled, block, isPetControl });
  window.__codeyPetControlShieldCleanup = () => {
    shieldLifecycle?.cleanup();
    delete window.__codeyBlockNativePetControls;
    delete window.__codeyPetControlShield;
    delete window.__codeyPetControlShieldCleanup;
  };
  block();
})();
