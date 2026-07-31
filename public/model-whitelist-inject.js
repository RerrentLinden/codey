// Keep Codex's native model allowlist aligned with the current Codey channel.
(() => {
  const patchVersion = "3";
  const existingPatch = window.__codeyModelWhitelistPatch;
  if (existingPatch?.version === patchVersion) {
    void existingPatch.refresh();
    return;
  }
  existingPatch?.dispose?.();

  const modelConfigId = "107580212";
  const modelCatalogPath = "/codex-model-catalog";
  const interactionEvents = ["pointerdown", "focusin"];
  const modelQueryKey = ["models", "list"];
  const modelResponseEvent = "message";
  const modelRequestEvent = "codex-message-from-view";
  let catalog = {
    loaded: false,
    models: [],
    defaultModel: "",
  };
  let refreshTimer = 0;
  let refreshUntil = 0;
  let catalogLoadPromise = null;
  let catalogRevision = 0;
  let disposed = false;
  const modelListRequestIds = new Set();
  const knownModelQueryClients = new Set();
  let deliveryState = {
    revision: 0,
    statsigClients: 0,
    notifiedClients: 0,
    queryClients: 0,
    queryEntries: 0,
    reactContainers: 0,
    responsePatchInstalled: false,
  };

  const uniqueModelNames = (values) => Array.from(new Set(
    (Array.isArray(values) ? values : [])
      .filter((value) => typeof value === "string")
      .map((value) => value.trim())
      .filter(Boolean),
  ));

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
      .find((model) => typeof model === "string" && models.includes(model.trim()));
    return {
      loaded: true,
      models,
      defaultModel: requestedDefault?.trim() || models[0] || "",
    };
  };

  const modelReasoningEfforts = () => [
    "minimal",
    "low",
    "medium",
    "high",
    "xhigh",
  ].map((reasoningEffort) => ({
    reasoningEffort,
    description: `${reasoningEffort} effort`,
  }));

  const modelDescriptor = (modelName, current = null) => ({
    ...(current && typeof current === "object" ? current : {}),
    model: modelName,
    id: typeof current?.id === "string" && current.id ? current.id : modelName,
    slug: typeof current?.slug === "string" && current.slug ? current.slug : modelName,
    name: typeof current?.name === "string" && current.name ? current.name : modelName,
    displayName: typeof current?.displayName === "string" && current.displayName
      ? current.displayName
      : modelName,
    description: typeof current?.description === "string" && current.description
      ? current.description
      : "Custom model",
    hidden: false,
    isDefault: modelName === catalog.defaultModel,
    defaultReasoningEffort: typeof current?.defaultReasoningEffort === "string"
      ? current.defaultReasoningEffort
      : "medium",
    supportedReasoningEfforts: Array.isArray(current?.supportedReasoningEfforts)
      && current.supportedReasoningEfforts.length > 0
      ? current.supportedReasoningEfforts
      : modelReasoningEfforts(),
    serviceTiers: Array.isArray(current?.serviceTiers) ? current.serviceTiers : [],
    additionalSpeedTiers: Array.isArray(current?.additionalSpeedTiers)
      ? current.additionalSpeedTiers
      : [],
  });

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
    const existing = new Map(models.map((item) => [item.model, item]));
    const nextModels = catalog.models.map((modelName) => (
      modelDescriptor(modelName, existing.get(modelName))
    ));
    const unchanged = (
      models.length === nextModels.length
      && models.every((model, index) => (
        model?.model === nextModels[index]?.model
        && model?.hidden === false
        && model?.isDefault === nextModels[index]?.isDefault
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

  const statsigClients = () => {
    const root = window.__STATSIG__ || globalThis.__STATSIG__;
    if (!root || typeof root !== "object") return [];
    let currentInstance = null;
    try {
      currentInstance = typeof root.instance === "function" ? root.instance() : null;
    } catch {
      currentInstance = null;
    }
    return [
      root.firstInstance,
      currentInstance,
      ...(root.instances && typeof root.instances === "object"
        ? Object.values(root.instances)
        : []),
    ].filter((client, index, clients) => (
      client
      && typeof client === "object"
      && clients.indexOf(client) === index
    ));
  };

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

  const reactFiberKeys = (element) => {
    if (!element || (typeof element !== "object" && typeof element !== "function")) return [];
    try {
      return Object.keys(element).filter((key) => (
        key.startsWith("__reactFiber")
        || key.startsWith("__reactInternalInstance")
        || key.startsWith("__reactProps")
        || key.startsWith("__reactContainer")
      ));
    } catch {
      return [];
    }
  };

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
          knownModelQueryClients.add(value);
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

  const patchModelQueryClients = async ({
    forceScan = false,
    invalidate = false,
  } = {}) => {
    const scan = scanReactObjectGraph(forceScan);
    let queryEntries = 0;
    let changedEntries = 0;
    const invalidations = [];

    for (const client of scan.queryClients) {
      let entries = [];
      try {
        entries = client.getQueriesData({ queryKey: modelQueryKey }) || [];
      } catch {
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
      forceScan: invalidate || knownModelQueryClients.size === 0,
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
    return true;
  };

  const scheduleRefresh = (durationMs = 5000) => {
    if (disposed) return;
    refreshUntil = Math.max(refreshUntil, Date.now() + durationMs);
    if (refreshTimer) return;
    const tick = () => {
      refreshTimer = 0;
      if (catalog.loaded) {
        void deliverModelCatalog({ invalidate: false });
      } else {
        void loadModelCatalog();
      }
      if (!disposed && Date.now() < refreshUntil) {
        refreshTimer = window.setTimeout(tick, 120);
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
    catalogRevision += 1;
    catalog = nextCatalog;
    return deliverModelCatalog().then((delivered) => {
      scheduleRefresh();
      return delivered;
    });
  };

  const handleModelRequest = (event) => {
    const detail = event?.detail;
    const request = detail?.request;
    if (
      detail?.type !== "mcp-request"
      || request?.method !== "model/list"
      || request?.id == null
    ) return;
    modelListRequestIds.add(String(request.id));
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
  window.addEventListener?.("focus", handleFocus);
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
      interactionEvents.forEach((eventName) => {
        document.removeEventListener(eventName, handleInteraction, true);
      });
      window.removeEventListener?.("focus", handleFocus);
      window.removeEventListener?.(modelRequestEvent, handleModelRequest, true);
      window.removeEventListener?.(modelResponseEvent, handleModelResponse, true);
      knownModelQueryClients.clear();
      modelListRequestIds.clear();
    },
  };
  window.__codeyModelWhitelistPatch = api;
  void loadModelCatalog();
})();
