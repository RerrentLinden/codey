// Keep Codex's native model allowlist aligned with the current Codey channel.
(() => {
  const patchVersion = "14";
  const existingPatch = window.__codeyModelWhitelistPatch;
  if (existingPatch?.version === patchVersion) {
    void existingPatch.refresh();
    return;
  }
  existingPatch?.dispose?.();

  const modelConfigId = "107580212";
  const modelCatalogPath = "/codex-model-catalog";
  const fastServiceTierId = "priority";
  const fastSpeedTierId = "fast";
  const interactionEvents = ["pointerdown", "focusin"];
  const groupedMenuStyleId = "codey-model-route-menu-style";
  const groupedMenuSelector = "[role='menu'], [role='listbox']";
  const groupedMenuItemSelector = "[role='menuitem'], [role='menuitemradio'], [role='option']";
  const modelQueryKey = ["models", "list"];
  const modelResponseEvent = "message";
  const modelRequestEvent = "codex-message-from-view";
  const modelBoundRequestMethods = new Set([
    "thread/start",
    "thread/resume",
    "turn/start",
  ]);
  let catalog = {
    loaded: false,
    models: [],
    defaultModel: "",
    modelMetadata: {},
    routeMetadata: {},
  };
  let refreshTimer = 0;
  let refreshUntil = 0;
  let refreshRetryDelay = 120;
  let refreshDeliveryInFlight = false;
  let catalogLoadPromise = null;
  let catalogRevision = 0;
  let disposed = false;
  const fullReactDiscoveryIntervalMs = 10_000;
  const maxTrackedModelListRequests = 256;
  const maxKnownModelQueryClients = 8;
  let nextFullReactDiscoveryAt = 0;
  const modelListRequestIds = new Set();
  const knownModelQueryClients = new Set();
  let originalDispatchEvent = null;
  let patchedDispatchEvent = null;
  let groupedMenuTimer = 0;
  let groupedMenuObserver = null;
  const patchedProviderKey = Symbol("codeyPatchedModelProvider");
  let deliveryState = {
    revision: 0,
    statsigClients: 0,
    notifiedClients: 0,
    queryClients: 0,
    queryEntries: 0,
    reactContainers: 0,
    responsePatchInstalled: false,
  };

  const rememberBounded = (set, value, limit) => {
    set.delete(value);
    set.add(value);
    while (set.size > limit) {
      set.delete(set.values().next().value);
    }
  };

  const modelKey = (value) => String(value || "").trim().toLowerCase();
  const uniqueModelNames = (values) => {
    const seen = new Set();
    return (Array.isArray(values) ? values : []).reduce((models, value) => {
      if (typeof value !== "string") return models;
      const model = value.trim();
      const key = modelKey(model);
      if (!key || seen.has(key)) return models;
      seen.add(key);
      models.push(model);
      return models;
    }, []);
  };
  const canonicalModelName = (models, value) => {
    const key = modelKey(value);
    return key ? models.find((model) => modelKey(model) === key) || "" : "";
  };
  const requestProviderId = (providerId) => (
    typeof providerId === "string" ? providerId.trim() : ""
  );
  const markPatchedProvider = (params, providerId) => {
    try {
      Object.defineProperty(params, patchedProviderKey, {
        value: providerId,
        configurable: true,
      });
    } catch {
      // Ignore non-extensible request objects; the serialized request payload is unchanged.
    }
    return params;
  };
  const cleanText = (value) => (
    typeof value === "string" ? value.trim().replace(/\s+/g, " ") : ""
  );
  const metadataText = (metadata, key) => cleanText(
    metadata && typeof metadata === "object" ? metadata[key] : "",
  );
  const displayNameParts = (displayName) => {
    const separator = displayName.indexOf(" / ");
    return separator > 0
      ? {
        routeName: cleanText(displayName.slice(0, separator)),
        modelName: cleanText(displayName.slice(separator + 3)),
      }
      : { routeName: "", modelName: cleanText(displayName) };
  };
  const modelFallbackName = (modelName) => {
    const separator = modelName.indexOf("/");
    return cleanText(separator > 0 ? modelName.slice(separator + 1) : modelName);
  };

  const sameModelNames = (left, right) => (
    Array.isArray(left)
    && left.length === right.length
    && left.every((value, index) => value === right[index])
  );

  const normalizedCatalog = (value) => {
    if (
      !value
      || typeof value !== "object"
      || !["ok", "not_configured"].includes(value.status)
    ) {
      return null;
    }
    const models = uniqueModelNames(value.models);
    const requestedDefault = [value.default_model, value.model]
      .map((model) => canonicalModelName(models, model))
      .find(Boolean);
    const modelMetadata = Object.fromEntries(
      (Array.isArray(value.model_metadata) ? value.model_metadata : [])
        .flatMap((metadata) => {
          if (
            !metadata
            || typeof metadata !== "object"
            || typeof metadata.model !== "string"
          ) return [];
          const model = canonicalModelName(models, metadata.model);
          return model ? [[model, metadata]] : [];
        }),
    );
    const routeMetadata = Object.fromEntries(
      Object.entries(modelMetadata).map(([model, metadata]) => {
        const providerId = typeof metadata.provider_id === "string"
          ? metadata.provider_id.trim()
          : "";
        const sourceModel = typeof metadata.source_model === "string"
          ? metadata.source_model.trim()
          : "";
        const routeName = metadataText(metadata, "route_name")
          || displayNameParts(metadataText(metadata, "display_name")).routeName
          || providerId;
        return [model, { providerId, sourceModel, routeName }];
      }).filter(([, route]) => route.providerId && route.sourceModel),
    );
    for (const model of models) {
      if (routeMetadata[model]) continue;
      const separator = model.indexOf("/");
      if (separator <= 0) continue;
      const providerId = model.slice(0, separator).trim();
      const sourceModel = model.slice(separator + 1).replace(/#\d+$/, "").trim();
      if (providerId && sourceModel) {
        routeMetadata[model] = { providerId, sourceModel, routeName: providerId };
      }
    }
    return {
      loaded: true,
      models,
      defaultModel: requestedDefault?.trim() || models[0] || "",
      modelMetadata,
      routeMetadata,
    };
  };

  const reasoningEffortName = (value) => (
    typeof value === "string"
      ? value.trim()
      : typeof value?.reasoningEffort === "string"
        ? value.reasoningEffort.trim()
        : ""
  );

  const reasoningEffortDescriptors = (values) => uniqueModelNames(
    (Array.isArray(values) ? values : []).map(reasoningEffortName),
  ).map((reasoningEffort) => ({
    reasoningEffort,
    description: `${reasoningEffort} effort`,
  }));

  const fallbackReasoningEfforts = () => reasoningEffortDescriptors([
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
  ]);

  const fastServiceTier = () => ({
    id: fastServiceTierId,
    name: "Fast",
    description: "1.5x speed, increased usage",
  });

  const nativeFastServiceTiers = (value) => {
    const tiers = Array.isArray(value) ? value : [];
    const fastTierIndex = tiers.findIndex((tier) => tier?.id === fastServiceTierId);
    if (fastTierIndex < 0) return [...tiers, fastServiceTier()];
    const currentFastTier = tiers[fastTierIndex];
    if (
      !currentFastTier
      || typeof currentFastTier !== "object"
      || !Object.hasOwn(currentFastTier, "iconKind")
    ) return tiers;
    const nativeFastTier = { ...currentFastTier };
    delete nativeFastTier.iconKind;
    const nextTiers = [...tiers];
    nextTiers[fastTierIndex] = nativeFastTier;
    return nextTiers;
  };

  const nativeFastSpeedTiers = (value) => {
    const tiers = Array.isArray(value) ? value : [];
    return tiers.includes(fastSpeedTierId) ? tiers : [...tiers, fastSpeedTierId];
  };

  const modelPresentation = (modelName, current = null) => {
    const metadata = catalog.modelMetadata[modelName];
    const route = catalog.routeMetadata[modelName];
    const metadataDisplayName = metadataText(metadata, "display_name");
    const displayParts = displayNameParts(metadataDisplayName);
    const routeName = metadataText(metadata, "route_name")
      || route?.routeName
      || displayParts.routeName
      || cleanText(route?.providerId)
      || "";
    const sourceModel = metadataText(metadata, "source_model") || route?.sourceModel || "";
    const modelLabel = metadataText(metadata, "model_display_name")
      || sourceModel
      || displayParts.modelName
      || modelFallbackName(modelName);
    const currentDisplayName = cleanText(current?.displayName);
    const displayName = metadataDisplayName
      || (routeName && modelLabel ? `${routeName} / ${modelLabel}` : "")
      || currentDisplayName
      || modelName;
    return {
      routeName,
      modelName: modelLabel,
      displayName,
      providerId: cleanText(route?.providerId) || metadataText(metadata, "provider_id"),
      sourceModel: sourceModel || modelName,
    };
  };

  const modelDescriptor = (modelName, current = null) => {
    const metadata = catalog.modelMetadata[modelName];
    const presentation = modelPresentation(modelName, current);
    const displayName = presentation.displayName;
    const supportedReasoningEfforts = reasoningEffortDescriptors(
      metadata?.supported_reasoning_efforts,
    );
    const currentReasoningEfforts = reasoningEffortDescriptors(
      current?.supportedReasoningEfforts,
    );
    const resolvedReasoningEfforts = supportedReasoningEfforts.length > 0
      ? supportedReasoningEfforts
      : currentReasoningEfforts.length > 0
        ? currentReasoningEfforts
        : fallbackReasoningEfforts();
    const supportedNames = resolvedReasoningEfforts.map(reasoningEffortName);
    const requestedDefault = [
      metadata?.default_reasoning_effort,
      current?.defaultReasoningEffort,
      "medium",
      "low",
      supportedNames[0],
    ].find((effort) => (
      typeof effort === "string" && supportedNames.includes(effort.trim())
    ));
    return {
      ...(current && typeof current === "object" ? current : {}),
      model: modelName,
      id: typeof current?.id === "string" && current.id ? current.id : modelName,
      slug: typeof current?.slug === "string" && current.slug ? current.slug : modelName,
      name: displayName
        || (typeof current?.name === "string" && current.name ? current.name : modelName),
      displayName,
      routeName: presentation.routeName,
      providerName: presentation.routeName,
      providerId: presentation.providerId,
      sourceModel: presentation.sourceModel,
      codeyRouteName: presentation.routeName,
      codeyModelName: presentation.modelName,
      description: typeof current?.description === "string" && current.description
        ? current.description
        : presentation.routeName || "Custom model",
      hidden: false,
      isDefault: modelName === catalog.defaultModel,
      defaultReasoningEffort: requestedDefault?.trim() || "medium",
      supportedReasoningEfforts: resolvedReasoningEfforts,
      serviceTiers: nativeFastServiceTiers(current?.serviceTiers),
      additionalSpeedTiers: nativeFastSpeedTiers(current?.additionalSpeedTiers),
      defaultServiceTier: Object.hasOwn(current || {}, "defaultServiceTier")
        ? current.defaultServiceTier
        : null,
    };
  };

  const sameReasoningEffortNames = (left, right) => (
    Array.isArray(left)
    && left.length === right.length
    && left.every((value, index) => (
      reasoningEffortName(value) === reasoningEffortName(right[index])
    ))
  );

  const sameReasoningEfforts = (left, right) => (
    Array.isArray(left)
    && left.every((value) => (
      value
      && typeof value === "object"
      && typeof value.reasoningEffort === "string"
      && value.reasoningEffort.trim()
    ))
    && sameReasoningEffortNames(left, right)
  );

  const sameModelMetadata = (left, right, models) => models.every((modelName) => {
    const leftMetadata = left[modelName];
    const rightMetadata = right[modelName];
    if (!leftMetadata || !rightMetadata) return leftMetadata === rightMetadata;
    return (
      leftMetadata.default_reasoning_effort === rightMetadata.default_reasoning_effort
      && leftMetadata.display_name === rightMetadata.display_name
      && leftMetadata.route_name === rightMetadata.route_name
      && leftMetadata.model_display_name === rightMetadata.model_display_name
      && leftMetadata.provider_id === rightMetadata.provider_id
      && leftMetadata.source_model === rightMetadata.source_model
      && sameReasoningEffortNames(
        leftMetadata.supported_reasoning_efforts,
        rightMetadata.supported_reasoning_efforts,
      )
    );
  });

  const sameCatalog = (left, right) => (
    left.loaded
    && sameModelNames(left.models, right.models)
    && left.defaultModel === right.defaultModel
    && sameModelMetadata(left.modelMetadata, right.modelMetadata, right.models)
  );

  const modelArrayLooksPatchable = (value, allowEmpty = false) => (
    Array.isArray(value)
    && (allowEmpty || value.length > 0)
    && Array.from(value).every((item) => (
      item
      && typeof item === "object"
      && typeof item.model === "string"
    ))
  );

  const patchedModelArray = (models, allowEmpty = false) => {
    if (!catalog.loaded || !modelArrayLooksPatchable(models, allowEmpty)) return null;
    const existing = new Map(models.map((item) => [modelKey(item.model), item]));
    const nextModels = catalog.models.map((modelName) => (
      modelDescriptor(modelName, existing.get(modelKey(modelName)))
    ));
    const unchanged = (
      models.length === nextModels.length
      && models.every((model, index) => (
        model?.model === nextModels[index]?.model
        && model?.name === nextModels[index]?.name
        && model?.displayName === nextModels[index]?.displayName
        && model?.routeName === nextModels[index]?.routeName
        && model?.providerName === nextModels[index]?.providerName
        && model?.providerId === nextModels[index]?.providerId
        && model?.sourceModel === nextModels[index]?.sourceModel
        && model?.codeyRouteName === nextModels[index]?.codeyRouteName
        && model?.codeyModelName === nextModels[index]?.codeyModelName
        && model?.hidden === false
        && model?.isDefault === nextModels[index]?.isDefault
        && model?.defaultReasoningEffort === nextModels[index]?.defaultReasoningEffort
        && sameReasoningEfforts(
          model?.supportedReasoningEfforts,
          nextModels[index]?.supportedReasoningEfforts,
        )
        && model?.serviceTiers === nextModels[index]?.serviceTiers
        && model?.additionalSpeedTiers === nextModels[index]?.additionalSpeedTiers
        && model?.defaultServiceTier === nextModels[index]?.defaultServiceTier
      ))
    );
    return unchanged ? null : nextModels;
  };

  const patchedModelPayload = (value) => {
    if (!catalog.loaded || !value || typeof value !== "object") {
      return { changed: false, value };
    }
    if (Array.isArray(value)) {
      const models = patchedModelArray(value);
      return models
        ? { changed: true, value: models }
        : { changed: false, value };
    }

    let changed = false;
    const next = { ...value };
    for (const key of ["data", "models"]) {
      const allowEmpty = key === "data"
        ? ("nextCursor" in value || "next_cursor" in value)
        : (
          "defaultModel" in value
          || "default_model" in value
          || "hasModelSupportingMaxReasoningEffort" in value
        );
      const models = patchedModelArray(value[key], allowEmpty);
      if (!models) continue;
      next[key] = models;
      changed = true;
    }
    for (const key of ["result", "message"]) {
      if (!value[key] || typeof value[key] !== "object") continue;
      const nested = patchedModelPayload(value[key]);
      if (!nested.changed) continue;
      next[key] = nested.value;
      changed = true;
    }
    if (
      Array.isArray(value.availableModels)
      && !sameModelNames(value.availableModels, catalog.models)
    ) {
      next.availableModels = [...catalog.models];
      changed = true;
    }
    if (
      Array.isArray(value.available_models)
      && !sameModelNames(value.available_models, catalog.models)
    ) {
      next.available_models = [...catalog.models];
      changed = true;
    }
    if ("defaultModel" in value && catalog.defaultModel) {
      if (typeof value.defaultModel === "string" && value.defaultModel !== catalog.defaultModel) {
        next.defaultModel = catalog.defaultModel;
        changed = true;
      } else if (
        value.defaultModel
        && typeof value.defaultModel === "object"
        && value.defaultModel.model !== catalog.defaultModel
      ) {
        const models = next.models || value.models;
        next.defaultModel = Array.isArray(models)
          ? models.find((model) => model?.model === catalog.defaultModel)
            || modelDescriptor(catalog.defaultModel)
          : modelDescriptor(catalog.defaultModel);
        changed = true;
      }
    }
    return { changed, value: changed ? next : value };
  };

  const patchedModelConfig = (config) => {
    if (
      !catalog.loaded
      || !config
      || typeof config !== "object"
      || !config.value
      || typeof config.value !== "object"
    ) {
      return config;
    }
    const value = config.value;
    if (
      sameModelNames(value.available_models, catalog.models)
      && value.default_model === catalog.defaultModel
    ) {
      return config;
    }
    const nextConfig = {
      ...config,
      value: {
        ...value,
        available_models: [...catalog.models],
        default_model: catalog.defaultModel,
      },
    };
    try {
      config.value = nextConfig.value;
      if (config.value === nextConfig.value) return config;
    } catch {
      // Frozen Statsig results are returned as a shallow copy by the wrapper.
    }
    return nextConfig;
  };

  const addConfigReference = (references, parent, key) => {
    if (!parent || typeof parent !== "object" || !(key in parent)) return;
    references.push({ parent, key });
  };

  const statsigModelConfigReferences = (client) => {
    const references = [];
    const memoCache = client?._memoCache;
    if (memoCache && typeof memoCache === "object") {
      Object.keys(memoCache)
        .filter((key) => key.includes(modelConfigId))
        .forEach((key) => addConfigReference(references, memoCache, key));
    }
    [
      client?._store?._valuesForExternalUse?.dynamic_configs,
      client?._store?._values?._values?.dynamic_configs,
      client?._store?._values?.dynamic_configs,
    ].forEach((configs) => addConfigReference(references, configs, modelConfigId));
    return references;
  };

  const patchStatsigClient = (client) => {
    if (!client || typeof client !== "object") return false;
    let changed = false;
    const memoCache = client._memoCache;
    if (memoCache instanceof Map) {
      for (const [key, current] of memoCache.entries()) {
        if (!String(key).includes(modelConfigId)) continue;
        const alreadyPatched = (
          sameModelNames(current?.value?.available_models, catalog.models)
          && current?.value?.default_model === catalog.defaultModel
        );
        const next = patchedModelConfig(current);
        if (next !== current) {
          try {
            memoCache.set(key, next);
          } catch {
            // The getDynamicConfig wrapper still fixes immutable cache entries.
          }
        }
        if (!alreadyPatched) changed = true;
      }
    }
    for (const { parent, key } of statsigModelConfigReferences(client)) {
      const current = parent[key];
      const alreadyPatched = (
        sameModelNames(current?.value?.available_models, catalog.models)
        && current?.value?.default_model === catalog.defaultModel
      );
      const next = patchedModelConfig(current);
      if (next !== current) {
        try {
          parent[key] = next;
        } catch {
          // The getDynamicConfig wrapper still fixes immutable cache entries.
        }
      }
      if (!alreadyPatched) changed = true;
    }

    const currentGetter = client.getDynamicConfig;
    if (
      typeof currentGetter === "function"
      && currentGetter.__codeyModelWhitelistPatchVersion !== patchVersion
    ) {
      const originalGetter = currentGetter.bind(client);
      const wrappedGetter = (name, options) => {
        const result = originalGetter(name, options);
        return String(name) === modelConfigId ? patchedModelConfig(result) : result;
      };
      Object.defineProperty(wrappedGetter, "__codeyModelWhitelistPatchVersion", {
        value: patchVersion,
      });
      try {
        client.getDynamicConfig = wrappedGetter;
        changed = client.getDynamicConfig === wrappedGetter || changed;
      } catch {
        // A later refresh retries if Statsig temporarily exposes a readonly API.
      }
    }
    return changed;
  };

  const statsigClients = window.__codeySharedRuntime.statsigClients;

  const notifyStatsigClients = () => {
    let notified = 0;
    for (const client of statsigClients()) {
      if (typeof client.$emt !== "function") continue;
      try {
        client.$emt({ name: "values_updated" });
        notified += 1;
      } catch {
        // A later refresh retries transient Statsig subscription failures.
      }
    }
    return notified;
  };

  const applyModelWhitelist = () => {
    if (!catalog.loaded || disposed) return false;
    let changed = false;
    statsigClients().forEach((client) => {
      if (patchStatsigClient(client)) changed = true;
    });
    return changed;
  };

  const ensureGroupedMenuStyles = () => {
    if (
      document.getElementById?.(groupedMenuStyleId)
      || typeof document.createElement !== "function"
    ) return;
    const style = document.createElement("style");
    if (!style) return;
    style.id = groupedMenuStyleId;
    style.textContent = `
      .codey-model-route-heading {
        display: flex;
        align-items: center;
        gap: 8px;
        margin: 8px 8px 4px;
        padding: 0 2px;
        color: #8b949e;
        font-size: 11px;
        font-weight: 600;
        line-height: 16px;
        pointer-events: none;
        user-select: none;
      }
      .codey-model-route-heading::after {
        content: "";
        flex: 1 1 auto;
        border-top: 1px solid rgba(139, 148, 158, 0.24);
      }
    `;
    (document.head || document.documentElement || document.body)?.appendChild?.(style);
  };

  const replaceTextOnce = (element, from, to) => {
    const source = cleanText(from);
    const target = cleanText(to);
    if (!source || !target || source === target) return false;
    if (typeof document.createTreeWalker === "function") {
      const walker = document.createTreeWalker(element, 4);
      let node = walker.nextNode();
      while (node) {
        const text = node.nodeValue || "";
        if (text.includes(source)) {
          node.nodeValue = text.replace(source, target);
          return true;
        }
        node = walker.nextNode();
      }
    }
    if (cleanText(element.textContent) === source) {
      element.textContent = target;
      return true;
    }
    return false;
  };

  const createRouteHeading = (routeName) => {
    if (typeof document.createElement !== "function") return null;
    const heading = document.createElement("div");
    heading.className = "codey-model-route-heading";
    heading.textContent = routeName;
    heading.setAttribute("role", "presentation");
    heading.setAttribute("aria-hidden", "true");
    heading.dataset.codeyRouteHeading = routeName;
    return heading;
  };

  const directRouteHeadings = (parent) => Array.from(parent?.children || [])
    .filter((child) => Boolean(child?.dataset?.codeyRouteHeading));

  const reconcileRouteHeadings = (parent, items) => {
    const routeStarts = [];
    let previousRoute = "";
    for (const { item, routeName } of items) {
      if (routeName === previousRoute) continue;
      routeStarts.push({ item, routeName });
      previousRoute = routeName;
    }
    const headings = directRouteHeadings(parent);
    const children = Array.from(parent?.children || []);
    const alreadyGrouped = headings.length === routeStarts.length
      && routeStarts.every(({ item, routeName }) => {
        const itemIndex = children.indexOf(item);
        return itemIndex > 0
          && children[itemIndex - 1]?.dataset?.codeyRouteHeading === routeName;
      });
    if (alreadyGrouped) return false;
    headings.forEach((heading) => heading.remove?.());
    for (const { item, routeName } of routeStarts) {
      const heading = createRouteHeading(routeName);
      if (heading) parent.insertBefore?.(heading, item);
    }
    return true;
  };

  const enhanceGroupedModelMenus = () => {
    if (!catalog.loaded || disposed || typeof document.querySelectorAll !== "function") return;
    ensureGroupedMenuStyles();
    const byDisplayName = new Map();
    for (const modelName of catalog.models) {
      const presentation = modelPresentation(modelName);
      const displayName = cleanText(presentation.displayName);
      if (!displayName || !presentation.routeName || !presentation.modelName) continue;
      byDisplayName.set(displayName, { modelName, presentation });
    }
    if (byDisplayName.size === 0) return;
    const containers = Array.from(document.querySelectorAll(groupedMenuSelector) || []);
    for (const container of containers) {
      const items = Array.from(container.querySelectorAll?.(groupedMenuItemSelector) || []);
      const enhancedItems = [];
      for (const item of items) {
        const itemText = cleanText(item.textContent);
        const existingModel = item.dataset?.codeyRouteModel || "";
        const existingPresentation = existingModel ? modelPresentation(existingModel) : null;
        const existingPresentationStillMatches = existingPresentation?.routeName
          && [existingPresentation.displayName, existingPresentation.modelName]
            .map(cleanText)
            .includes(itemText);
        const matched = existingPresentationStillMatches
          ? { modelName: existingModel, presentation: existingPresentation }
          : byDisplayName.get(itemText);
        if (!matched?.presentation?.routeName || !matched.presentation.modelName) continue;
        item.dataset.codeyRouteModel = matched.modelName;
        item.dataset.codeyRouteName = matched.presentation.routeName;
        item.classList?.add?.("codey-model-route-item");
        item.setAttribute?.(
          "aria-label",
          `${matched.presentation.routeName} / ${matched.presentation.modelName}`,
        );
        replaceTextOnce(
          item,
          matched.presentation.displayName,
          matched.presentation.modelName,
        );
        enhancedItems.push({ item, routeName: matched.presentation.routeName });
      }
      if (enhancedItems.length === 0) continue;
      const itemsByParent = new Map();
      for (const entry of enhancedItems) {
        const { item } = entry;
        const parent = item.parentElement || container;
        const siblings = itemsByParent.get(parent) || [];
        siblings.push(entry);
        itemsByParent.set(parent, siblings);
      }
      for (const [parent, groupedItems] of itemsByParent) {
        reconcileRouteHeadings(parent, groupedItems);
      }
    }
  };

  const scheduleGroupedModelMenuEnhancement = () => {
    if (disposed || groupedMenuTimer || !catalog.loaded) return;
    groupedMenuTimer = window.setTimeout(() => {
      groupedMenuTimer = 0;
      enhanceGroupedModelMenus();
    }, 0);
  };

  const installGroupedModelMenuObserver = () => {
    const MutationObserver = window.MutationObserver || globalThis.MutationObserver;
    if (
      groupedMenuObserver
      || typeof MutationObserver !== "function"
      || !document.body
    ) return;
    groupedMenuObserver = new MutationObserver(scheduleGroupedModelMenuEnhancement);
    groupedMenuObserver.observe(document.body, { childList: true, subtree: true });
  };

  const reactFiberKeys = (element) =>
    window.__codeySharedRuntime.reactInternalKeys(element, { includeContainer: true });

  const reactModelStateNodes = (forceScan = false) => {
    const nodes = [
      document.body,
      document.documentElement,
      document.getElementById?.("root"),
      ...Array.from(document.querySelectorAll?.(
        "[role='menu'], [role='dialog'], [role='listbox'], [data-radix-popper-content-wrapper]",
      ) || []),
    ].filter(Boolean);
    if (forceScan) {
      nodes.push(...Array.from(document.querySelectorAll?.("*") || []).slice(0, 600));
    }
    return nodes.filter((node, index, all) => all.indexOf(node) === index);
  };

  const scanReactObjectGraph = (forceScan = false) => {
    if (!forceScan && knownModelQueryClients.size > 0) {
      return {
        queryClients: [...knownModelQueryClients],
        reactContainers: 0,
      };
    }
    const queryClients = new Set(knownModelQueryClients);
    const visited = new WeakSet();
    let visitedCount = 0;
    let reactContainers = 0;

    const visit = (value, depth = 0) => {
      if (
        !value
        || (typeof value !== "object" && typeof value !== "function")
        || visited.has(value)
        || visitedCount >= 30_000
        || depth > 8
      ) return;
      visited.add(value);
      visitedCount += 1;

      try {
        if (
          typeof value.getQueriesData === "function"
          && typeof value.setQueryData === "function"
          && typeof value.invalidateQueries === "function"
        ) {
          queryClients.add(value);
          rememberBounded(
            knownModelQueryClients,
            value,
            maxKnownModelQueryClients,
          );
        }
      } catch {
        // Ignore proxy-backed values that reject capability probes.
      }

      const patched = patchedModelPayload(value);
      if (patched.changed && patched.value !== value) {
        for (const key of ["data", "models", "result", "message", "availableModels", "available_models", "defaultModel"]) {
          if (!(key in patched.value) || patched.value[key] === value[key]) continue;
          try {
            value[key] = patched.value[key];
            reactContainers += 1;
          } catch {
            // QueryClient.setQueryData handles immutable cached results below.
          }
        }
      }

      let keys = [];
      try {
        keys = Object.keys(value).slice(0, 120);
      } catch {
        return;
      }
      for (const key of keys) {
        if (
          key === "ownerDocument"
          || key === "parentElement"
          || key === "parentNode"
          || key === "children"
          || key === "childNodes"
        ) continue;
        let child;
        try {
          child = value[key];
        } catch {
          continue;
        }
        visit(child, depth + 1);
      }
    };

    for (const node of reactModelStateNodes(forceScan)) {
      for (const key of reactFiberKeys(node)) {
        let root;
        try {
          root = node[key];
        } catch {
          continue;
        }
        visit(root);
      }
    }
    return { queryClients: [...queryClients], reactContainers };
  };

  const scanReactObjectGraphWhenDue = (forceScan = false) => {
    let runFullScan = forceScan;
    if (!runFullScan && knownModelQueryClients.size === 0) {
      const now = Date.now();
      if (now < nextFullReactDiscoveryAt) {
        return { queryClients: [], reactContainers: 0 };
      }
      runFullScan = true;
    }
    if (runFullScan) {
      nextFullReactDiscoveryAt = Date.now() + fullReactDiscoveryIntervalMs;
    }
    return scanReactObjectGraph(runFullScan);
  };

  const patchModelQueryClients = async ({
    forceScan = false,
    invalidate = false,
  } = {}) => {
    const scan = scanReactObjectGraphWhenDue(forceScan);
    let queryEntries = 0;
    let changedEntries = 0;
    const invalidations = [];

    for (const client of scan.queryClients) {
      let entries = [];
      try {
        entries = client.getQueriesData({ queryKey: modelQueryKey }) || [];
      } catch {
        knownModelQueryClients.delete(client);
        continue;
      }
      queryEntries += entries.length;
      for (const [queryKey, current] of entries) {
        const patched = patchedModelPayload(current);
        if (!patched.changed) continue;
        try {
          client.setQueryData(queryKey, patched.value);
          changedEntries += 1;
        } catch {
          // The response interceptor still patches the next active refetch.
        }
      }
      if (invalidate) {
        try {
          invalidations.push(Promise.resolve(client.invalidateQueries({
            queryKey: modelQueryKey,
            refetchType: "active",
          })));
        } catch {
          // A later scheduled pass retries discovery and refresh.
        }
      }
    }
    if (invalidations.length > 0) {
      void Promise.allSettled(invalidations).then(async () => {
        if (disposed || !catalog.loaded) return;
        const settledPass = await patchModelQueryClients({
          forceScan: false,
          invalidate: false,
        });
        const notifiedClients = notifyStatsigClients();
        updateDeliveryState({
          statsigClients: statsigClients().length,
          notifiedClients,
          queryClients: settledPass.queryClients,
          queryEntries: settledPass.queryEntries,
          reactContainers: settledPass.reactContainers,
        });
      });
    }
    return {
      queryClients: scan.queryClients.length,
      queryEntries,
      changedEntries,
      reactContainers: scan.reactContainers,
    };
  };

  const updateDeliveryState = (report) => {
    if (deliveryState.revision !== catalogRevision) {
      deliveryState = {
        revision: catalogRevision,
        statsigClients: 0,
        notifiedClients: 0,
        queryClients: 0,
        queryEntries: 0,
        reactContainers: 0,
        responsePatchInstalled: true,
      };
    }
    deliveryState.statsigClients = Math.max(
      deliveryState.statsigClients,
      report.statsigClients || 0,
    );
    deliveryState.notifiedClients = Math.max(
      deliveryState.notifiedClients,
      report.notifiedClients || 0,
    );
    deliveryState.queryClients = Math.max(
      deliveryState.queryClients,
      report.queryClients || 0,
    );
    deliveryState.queryEntries = Math.max(
      deliveryState.queryEntries,
      report.queryEntries || 0,
    );
    deliveryState.reactContainers = Math.max(
      deliveryState.reactContainers,
      report.reactContainers || 0,
    );
  };

  const deliverModelCatalog = async ({ invalidate = true } = {}) => {
    if (!catalog.loaded || disposed) return false;
    const statsigChanged = applyModelWhitelist();
    const firstPass = await patchModelQueryClients({
      forceScan: invalidate,
      invalidate,
    });
    const shouldNotify = (
      invalidate
      || statsigChanged
      || firstPass.changedEntries > 0
      || firstPass.reactContainers > 0
    );
    const firstNotifications = shouldNotify ? notifyStatsigClients() : 0;
    const secondPass = invalidate
      ? await patchModelQueryClients({ forceScan: false, invalidate: false })
      : firstPass;
    updateDeliveryState({
      statsigClients: statsigClients().length,
      notifiedClients: firstNotifications,
      queryClients: Math.max(firstPass.queryClients, secondPass.queryClients),
      queryEntries: Math.max(firstPass.queryEntries, secondPass.queryEntries),
      reactContainers: firstPass.reactContainers + secondPass.reactContainers,
    });
    scheduleGroupedModelMenuEnhancement();
    return true;
  };

  const scheduleRefresh = (durationMs = 5000) => {
    if (disposed) return;
    refreshUntil = Math.max(refreshUntil, Date.now() + durationMs);
    if (refreshTimer) return;
    refreshRetryDelay = 120;
    const tick = () => {
      // Keep the fired handle truthy while the tick body runs: the
      // bridge-missing path inside loadModelCatalog calls scheduleRefresh
      // synchronously, and a cleared handle here would let it start a second
      // timer chain that can never be cancelled.
      if (disposed) {
        refreshTimer = 0;
        return;
      }
      if (catalog.loaded) {
        if (!refreshDeliveryInFlight) {
          refreshDeliveryInFlight = true;
          void deliverModelCatalog({ invalidate: false }).then(
            () => {
              refreshDeliveryInFlight = false;
            },
            (error) => {
              refreshDeliveryInFlight = false;
              console.warn("[Codey] scheduled model delivery failed", error);
            },
          );
        }
      } else {
        void loadModelCatalog();
      }
      if (Date.now() < refreshUntil) {
        const nextDelay = refreshRetryDelay;
        refreshRetryDelay = Math.min(
          refreshRetryDelay * 2,
          catalog.loaded ? 1000 : 2000,
        );
        refreshTimer = window.setTimeout(tick, nextDelay);
      } else {
        refreshTimer = 0;
      }
    };
    refreshTimer = window.setTimeout(tick, 0);
  };

  const loadModelCatalog = () => {
    if (catalogLoadPromise) return catalogLoadPromise;
    const requestedRevision = catalogRevision;
    catalogLoadPromise = (async () => {
      if (disposed || typeof window.__codexSessionDeleteBridge !== "function") {
        scheduleRefresh();
        return false;
      }
      try {
        const result = await window.__codexSessionDeleteBridge(modelCatalogPath, {});
        const nextCatalog = normalizedCatalog(result);
        if (!nextCatalog) {
          if (!catalog.loaded) scheduleRefresh();
          return false;
        }
        if (requestedRevision !== catalogRevision) return false;
        const unchanged = sameCatalog(catalog, nextCatalog);
        if (unchanged) {
          // Window-focus reloads land here when nothing changed upstream:
          // skip the invalidating re-delivery (full client scan plus query
          // invalidation) and keep only a short non-invalidating window.
          scheduleRefresh(1000);
          return true;
        }
        catalogRevision += 1;
        catalog = nextCatalog;
        await deliverModelCatalog();
        scheduleRefresh();
        return true;
      } catch (error) {
        console.warn("[Codey] model whitelist refresh failed", error);
        if (!catalog.loaded) scheduleRefresh();
        return false;
      }
    })().finally(() => {
      catalogLoadPromise = null;
    });
    return catalogLoadPromise;
  };

  const setModelCatalog = (value) => {
    if (disposed) return false;
    const nextCatalog = normalizedCatalog(value);
    if (!nextCatalog) return false;
    if (sameCatalog(catalog, nextCatalog)) {
      scheduleRefresh(1000);
      return Promise.resolve(true);
    }
    catalogRevision += 1;
    catalog = nextCatalog;
    return deliverModelCatalog().then((delivered) => {
      scheduleRefresh();
      return delivered;
    });
  };

  const routeForModel = (modelName) => {
    const route = catalog.routeMetadata[modelName];
    if (route) return route;
    const metadata = catalog.modelMetadata[modelName];
    const providerId = typeof metadata?.provider_id === "string"
      ? metadata.provider_id.trim()
      : "";
    const sourceModel = typeof metadata?.source_model === "string"
      ? metadata.source_model.trim()
      : "";
    return providerId && sourceModel ? { providerId, sourceModel } : null;
  };

  const routeForProviderAlias = (modelName) => {
    const model = typeof modelName === "string" ? modelName.trim() : "";
    const separator = model.indexOf("/");
    if (separator <= 0) return null;
    const providerId = model.slice(0, separator).trim();
    const sourceModel = model.slice(separator + 1).trim();
    if (!providerId || !sourceModel) return null;
    const catalogAlias = canonicalModelName(catalog.models, model);
    if (catalogAlias) {
      const catalogRoute = routeForModel(catalogAlias);
      if (catalogRoute) return catalogRoute;
      const catalogSeparator = catalogAlias.indexOf("/");
      if (catalogSeparator > 0) {
        return {
          providerId: catalogAlias.slice(0, catalogSeparator).trim(),
          sourceModel: catalogAlias.slice(catalogSeparator + 1).replace(/#\d+$/, "").trim(),
        };
      }
    }
    return catalog.models
      .map(routeForModel)
      .find((route) => (
        modelKey(route?.providerId) === modelKey(providerId)
        && modelKey(route?.sourceModel) === modelKey(sourceModel)
      )) || null;
  };

  const patchedRequestParams = (method, params) => {
    if (
      !catalog.loaded
      || !catalog.defaultModel
      || !modelBoundRequestMethods.has(method)
    ) {
      return params;
    }
    const source = params && typeof params === "object" ? params : {};
    const requestedModel = typeof source.model === "string"
      ? source.model.trim()
      : "";
    const requestedProvider = typeof source.model_provider === "string"
      ? source.model_provider.trim()
      : "";
    const requestedModelForProvider = (() => {
      if (!requestedProvider || !requestedModel) return requestedModel;
      const prefix = `${requestedProvider}/`;
      return requestedModel.toLowerCase().startsWith(prefix.toLowerCase())
        ? requestedModel.slice(prefix.length).trim()
        : requestedModel;
    })();
    const canonicalRequestedModel = canonicalModelName(catalog.models, requestedModel);
    const canonicalRoute = canonicalRequestedModel
      ? routeForModel(canonicalRequestedModel)
      : null;
    const shouldPreferOfficialRawModel = (
      canonicalRoute?.providerId === "openai"
      && requestedProvider
      && requestedProvider !== "openai"
      && source[patchedProviderKey] !== requestedProvider
    );
    const existingRoute = requestedProvider && !shouldPreferOfficialRawModel
      ? catalog.models
        .map(routeForModel)
        .find((route) => (
          route?.providerId === requestedProvider
          && modelKey(route.sourceModel) === modelKey(requestedModelForProvider)
        ))
      : null;
    if (existingRoute) {
      const providerId = requestProviderId(existingRoute.providerId);
      if (
        requestedModel === existingRoute.sourceModel
        && (source.model_provider || "") === providerId
      ) return params;
      const next = {
        ...source,
        model: existingRoute.sourceModel,
      };
      if (providerId) next.model_provider = providerId;
      else delete next.model_provider;
      return markPatchedProvider(next, providerId);
    }
    const aliasRoute = routeForProviderAlias(requestedModel);
    if (aliasRoute) {
      const providerId = requestProviderId(aliasRoute.providerId);
      const next = {
        ...source,
        model: aliasRoute.sourceModel,
      };
      if (providerId) next.model_provider = providerId;
      else delete next.model_provider;
      return (
        requestedModel === next.model
        && (source.model_provider || "") === (next.model_provider || "")
      ) ? params : markPatchedProvider(next, providerId);
    }
    if (canonicalRequestedModel) {
      const nextModel = canonicalRoute?.sourceModel || canonicalRequestedModel;
      const next = {
        ...source,
        model: nextModel,
      };
      const providerId = requestProviderId(canonicalRoute?.providerId || "");
      if (providerId) next.model_provider = providerId;
      else delete next.model_provider;
      return (
        requestedModel === next.model
        && (source.model_provider || "") === (next.model_provider || "")
      ) ? params : markPatchedProvider(next, providerId);
    }
    const route = routeForModel(catalog.defaultModel);
    const next = {
      ...source,
      model: route?.sourceModel || catalog.defaultModel,
    };
    const providerId = requestProviderId(route?.providerId || "");
    if (providerId) next.model_provider = providerId;
    else delete next.model_provider;
    return markPatchedProvider(next, providerId);
  };

  const patchOutgoingModelRequest = (detail) => {
    const request = detail?.request;
    if (
      detail?.type !== "mcp-request"
      || !request
      || typeof request !== "object"
    ) return false;
    if (request.method === "model/list" && request.id != null) {
      rememberBounded(
        modelListRequestIds,
        String(request.id),
        maxTrackedModelListRequests,
      );
    }

    const wrappedMethod = request.method === "send-cli-request-for-host"
      && typeof request.params?.method === "string"
      ? request.params.method
      : "";
    const method = wrappedMethod || String(request.method || "");
    const params = wrappedMethod ? request.params?.params : request.params;
    const nextParams = patchedRequestParams(method, params);
    if (nextParams === params) return false;
    if (wrappedMethod) {
      request.params = {
        ...(request.params || {}),
        params: nextParams,
      };
    } else {
      request.params = nextParams;
    }
    return true;
  };

  const handleModelRequest = (event) => {
    patchOutgoingModelRequest(event?.detail);
  };

  const installModelRequestDispatchPatch = () => {
    if (typeof window.dispatchEvent !== "function") return;
    if (window.dispatchEvent.__codeyModelRequestPatchVersion === patchVersion) return;
    originalDispatchEvent = window.dispatchEvent;
    patchedDispatchEvent = function codeyModelRequestDispatchEvent(event) {
      try {
        if (event?.type === modelRequestEvent) {
          patchOutgoingModelRequest(event.detail);
        }
      } catch (error) {
        console.warn("[Codey] model request repair failed", error);
      }
      return originalDispatchEvent.call(this, event);
    };
    Object.defineProperty(patchedDispatchEvent, "__codeyModelRequestPatchVersion", {
      value: patchVersion,
    });
    window.dispatchEvent = patchedDispatchEvent;
  };

  const handleModelResponse = (event) => {
    const data = event?.data;
    if (data?.type !== "mcp-response") return;
    const message = data.message || data.response;
    const requestId = message?.id == null ? "" : String(message.id);
    const isModelListResponse = (
      modelListRequestIds.has(requestId)
      || data.requestMethod === "model/list"
      || message?.requestMethod === "model/list"
    );
    if (!isModelListResponse) return;
    modelListRequestIds.delete(requestId);
    const patched = patchedModelPayload(message?.result);
    if (!patched.changed) return;
    try {
      message.result = patched.value;
    } catch {
      // Immutable bridge messages fall back to cached-query patching.
    }
    scheduleRefresh(1000);
  };

  // The wrapped getDynamicConfig already patches results on read, so the
  // interaction-driven re-apply is only a safety net for clients created
  // between events. Rescanning every Statsig memo cache on every pointerdown
  // and focusin is far more often than that safety net needs.
  let lastInteractionApply = 0;
  const interactionApplyIntervalMs = 2_000;
  const handleInteraction = () => {
    scheduleGroupedModelMenuEnhancement();
    const now = Date.now();
    if (now - lastInteractionApply < interactionApplyIntervalMs) return;
    lastInteractionApply = now;
    void deliverModelCatalog({ invalidate: false });
  };
  const handleFocus = () => {
    void loadModelCatalog();
  };
  interactionEvents.forEach((eventName) => {
    document.addEventListener(eventName, handleInteraction, true);
  });
  installGroupedModelMenuObserver();
  window.addEventListener?.("focus", handleFocus);
  installModelRequestDispatchPatch();
  if (typeof window.addEventListener === "function") {
    window.addEventListener(modelRequestEvent, handleModelRequest, true);
    window.addEventListener(modelResponseEvent, handleModelResponse, true);
    deliveryState.responsePatchInstalled = true;
  }

  const api = {
    version: patchVersion,
    apply: applyModelWhitelist,
    refresh: loadModelCatalog,
    setCatalog: setModelCatalog,
    enhanceModelMenus: enhanceGroupedModelMenus,
    delivery: () => ({ ...deliveryState }),
    snapshot: () => ({
      loaded: catalog.loaded,
      models: [...catalog.models],
      defaultModel: catalog.defaultModel,
    }),
    dispose() {
      disposed = true;
      window.clearTimeout(refreshTimer);
      refreshTimer = 0;
      window.clearTimeout(groupedMenuTimer);
      groupedMenuTimer = 0;
      groupedMenuObserver?.disconnect?.();
      groupedMenuObserver = null;
      interactionEvents.forEach((eventName) => {
        document.removeEventListener(eventName, handleInteraction, true);
      });
      window.removeEventListener?.("focus", handleFocus);
      window.removeEventListener?.(modelRequestEvent, handleModelRequest, true);
      window.removeEventListener?.(modelResponseEvent, handleModelResponse, true);
      if (patchedDispatchEvent && window.dispatchEvent === patchedDispatchEvent) {
        window.dispatchEvent = originalDispatchEvent;
      }
      originalDispatchEvent = null;
      patchedDispatchEvent = null;
      knownModelQueryClients.clear();
      modelListRequestIds.clear();
    },
  };
  window.__codeyModelWhitelistPatch = api;
  void loadModelCatalog();
})();
