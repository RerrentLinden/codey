(() => {
  const disablePet = __DISABLE_PET__;
  const fastCodexStartup = __FAST_CODEX_STARTUP__;
  const codeyErrorLoggerExecutable = "__CODEY_ERROR_LOGGER_EXECUTABLE__";
  const recordCodeyPatchFailure = (operation, error, context = {}) => {
    const unresolvedExecutable =
      ["__CODEY", "ERROR_LOGGER_EXECUTABLE__"].join("_");
    if (
      !codeyErrorLoggerExecutable ||
      codeyErrorLoggerExecutable === unresolvedExecutable
    ) return;
    const message = error instanceof Error
      ? `${error.name}: ${error.message}${error.stack ? `\n${error.stack}` : ""}`
      : String(error || "unknown patch failure");
    try {
      const now = new Date();
      const platform =
        process.platform === "win32"
          ? "windows"
          : process.platform === "darwin"
            ? "macos"
            : process.platform;
      const optionalPatch =
        operation.startsWith("renderer_patch:") ||
        operation.startsWith("optional_main_bundle_patch:");
      const stage = operation.startsWith("renderer_patch:")
        ? "startup.renderer_asset_patch"
        : operation.startsWith("optional_main_bundle_patch:")
          ? "startup.optional_main_bundle_patch"
          : "startup.main_process_patch";
      const result = process.getBuiltinModule("child_process").spawnSync(
        codeyErrorLoggerExecutable,
        ["--codey-record-error"],
        {
          input: JSON.stringify({
          timestamp: now.toISOString(),
          timestampMs: now.getTime(),
          pid: process.pid,
          platform,
          event: "patch_failed",
          operation,
          error: message,
          stage,
          recoverable: optionalPatch,
          context: {
            ...context,
            electronVersion: process.versions?.electron || "",
            chromeVersion: process.versions?.chrome || "",
            nodeVersion: process.versions?.node || "",
          },
          }),
          encoding: "utf8",
          maxBuffer: 64 * 1024,
          timeout: 2000,
          windowsHide: true,
        },
      );
      if (result.error) throw result.error;
      if (result.status !== 0) {
        throw new Error(
          `Codey error log helper exited with ${result.status}: ${String(result.stderr || "").trim()}`,
        );
      }
    } catch (logError) {
      try {
        console.error("[Codey] failed to write patch error log", logError);
      } catch {}
    }
  };
  const statsigBootstrapTimeoutMs = 1500;
  const threadOwnerDiscoveryTimeoutMs = 150;
  const threadOwnerCacheTtlMs = 5000;
  const threadOwnerCacheMaxEntries = 64;
  const statsigStartupRemainingMs =
    `Math.max(0,(globalThis.__CODEY_STATSIG_STARTUP_DEADLINE_MS__??=Date.now()+${statsigBootstrapTimeoutMs})-Date.now())`;
  const disableWindowsOptimizations = process.platform === "win32";
  const disableMicro = disableWindowsOptimizations;
  const disableWindowsWmiSampler = disableWindowsOptimizations;
  const Module = process.getBuiltinModule("module");
  const originalLoad = Module._load;
  const isInspectorArgument = (argument) =>
    typeof argument === "string" && /^--inspect(?:-brk)?(?:=|$)/.test(argument);
  // Each renderer gate is optional and independent. Codex bundles are minified
  // and reshape between releases, so a single drifted anchor must skip only its
  // own gate — never discard the sibling gates that are still compatible. That
  // is what previously hid the whole Fast/service-tier control on the builds
  // where one unrelated anchor moved: an exception here aborted every gate on
  // the asset. Log and return the source unchanged so the rest still apply.
  const replaceUniqueRendererGate = (source, pattern, replacement, name) => {
    const gates = Array.isArray(pattern) ? pattern : [{ pattern, replacement }];
    let matchCount = 0;
    let patched = source;
    for (const gate of gates) {
      let gateCount = 0;
      const candidate = source.replace(gate.pattern, (...args) => {
        gateCount += 1;
        return typeof gate.replacement === "function"
          ? gate.replacement(...args)
          : gate.replacement;
      });
      if (gateCount > 0 && matchCount === 0) patched = candidate;
      matchCount += gateCount;
    }
    if (matchCount !== 1) {
      const message =
        `Codey skipped an incompatible Codex renderer patch: ${name} gate matched ${matchCount} times`;
      recordCodeyPatchFailure(`renderer_patch:${name}`, message, { matchCount });
      try {
        console.error(message);
      } catch {}
      return source;
    }
    return patched;
  };
  const replacePetRendererImportWithStubs = (match, importClause) => {
    if (typeof importClause !== "string" || importClause.trim() === "") {
      return "";
    }
    const localBindings = [];
    const rememberBinding = (binding) => {
      if (
        /^[$A-Z_a-z][$\w]*$/.test(binding)
        && !localBindings.includes(binding)
      ) {
        localBindings.push(binding);
      }
    };
    const defaultBinding = importClause.match(/^\s*([$A-Z_a-z][$\w]*)/);
    if (defaultBinding) rememberBinding(defaultBinding[1]);
    for (const specifier of importClause.matchAll(
      /(?:^|[,{])\s*([$A-Z_a-z][$\w]*)(?:\s+as\s+([$A-Z_a-z][$\w]*))?\s*(?=[,}])/g,
    )) {
      rememberBinding(specifier[2] ?? specifier[1]);
    }
    for (const namespace of importClause.matchAll(
      /\*\s+as\s+([$A-Z_a-z][$\w]*)/g,
    )) {
      rememberBinding(namespace[1]);
    }
    if (!localBindings.length) {
      const message =
        "Codey could not identify Codex pet settings renderer import bindings";
      recordCodeyPatchFailure("renderer_patch:pet settings avatar resources", message);
      try {
        console.error(message);
      } catch {}
      return match;
    }
    const [firstBinding, ...aliases] = localBindings;
    const aliasDeclarations = aliases
      .map((binding) => `,${binding}=${firstBinding}`)
      .join("");
    return `const ${firstBinding}=(()=>{const target=function(){return null};return new Proxy(target,{get(target,property,receiver){if(property===Symbol.iterator)return function*(){};if(property===\`map\`||property===\`filter\`||property===\`flatMap\`||property===\`slice\`)return()=>[];if(property===\`then\`)return void 0;return Reflect.get(target,property,receiver)},construct(){return{}}})})()${aliasDeclarations};`;
  };
  const threadOwnerDiscoveryExpression = (
    coordinationName,
    hostIdName,
    conversationIdName,
  ) =>
    [
      "await (globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V1__??=(()=>{",
      "const states=new WeakMap;",
      "const remember=(state,key,owner,now)=>{",
      `if(state.cache.size>=${threadOwnerCacheMaxEntries})state.cache.delete(state.cache.keys().next().value);`,
      `state.cache.set(key,{owner,expiresAt:now+${threadOwnerCacheTtlMs}})`,
      "};",
      "return{find(client,hostId,conversationId){",
      "let state=states.get(client);",
      "if(state==null){state={cache:new Map,inflight:new Map};states.set(client,state)}",
      "const key=String(hostId)+String.fromCharCode(0)+String(conversationId),now=Date.now(),cached=state.cache.get(key);",
      "if(cached!=null){if(cached.expiresAt>now)return Promise.resolve(cached.owner);state.cache.delete(key)}",
      "const existing=state.inflight.get(key);",
      "if(existing!=null)return existing;",
      "let settled=false,timer;",
      "const lookup=Promise.resolve().then(()=>client.findThreadOwner({hostId,conversationId}));",
      "const request=new Promise((resolve,reject)=>{",
      `timer=globalThis.setTimeout(()=>{if(settled)return;settled=true;resolve(null)},${threadOwnerDiscoveryTimeoutMs});`,
      "lookup.then(owner=>{",
      "if(settled)return;",
      "settled=true;globalThis.clearTimeout(timer);",
      "if(owner!=null)remember(state,key,owner,Date.now());",
      "resolve(owner)",
      "},error=>{",
      "if(settled)return;",
      "settled=true;globalThis.clearTimeout(timer);reject(error)",
      "})",
      "}).finally(()=>{if(state.inflight.get(key)===request)state.inflight.delete(key)});",
      "state.inflight.set(key,request);",
      "return request",
      "}}",
      "})()).find(",
      `${coordinationName}.clientCoordination,${hostIdName},${conversationIdName})`,
    ].join("");
  const patchCodexRendererAsset = (source) => {
    let patched = source;
    if (
      disablePet
      && /settings\.(?:(?:appearance|personalization)\.)?pets(?:[."`]|$)/.test(source)
      && /import(?:\s*[^;"']+?\s*from)?\s*["']\.\/codex-avatar(?:[~-][^/"']*)?\.js["']/.test(source)
    ) {
      // Recent Codex builds keep the Pets settings preview in a regular
      // settings chunk and statically import codex-avatar from it. Hiding the
      // controls after React mounts is too late: that import has already pulled
      // the avatar renderer and every bundled spritesheet into the main window.
      // Replace only that settings-side dependency with inert callable/iterable
      // bindings. The shared avatar overlay host stays intact because current
      // Codex builds also use it for voice controls.
      patched = replaceUniqueRendererGate(
        patched,
        /import(?:\s*([^;"']+?)\s*from)?\s*["']\.\/codex-avatar(?:[~-][^/"']*)?\.js["'];?/g,
        replacePetRendererImportWithStubs,
        "pet settings avatar resources",
      );
    }
    if (
      source.includes("72216192") &&
      source.includes("enable_i18n") &&
      source.includes("locale_source") &&
      source.includes(".localeOverride")
    ) {
      // Resolve the locale before React's first i18n render. The later CDP
      // injection still persists localeOverride, but it can arrive after the
      // first route has already selected and cached English messages.
      patched = replaceUniqueRendererGate(
        patched,
        /let\s+([$A-Z_a-z][$\w]*)\s*=\s*([$A-Z_a-z][$\w]*)\s*,\s*([$A-Z_a-z][$\w]*)\s*=\s*([$A-Z_a-z][$\w]*)\?\.\s*get\(\s*`locale_source`\s*,\s*`IDE`\s*\)\s*,\s*([$A-Z_a-z][$\w]*)\s*=\s*([$A-Z_a-z][$\w]*)\(\s*([$A-Z_a-z][$\w]*)\.localeOverride\s*\)/g,
        (
          _match,
          i18nEnabledName,
          _i18nGateValueName,
          localeSourceName,
          _dynamicConfigName,
          localeOverrideName,
        ) =>
          `let ${i18nEnabledName}=(globalThis.__CODEY_DEFAULT_CHINESE_LOCALE_RENDERER_PATCH__=!0),${localeSourceName}=\`SYSTEM\`,${localeOverrideName}=\`zh-CN\``,
        "default Chinese locale",
      );
    }
    if (
      source.includes("maybe_resume_owner_discovery_failed")
      && source.includes("followExistingOwner")
      && source.includes(".clientCoordination.findThreadOwner")
    ) {
      // Owner discovery is an optimization for reusing a stream already owned
      // by another window. Cache only positive, recent answers within the same
      // coordination client and merge duplicate in-flight lookups. Cache hits
      // have no timer or IPC wait; misses retain a short safety window before
      // local hydration. Null, rejected and timed-out lookups are never cached.
      patched = replaceUniqueRendererGate(
        patched,
        /await\s+([$A-Z_a-z][$\w]*)\.clientCoordination\.findThreadOwner\(\{\s*hostId\s*:\s*([$A-Z_a-z][$\w]*)\s*,\s*conversationId\s*:\s*([$A-Z_a-z][$\w]*)\s*\}\)/g,
        (_match, coordinationName, hostIdName, conversationIdName) =>
          threadOwnerDiscoveryExpression(
            coordinationName,
            hostIdName,
            conversationIdName,
          ),
        "thread owner discovery cache",
      );
    }
    if (
      source.includes("useHiddenModels:") &&
      source.includes("availableModels:") &&
      source.includes("includeUltraReasoningEffort") &&
      source.includes("amazonBedrock")
    ) {
      patched = replaceUniqueRendererGate(
        patched,
        /if\s*\(\s*\(*\s*(?:[$A-Z_a-z][$\w]*\s*(?:\?\.|\.)\s*has\(\s*[$A-Z_a-z][$\w]*\.model\s*\)\s*(?:===\s*!0)?\s*\|\|\s*)?\(?\s*([$A-Z_a-z][$\w]*)\s*\?\s*([$A-Z_a-z][$\w]*)\.has\(\s*([$A-Z_a-z][$\w]*)\.model\s*\)\s*:\s*(?:!\s*\3\.hidden|\3\.hidden\s*!==\s*!0|\3\.hidden\s*===\s*!1)\s*\)?\s*\)*\s*\)/g,
        (_match, useAllowlistName, allowlistName, modelName) =>
          `if(${useAllowlistName}?(${allowlistName}.has(${modelName}.model)||!${modelName}.hidden):!${modelName}.hidden)`,
        "model allowlist",
      );
    }
    if (
      source.includes("useHiddenModels:") &&
      source.includes("includeUltraReasoningEffort") &&
      source.includes("amazonBedrock")
    ) {
      patched = replaceUniqueRendererGate(
        patched,
        /(\b[$A-Z_a-z][$\w]*\s*=\s*\(?\s*[$A-Z_a-z][$\w]*(?:\s*(?:\?\.|\.)\s*[$A-Z_a-z][$\w]*)?\s*\)?\s*&&\s*)\(?\s*([$A-Z_a-z][$\w]*(?:\s*(?:\?\.|\.)\s*[$A-Z_a-z][$\w]*)?)\s*(?:!==|!=)\s*(["'`])amazonBedrock\3\s*\)?/g,
        (_match, visibilityPrefix, authMethodExpression) =>
          `${visibilityPrefix}${authMethodExpression}=== \`chatgpt\``,
        "model visibility",
      );
    }
    if (
      source.includes("isServiceTierAllowed") &&
      source.includes("featureRequirements?.fast_mode") &&
      source.includes("authMethod:")
    ) {
      // Model serviceTiers are the authority for whether the control exists.
      // Account requirements and their loading state must never hide it.
      patched = replaceUniqueRendererGate(
        patched,
        /(\b([$A-Z_a-z][$\w]*)\s*=\s*)([$A-Z_a-z][$\w]*)\s*&&\s*!([$A-Z_a-z][$\w]*)\s*&&\s*([$A-Z_a-z][$\w]*)\s*!=\s*null\s*&&\s*\5\?\.requirements\?\.featureRequirements\?\.fast_mode\s*!==\s*!1/g,
        (_match, assignment) => `${assignment}!0`,
        "service tier UI",
      );
    }
    if (
      source.includes("isServiceTierAllowed") &&
      source.includes("serviceTierForRequest:") &&
      source.includes("availableOptions:")
    ) {
      // Preserve the model-aware resolver but remove its entitlement argument.
      // This also covers builds where the permission provider above reshaped.
      patched = replaceUniqueRendererGate(
        patched,
        /(\?\s*)([$A-Z_a-z][$\w]*)\s*\?\s*([$A-Z_a-z][$\w]*)\s*:\s*null\s*:\s*([$A-Z_a-z][$\w]*)\(\s*([$A-Z_a-z][$\w]*)\s*,\s*\3\s*,\s*\2\s*\)/g,
        (_match, _questionMark, _isAllowedName, tierName, resolverName, modelName) =>
          `?${tierName}:${resolverName}(${modelName},${tierName})`,
        "service tier selection permission",
      );
      // Reuse Codex's normalized selected tier for the request too. A Fast tier
      // left over from another model must become null after switching to a
      // model whose serviceTiers do not contain it.
      patched = replaceUniqueRendererGate(
        patched,
        /(\b([$A-Z_a-z][$\w]*)\s*=\s*([$A-Z_a-z][$\w]*)\s*==\s*null\s*\?\s*null\s*:\s*([$A-Z_a-z][$\w]*)\(\s*([$A-Z_a-z][$\w]*)\s*,\s*\3\s*\))(?=\s*;\s*let\s+[$A-Z_a-z][$\w]*\s*=\s*[$A-Z_a-z][$\w]*\(\s*\3\s*\?\?\s*null\s*\))/g,
        (_match, selectedExpression, selectedName, requestTierName) =>
          `${selectedExpression},${requestTierName}=${selectedName}`,
        "service tier model validation",
      );
      // Requirements can remain pending independently of the model catalog.
      // Do not report that entitlement fetch as service-tier option loading.
      patched = replaceUniqueRendererGate(
        patched,
        /(\b([$A-Z_a-z][$\w]*)\s*=\s*([$A-Z_a-z][$\w]*)\.isLoading\s*\|\|\s*([$A-Z_a-z][$\w]*)\s*\|\|\s*([$A-Z_a-z][$\w]*)\.isLoading)\s*\|\|\s*[$A-Z_a-z][$\w]*\s*==\s*null\s*&&\s*[$A-Z_a-z][$\w]*(?=\s*,)/g,
        (_match, modelLoadingExpression) => modelLoadingExpression,
        "service tier entitlement loading",
      );
    }
    if (
      source.includes("composer.toggleFastMode") &&
      source.includes("isServiceTierAllowed") &&
      source.includes("availableOptions.length")
    ) {
      // The current model's options decide whether the speed control exists.
      patched = replaceUniqueRendererGate(
        patched,
        /(\b([$A-Z_a-z][$\w]*)\s*=\s*)[$A-Z_a-z][$\w]*\s*&&\s*([$A-Z_a-z][$\w]*)\.availableOptions\.length\s*>\s*1(?=\s*[,;][\s\S]{0,2048}?`composer\.toggleFastMode`)/g,
        (_match, assignment, _resultName, settingsName) =>
          `${assignment}${settingsName}.availableOptions.length>1`,
        "model-aware service tier control",
      );
      patched = replaceUniqueRendererGate(
        patched,
        /(`composer\.toggleFastMode`[\s\S]{0,512}?\{\s*enabled\s*:\s*)[$A-Z_a-z][$\w]*\s*&&\s*!\s*([$A-Z_a-z][$\w]*)\s*&&\s*([$A-Z_a-z][$\w]*)\s*!=\s*null/g,
        (_match, prefix, loadingName, fastOptionName) =>
          `${prefix}!${loadingName}&&${fastOptionName}!=null`,
        "model-aware Fast toggle",
      );
    }
    if (
      source.includes("composer.speedSlashCommand.disableDescription") &&
      source.includes("isServiceTierAllowed") &&
      source.includes("availableOptions.map")
    ) {
      // These commands are created only for service tiers exposed by the model.
      patched = replaceUniqueRendererGate(
        patched,
        /(enabled\s*:\s*)[$A-Z_a-z][$\w]*\s*&&\s*!\s*([$A-Z_a-z][$\w]*)\.isLoading(?=\s*,\s*isSelected\s*:)/g,
        (_match, assignment, settingsName) =>
          `${assignment}!${settingsName}.isLoading`,
        "model-aware service tier commands",
      );
    }
    if (
      source.includes("isServiceTierAllowed") &&
      /availableOptions\.length\s*<=\s*1/.test(source) &&
      source.includes("selectedServiceTier")
    ) {
      patched = replaceUniqueRendererGate(
        patched,
        /if\s*\(\s*!\s*([$A-Z_a-z][$\w]*)\s*\|\|\s*([$A-Z_a-z][$\w]*)\.availableOptions\.length\s*<=\s*1\s*\)\s*return\s+null/g,
        (_match, _isAllowedName, settingsName) =>
          `if(${settingsName}.availableOptions.length<=1)return null`,
        "service tier settings UI",
      );
    }
    if (
      source.includes("Failed to load config requirements for service tier") &&
      source.includes("featureRequirements?.fast_mode")
    ) {
      // A tier selected from the current model must not be stripped from thread
      // requests by an account entitlement lookup.
      patched = replaceUniqueRendererGate(
        patched,
        /if\s*\(\s*\(\s*await\s+([$A-Z_a-z][$\w]*)\(\s*\)\s*\)\.requirements\?\.featureRequirements\?\.fast_mode\s*===\s*!1\s*\)\s*return\s+null/g,
        "",
        "service tier request sanitizer",
      );
    }
    if (
      source.includes("Failed to read service tier for request") &&
      source.includes("featureRequirements?.fast_mode")
    ) {
      patched = replaceUniqueRendererGate(
        patched,
        /async\s+function\s+([$A-Z_a-z][$\w]*)\(\s*([$A-Z_a-z][$\w]*)\s*,\s*([$A-Z_a-z][$\w]*)\s*\)\s*\{\s*let\s+([$A-Z_a-z][$\w]*)\s*=\s*await\s+[$A-Z_a-z][$\w]*\(\s*\2\s*,\s*\3\s*\)\s*;\s*if\s*\(\s*\4\s*!==\s*`chatgpt`\s*\)\s*return\s*!1\s*;[\s\S]{0,768}?\.requirements\?\.featureRequirements\?\.fast_mode\s*!==\s*!1\s*\}/g,
        (_match, functionName, firstArgumentName, secondArgumentName) =>
          `async function ${functionName}(${firstArgumentName},${secondArgumentName}){return!0}`,
        "service tier request entitlement",
      );
    }
    if (
      source.includes("composer.intelligenceDropdown.model.title") &&
      source.includes("composer.intelligenceDropdown.model.rowLabel") &&
      source.includes("modelPickerTriggerConfig:") &&
      source.includes("selectedServiceTierIconKind:") &&
      source.includes("showFastServiceTierIndicator:")
    ) {
      // Third-party catalogs can expose fewer power selections than Codex's
      // native threshold even though model, effort, and Fast are all available.
      // Keep the modern native trigger in that case: it owns the filled Fast
      // indicator and avoids falling back to the legacy outlined model icon.
      patched = replaceUniqueRendererGate(
        patched,
        /(\b([$A-Z_a-z][$\w]*)\s*=\s*)[$A-Z_a-z][$\w]*\s*&&\s*!\s*([$A-Z_a-z][$\w]*)(?=\s*,[\s\S]{0,8192}?modelPickerTriggerConfig\s*:\s*\2\s*\?)/g,
        (
          _match,
          assignment,
          _triggerConfigName,
          hideLabelName,
        ) => `${assignment}!${hideLabelName}`,
        "fast model trigger availability",
      );
      // Preserve Codex's native Fast indicators. Its own model/tier support
      // checks already prevent them from appearing on unsupported models.
      patched = replaceUniqueRendererGate(
        patched,
        /(modelPickerTriggerConfig\s*:\s*([$A-Z_a-z][$\w]*)\s*[,}][\s\S]{0,2048}?selectedServiceTierIconKind\s*:[\s\S]{0,12288}?)if\s*\(\s*[$A-Z_a-z][$\w]*\s*&&\s*\2\s*!=\s*null\s*\)|if\s*\(\s*[$A-Z_a-z][$\w]*\s*&&\s*modelPickerTriggerConfig\s*!=\s*null\s*\)/g,
        (_match, aliasedPrefix, triggerConfigName) =>
          aliasedPrefix == null
            ? "if(modelPickerTriggerConfig!=null)"
            : `${aliasedPrefix}if(${triggerConfigName}!=null)`,
        "fast model trigger fallback",
      );
    }
    if (
      fastCodexStartup &&
      source.includes(
        "CODEX_POST_LOGIN_STATSIG_BOOTSTRAP_FAILURE_TYPE_CLIENT_INITIALIZATION_FAILED",
      ) &&
      source.includes("Statsig: error while bootstrapping post-login client") &&
      source.includes("CodexStatsigProvider.sync")
    ) {
      // Keep the bootstrap call outside the inner StatsigClient block. Minified
      // bundles often reuse the bootstrap argument name for the client binding;
      // moving both into one block would put the argument in that binding's TDZ.
      // A timeout still rejects the mutation and enters the provider fallback.
      patched = replaceUniqueRendererGate(
        patched,
        /let\s+([$A-Z_a-z][$\w]*)\s*=\s*await\s+([$A-Z_a-z][$\w]*)\(\s*([$A-Z_a-z][$\w]*)\s*\)\s*;\s*try\s*\{\s*let\s+([$A-Z_a-z][$\w]*)\s*=\s*new\s+([$A-Z_a-z][$\w]*)\.StatsigClient\s*\(/g,
        (
          _match,
          payloadName,
          bootstrapName,
          bootstrapInputName,
          clientName,
          statsigModuleName,
        ) =>
          `let ${payloadName}=await Promise.race([${bootstrapName}(${bootstrapInputName}),new Promise((_,reject)=>globalThis.setTimeout(()=>reject(new Error("Codey Statsig bootstrap timeout")),${statsigStartupRemainingMs}))]);try{let ${clientName}=new ${statsigModuleName}.StatsigClient(`,
        "post-login Statsig bootstrap timeout",
      );
    }
    if (
      fastCodexStartup &&
      source.includes("useStatsigInternalClientFactoryAsync") &&
      source.includes("_getInstance") &&
      source.includes("loadingStatus")
    ) {
      // The SDK's anonymous-client hook otherwise keeps the entire route tree
      // behind its loading placeholder until initializeAsync settles.
      patched = replaceUniqueRendererGate(
        patched,
        /([$A-Z_a-z][$\w]*)\.loadingStatus\s*!==\s*`Ready`\s*&&\s*\1\.initializeAsync\(\)\.catch\s*\(/g,
        (_match, clientName) =>
          `${clientName}.loadingStatus!==\`Ready\`&&Promise.race([${clientName}.initializeAsync(),new Promise((_,reject)=>globalThis.setTimeout(()=>reject(new Error("Codey Statsig async initialization timeout")),${statsigStartupRemainingMs}))]).catch(`,
        "Statsig async client initialization timeout",
      );
    }
    if (
      source.includes("activeInteractions=new Map") &&
      source.includes("beginCpuSampling") &&
      source.includes(
        "ensureHeartbeat(){this.heartbeatTimer??=setInterval",
      ) &&
      source.includes("rendererProcessCpuPercentAvg")
    ) {
      // Codey launches app-server with analytics.enabled=false, so renderer
      // interaction telemetry is discarded after paying for main/renderer CPU
      // snapshots and a 1 Hz heartbeat. Preserve span lifecycle semantics while
      // removing only those two recurring/IPC costs.
      patched = replaceUniqueRendererGate(
        patched,
        /cpuSampling:([$A-Z_a-z][$\w]*)===`dropped`\|\|([$A-Z_a-z][$\w]*)\.backfilled===!0\?null:this\.beginCpuSampling\(\)/g,
        "cpuSampling:null",
        "interaction CPU sampling",
      );
      patched = replaceUniqueRendererGate(
        patched,
        /ensureHeartbeat\(\)\{this\.heartbeatTimer\?\?=setInterval\(\(\)=>\{let ([$A-Z_a-z][$\w]*)=this\.now\(\),([$A-Z_a-z][$\w]*)=this\.wallNow\(\);for\(let ([$A-Z_a-z][$\w]*) of this\.activeInteractions\.values\(\)\)this\.recordHeartbeat\(\3,\1,\2\)\},([$A-Z_a-z][$\w]*)\)\}/g,
        "ensureHeartbeat(){}",
        "interaction heartbeat",
      );
    }
    return patched;
  };
  const isCodexRendererAssetRequest = (request) => {
    try {
      const url = new URL(request?.url);
      return (
        url.protocol === "app:" &&
        url.pathname.includes("/assets/") &&
        (
          /\/(?:(?:app-initial|codex-composer-adapter|general-settings|model-list-filter|windows-model-controls|use-service-tier-settings|read-service-tier-for-request)(?:[~-][^/]*)?)\.js$/i.test(
            url.pathname,
          )
          || (
            disablePet
            && /\/(?:(?:appearance-settings|pet-settings|pets-settings)(?:[~-][^/]*)?)\.js$/i.test(
              url.pathname,
            )
          )
        )
      );
    } catch {
      return false;
    }
  };
  const patchCodexRendererResponse = async (request, response) => {
    if (!isCodexRendererAssetRequest(request) || response?.ok !== true) return response;
    const source = await response.clone().text();
    let patched;
    try {
      patched = patchCodexRendererAsset(source);
    } catch (error) {
      // Codex renderer bundles are minified implementation details and their
      // shapes change between releases. These UI restorations are optional:
      // never turn a stale patch anchor into a failed app:// module request,
      // otherwise Codex remains on its static startup loader forever.
      recordCodeyPatchFailure("patch_codex_renderer_asset", error, {
        requestUrl: request?.url,
      });
      try {
        console.error("Codey skipped an incompatible Codex renderer patch", error);
      } catch {}
      return response;
    }
    if (patched === source) return response;
    const headers = new Headers(response.headers);
    headers.delete("content-length");
    return new Response(patched, {
      headers,
      status: response.status,
      statusText: response.statusText,
    });
  };

  // The inspector is only a startup injection mechanism. Do not pass its
  // pause state or command-line flags to Codex workers.
  process.execArgv.splice(
    0,
    process.execArgv.length,
    ...process.execArgv.filter((argument) => !isInspectorArgument(argument)),
  );
  process.argv.splice(
    0,
    process.argv.length,
    ...process.argv.filter((argument) => !isInspectorArgument(argument)),
  );

  // The desktop client explicitly opts the bundled app-server into analytics.
  // Remove that opt-in and add a command-local config override without touching
  // the user's persistent Codex configuration.
  const appServerAnalyticsConfig = "analytics.enabled=false";
  const rewriteCodexAppServerArgs = (args) => {
    if (!Array.isArray(args)) return args;
    const appServerIndexes = args
      .map((argument, index) => argument === "app-server" ? index : -1)
      .filter((index) => index >= 0);
    const analyticsFlagCount = args
      .filter((argument) => argument === "--analytics-default-enabled")
      .length;
    if (appServerIndexes.length !== 1 || analyticsFlagCount !== 1) return args;

    const rewritten = args.filter(
      (argument) => argument !== "--analytics-default-enabled",
    );
    let hasAnalyticsConfig = false;
    for (let index = 0; index < rewritten.length; index += 1) {
      const argument = rewritten[index];
      if (
        (argument === "-c" || argument === "--config") &&
        /^analytics\.enabled=/.test(String(rewritten[index + 1] ?? ""))
      ) {
        rewritten[index + 1] = appServerAnalyticsConfig;
        hasAnalyticsConfig = true;
      } else if (/^--config=analytics\.enabled=/.test(String(argument))) {
        rewritten[index] = `--config=${appServerAnalyticsConfig}`;
        hasAnalyticsConfig = true;
      }
    }
    if (!hasAnalyticsConfig) {
      const appServerIndex = rewritten.indexOf("app-server");
      rewritten.splice(appServerIndex, 0, "-c", appServerAnalyticsConfig);
    }
    return rewritten;
  };
  const rewriteCodexAppServerShellCommand = (command) => {
    if (typeof command !== "string") return command;
    const execMatches = [...command.matchAll(/(?:^|;)\s*exec\s+/g)];
    if (execMatches.length !== 1) return command;
    const execMatch = execMatches[0];
    const execCommandOffset = execMatch.index + execMatch[0].length;
    const execCommand = command.slice(execCommandOffset);
    const executableToken = /^(?:"[^"]+"|'[^']+'|(?:\\.|[^\s;&|])+)/.exec(
      execCommand,
    )?.[0];
    if (executableToken == null) return command;
    const normalizedExecutable = executableToken
      .replace(/^(["'])|(["'])$/g, "")
      .replace(/\\ /g, " ");
    if (!/(?:^|[/\\])codex(?:\.exe)?$/i.test(normalizedExecutable)) {
      return command;
    }

    const appServerMatches = execCommand.match(/\bapp-server\b/g);
    const analyticsFlagMatches = execCommand.match(
      /(?:^|[\s;])--analytics-default-enabled(?=$|[\s;&|])/g,
    );
    if (appServerMatches?.length !== 1 || analyticsFlagMatches?.length !== 1) {
      return command;
    }

    let hasAnalyticsConfig = false;
    let rewritten = execCommand.replace(
      /(^|[\s;])(-c|--config)\s+analytics\.enabled=[^\s;&|]+(?=$|[\s;&|])/g,
      (_match, prefix, configFlag) => {
        hasAnalyticsConfig = true;
        return `${prefix}${configFlag} ${appServerAnalyticsConfig}`;
      },
    );
    rewritten = rewritten.replace(
      /(^|[\s;])--config=analytics\.enabled=[^\s;&|]+(?=$|[\s;&|])/g,
      (_match, prefix) => {
        hasAnalyticsConfig = true;
        return `${prefix}--config=${appServerAnalyticsConfig}`;
      },
    );
    rewritten = rewritten.replace(
      /(^|[\s;])--analytics-default-enabled(?=$|[\s;&|])/g,
      (_match, prefix) => prefix,
    );
    if (!hasAnalyticsConfig) {
      rewritten = rewritten.replace(
        /\bapp-server\b/,
        `-c ${appServerAnalyticsConfig} app-server`,
      );
    }
    return command.slice(0, execCommandOffset) + rewritten;
  };
  const rewriteCodexAppServerSpawnArgs = (command, args) => {
    if (!Array.isArray(args)) return args;
    const commandName = String(command ?? "");
    if (/(?:^|[/\\])codex(?:\.exe)?$/i.test(commandName)) {
      return rewriteCodexAppServerArgs(args);
    }
    if (!/(?:^|[/\\])wsl(?:\.exe)?$/i.test(commandName)) return args;

    const shellFlagIndexes = args
      .map((argument, index) => argument === "-lc" ? index : -1)
      .filter((index) => index >= 0);
    if (shellFlagIndexes.length !== 1) return args;
    const shellFlagIndex = shellFlagIndexes[0];
    if (
      !/(?:^|[/\\])bash$/i.test(String(args[shellFlagIndex - 1] ?? "")) ||
      typeof args[shellFlagIndex + 1] !== "string"
    ) {
      return args;
    }
    const rewrittenCommand = rewriteCodexAppServerShellCommand(
      args[shellFlagIndex + 1],
    );
    if (rewrittenCommand === args[shellFlagIndex + 1]) return args;
    const rewritten = [...args];
    rewritten[shellFlagIndex + 1] = rewrittenCommand;
    return rewritten;
  };
  Object.defineProperty(globalThis, "__CODEY_REWRITE_CODEX_APP_SERVER_ARGS__", {
    configurable: false,
    value: rewriteCodexAppServerSpawnArgs,
    writable: false,
  });

  let appServerAnalyticsPatchCount = 0;
  const childProcess = process.getBuiltinModule("child_process");
  const NativeSpawn = childProcess.spawn;
  if (!NativeSpawn.__codeyAppServerAnalyticsDisabled) {
    const codeyAnalyticsDisabledSpawn = function (command, args, ...rest) {
      const rewritten = rewriteCodexAppServerSpawnArgs(command, args);
      if (rewritten === args) {
        return Reflect.apply(NativeSpawn, this, arguments);
      }
      appServerAnalyticsPatchCount += 1;
      return Reflect.apply(NativeSpawn, this, [command, rewritten, ...rest]);
    };
    Object.defineProperty(
      codeyAnalyticsDisabledSpawn,
      "__codeyAppServerAnalyticsDisabled",
      { value: true },
    );
    childProcess.spawn = codeyAnalyticsDisabledSpawn;
  }

  const externalPluginFocusReconcileMinIntervalMs = 30_000;
  let externalPluginFocusReconcileSuppressedCount = 0;
  const throttleExternalPluginFocusReconcile = (
    listener,
    minimumIntervalMs = externalPluginFocusReconcileMinIntervalMs,
  ) => {
    const monotonicNow = () => globalThis.performance?.now?.() ?? Date.now();
    let lastRunAt = Number.NEGATIVE_INFINITY;
    let trailingTimer = null;
    let trailingThis = null;
    let trailingArgs = null;
    const invoke = (receiver, args) => {
      lastRunAt = monotonicNow();
      trailingThis = null;
      trailingArgs = null;
      return Reflect.apply(listener, receiver, args);
    };
    const wrapped = function (...args) {
      const elapsed = monotonicNow() - lastRunAt;
      if (trailingTimer == null && elapsed >= minimumIntervalMs) {
        return invoke(this, args);
      }
      externalPluginFocusReconcileSuppressedCount += 1;
      trailingThis = this;
      trailingArgs = args;
      if (trailingTimer == null) {
        trailingTimer = setTimeout(() => {
          trailingTimer = null;
          invoke(trailingThis, trailingArgs ?? []);
        }, Math.max(1, minimumIntervalMs - elapsed));
        trailingTimer.unref?.();
      }
      return undefined;
    };
    Object.defineProperty(wrapped, "cancel", {
      configurable: false,
      value: () => {
        if (trailingTimer != null) clearTimeout(trailingTimer);
        trailingTimer = null;
        trailingThis = null;
        trailingArgs = null;
      },
      writable: false,
    });
    return wrapped;
  };
  Object.defineProperty(
    globalThis,
    "__CODEY_THROTTLE_EXTERNAL_PLUGIN_FOCUS_RECONCILE__",
    {
      configurable: false,
      value: throttleExternalPluginFocusReconcile,
      writable: false,
    },
  );
  const patchCodexMainFocusReconcile = (source) => {
    if (
      !source.includes("browser-window-focus") ||
      !source.includes("reconcileExternalPluginState")
    ) {
      throw new Error("Codey external plugin focus reconcile anchors not found");
    }
    let listenerName = null;
    let count = 0;
    let patched = source.replace(
      /(\b[$A-Z_a-z][$\w]*)=\(\)=>\{([$A-Z_a-z][$\w]*)\.reconcileExternalPluginState\((`focus`|"focus"|'focus')\)\}/g,
      (_match, matchedListenerName, coordinatorName, focusLiteral) => {
        count += 1;
        listenerName = matchedListenerName;
        return (
          `${matchedListenerName}=globalThis.` +
          `__CODEY_THROTTLE_EXTERNAL_PLUGIN_FOCUS_RECONCILE__(` +
          `()=>{${coordinatorName}.reconcileExternalPluginState(${focusLiteral})})`
        );
      },
    );
    if (count !== 1) {
      throw new Error(
        `Codey external plugin focus reconcile matched ${count} times`,
      );
    }
    let cleanupCount = 0;
    patched = patched.replace(
      /(\b[$A-Z_a-z][$\w]*)\.add\(\(\)=>\{([$A-Z_a-z][$\w]*)\.app\.off\((`browser-window-focus`|"browser-window-focus"|'browser-window-focus'),([$A-Z_a-z][$\w]*)\)\}\)/g,
      (match, disposerName, appName, eventLiteral, cleanupListenerName) => {
        if (cleanupListenerName !== listenerName) return match;
        cleanupCount += 1;
        return (
          `${disposerName}.add(()=>{${appName}.app.off(` +
          `${eventLiteral},${cleanupListenerName}),${cleanupListenerName}.cancel?.()})`
        );
      },
    );
    if (cleanupCount !== 1) {
      throw new Error(
        `Codey external plugin focus reconcile cleanup matched ${cleanupCount} times`,
      );
    }
    return patched;
  };
  Object.defineProperty(
    globalThis,
    "__CODEY_PATCH_CODEX_MAIN_FOCUS_RECONCILE__",
    {
      configurable: false,
      value: patchCodexMainFocusReconcile,
      writable: false,
    },
  );

  // Desktop CES telemetry has its own main-process transport and worker
  // transport. Disable the transport promise, worker bootstrap value, and the
  // later startup-config update explicitly so no events queue while app-server
  // configuration is still resolving.
  const patchCodexMainDesktopAnalytics = (source) => {
    let workerBootstrapCount = 0;
    let workerUpdateCount = 0;
    let mainTransportCount = 0;
    let patched = source.replace(
      /analyticsEnabled:([$A-Z_a-z][$\w]*)!=null&&\1\.analytics\?\.enabled!==!1/g,
      () => {
        workerBootstrapCount += 1;
        return "analyticsEnabled:!1";
      },
    );
    patched = patched.replace(
      /postMessage\(\{type:(`worker-analytics-enabled-update`|"worker-analytics-enabled-update"|'worker-analytics-enabled-update'),enabled:([$A-Z_a-z][$\w]*)\.analytics\?\.enabled!==!1\}\)/g,
      (_match, messageLiteral) => {
        workerUpdateCount += 1;
        return `postMessage({type:${messageLiteral},enabled:!1})`;
      },
    );
    patched = patched.replace(
      /analyticsEnabled:([$A-Z_a-z][$\w]*)\.get\(\)\.then\(([$A-Z_a-z][$\w]*)=>\2\.analytics\?\.enabled!==!1\)/g,
      () => {
        mainTransportCount += 1;
        return "analyticsEnabled:!1";
      },
    );
    if (
      workerBootstrapCount !== 1 ||
      workerUpdateCount !== 1 ||
      mainTransportCount !== 1
    ) {
      throw new Error(
        "Codey desktop analytics matches " +
        `${workerBootstrapCount}/${workerUpdateCount}/${mainTransportCount}`,
      );
    }
    return patched;
  };
  Object.defineProperty(
    globalThis,
    "__CODEY_PATCH_CODEX_MAIN_DESKTOP_ANALYTICS__",
    {
      configurable: false,
      value: patchCodexMainDesktopAnalytics,
      writable: false,
    },
  );

  // Codex's sampler manager asks the focused renderer for a full diagnostic
  // app-state snapshot every 30 seconds, then only records it as a debug log and
  // Sentry breadcrumb. Keep renderer-ready and explicit trigger snapshots, but
  // remove the periodic diagnostic heartbeat.
  const patchCodexMainAppStateHeartbeat = (source) => {
    if (
      !source.includes("appStateHeartbeat") ||
      !source.includes("electron-app-state-snapshot-request")
    ) {
      throw new Error("Codey app-state heartbeat anchors not found");
    }
    let count = 0;
    const patched = source.replace(
      /this\.appStateHeartbeat=setInterval\(\(\)=>\{this\.requestAppStateSnapshot\((`heartbeat`|"heartbeat"|'heartbeat')\)\},[$A-Z_a-z][$\w]*\),this\.appStateHeartbeat\.unref\(\)/g,
      () => {
        count += 1;
        return "this.appStateHeartbeat=null";
      },
    );
    if (count !== 1) {
      throw new Error(`Codey app-state heartbeat matched ${count} times`);
    }
    return patched;
  };
  Object.defineProperty(
    globalThis,
    "__CODEY_PATCH_CODEX_MAIN_APP_STATE_HEARTBEAT__",
    {
      configurable: false,
      value: patchCodexMainAppStateHeartbeat,
      writable: false,
    },
  );

  // Codex prewarms the shared avatar/voice overlay at startup by creating a
  // hidden BrowserWindow. In slim-pet mode the pet entry points are already
  // unavailable, so keep the manager and voice path intact but make prewarm a
  // no-op. Voice can still create the overlay on demand through the manager's
  // regular presentation path.
  const patchCodexAvatarOverlayPrewarm = (source) => {
    if (!disablePet) return source;
    let count = 0;
    const patched = source.replace(
      /async prewarm\(([$A-Z_a-z][$\w]*)\)\{if\(this\.window!=null\|\|this\.openingWindowPromise!=null\|\|this\.isAppQuitting\)return;let ([$A-Z_a-z][$\w]*)=this\.windowVisibilitySequence,([$A-Z_a-z][$\w]*)=await this\.ensureWindow\(\2\);/g,
      (match) => {
        count += 1;
        return match.replace("{", "{return;");
      },
    );
    if (count !== 1) {
      throw new Error(`Codey avatar overlay prewarm matches ${count}`);
    }
    return patched;
  };
  Object.defineProperty(
    globalThis,
    "__CODEY_PATCH_CODEX_AVATAR_OVERLAY_PREWARM__",
    {
      configurable: false,
      value: patchCodexAvatarOverlayPrewarm,
      writable: false,
    },
  );

  const workerThreads = process.getBuiltinModule("worker_threads");
  const NativeWorker = workerThreads.Worker;
  if (!NativeWorker.__codeyNoInspectWrapper) {
    const EventEmitter = process.getBuiltinModule("events").EventEmitter;
    const isWmiSnapshotWorker = (filename) =>
      disableWindowsWmiSampler &&
      /(?:^|[/\\])child-process-snapshot-worker\.js(?:[?#].*)?$/i.test(String(filename));

    // Codex starts this telemetry worker every 30 seconds. On Windows the
    // worker shells out to PowerShell for two full CIM/WMI process scans.
    // Return the protocol's valid empty snapshot without creating a thread,
    // process, timer, or PowerShell child.
    class CodeyDisabledWmiSnapshotWorker extends EventEmitter {
      constructor() {
        super();
        this.threadId = -1;
        this.stdin = null;
        this.stdout = null;
        this.stderr = null;
        this.codeyTerminated = false;
        process.nextTick(() => {
          if (this.codeyTerminated) return;
          this.emit("message", { type: "ok", value: [] });
          this.emit("exit", 0);
        });
      }
      postMessage() {}
      ref() { return this; }
      unref() { return this; }
      terminate() {
        if (!this.codeyTerminated) {
          this.codeyTerminated = true;
          process.nextTick(() => this.emit("exit", 0));
        }
        return Promise.resolve(0);
      }
    }

    class CodeyNoInspectWorker extends NativeWorker {
      constructor(filename, options = {}) {
        if (isWmiSnapshotWorker(filename)) {
          return new CodeyDisabledWmiSnapshotWorker();
        }
        super(filename, {
          ...options,
          execArgv: options.execArgv ?? [],
        });
      }
    }
    Object.defineProperty(CodeyNoInspectWorker, "__codeyNoInspectWrapper", {
      value: true,
    });
    workerThreads.Worker = CodeyNoInspectWorker;
  }

  const temporaryWebViews = new WeakMap();
  const temporaryWebViewLifecycle = Object.freeze({
    close(owner, partition) {
      const guests = temporaryWebViews.get(owner);
      const guest = guests?.get(partition);
      guests?.delete(partition);
      if (guests?.size === 0) temporaryWebViews.delete(owner);
      if (guest != null && !guest.isDestroyed()) guest.close();
    },
    track(owner, partition, guest) {
      let guests = temporaryWebViews.get(owner);
      if (guests == null) {
        guests = new Map();
        temporaryWebViews.set(owner, guests);
      }
      const previous = guests.get(partition);
      if (previous != null && previous !== guest && !previous.isDestroyed()) previous.close();
      guests.set(partition, guest);
      guest.once("destroyed", () => {
        if (guests.get(partition) === guest) guests.delete(partition);
        if (guests.size === 0) temporaryWebViews.delete(owner);
      });
    },
  });
  Object.defineProperty(globalThis, "__CODEY_TEMP_WEBVIEW_LIFECYCLE__", {
    configurable: false,
    value: temporaryWebViewLifecycle,
    writable: false,
  });

  const installExecutionReaper = ({
    connection,
    kill,
    snapshot,
    completionGraceMs: configuredCompletionGraceMs,
  }) => {
    const activeTurns = new Map();
    const completionGraceMs = configuredCompletionGraceMs ?? 1000;
    const reclaimRetryMs = 60 * 1000;
    const terminalTurnStates = new Set([
      "completed",
      "aborted",
      "cancelled",
      "canceled",
      "failed",
      "error",
      "errored",
      "closed",
      "stopped",
      "interrupted",
    ]);
    const terminalThreadMethods = new Set([
      "thread/archived",
      "thread/closed",
      "thread/deleted",
    ]);
    let cleanupPromise = null;
    let reclaimTimer = null;
    let reclaimBarrier = null;
    let reclaimAuthorizedVersion = null;
    let disposed = false;
    let lastTurnActivityAt = Date.now();
    let turnStateVersion = 0;

    const isReclaimable = (processInfo) => {
      const command = String(processInfo?.command ?? "");
      return (
        processInfo?.kind === "mcp" ||
        /(?:^|[/\\])node_repl(?:\.exe)?(?:\s|$)/i.test(command) ||
        /(?:^|[/\\])codegraph\.js\s+serve\b[^\r\n]*--mcp\b/i.test(command) ||
        /(?:^|\s|[/\\])mcp[/\\]server\.mjs(?:\s|$)/i.test(command)
      );
    };

    const clearReclaimTimer = () => {
      if (reclaimTimer == null) return;
      clearTimeout(reclaimTimer);
      reclaimTimer = null;
    };

    const cancelReclaimBarrier = () => {
      if (reclaimBarrier == null) return;
      const barrier = reclaimBarrier;
      reclaimBarrier = null;
      clearTimeout(barrier.timer);
      barrier.resolve(false);
    };

    const isReclaimAuthorized = (expectedVersion) =>
      !disposed &&
      activeTurns.size === 0 &&
      reclaimAuthorizedVersion === expectedVersion &&
      turnStateVersion === expectedVersion;

    const isReclaimSafe = (expectedVersion, now = Date.now()) =>
      isReclaimAuthorized(expectedVersion) &&
      now - lastTurnActivityAt >= completionGraceMs;

    const waitForReclaimBarrier = (expectedVersion, delayMs) => {
      if (!isReclaimSafe(expectedVersion)) return Promise.resolve(false);
      cancelReclaimBarrier();
      return new Promise((resolve) => {
        const timer = setTimeout(() => {
          if (reclaimBarrier?.timer === timer) reclaimBarrier = null;
          resolve(isReclaimSafe(expectedVersion));
        }, Math.max(0, delayMs));
        timer.unref?.();
        reclaimBarrier = { resolve, timer };
      });
    };

    const armReclaim = (reason, minimumDelayMs = 0) => {
      clearReclaimTimer();
      const expectedVersion = reclaimAuthorizedVersion;
      if (expectedVersion == null || !isReclaimAuthorized(expectedVersion)) return;
      const graceRemaining = completionGraceMs - (Date.now() - lastTurnActivityAt);
      reclaimTimer = setTimeout(() => {
        reclaimTimer = null;
        void reclaim(reason);
      }, Math.max(1, graceRemaining, minimumDelayMs));
      reclaimTimer.unref?.();
    };

    const recordTurnStateChange = (now) => {
      lastTurnActivityAt = now;
      turnStateVersion += 1;
      reclaimAuthorizedVersion = null;
      clearReclaimTimer();
      cancelReclaimBarrier();
    };

    const reclaim = (reason) => {
      const expectedVersion = reclaimAuthorizedVersion;
      if (expectedVersion == null) return cleanupPromise;
      if (cleanupPromise != null) return cleanupPromise;
      if (!isReclaimSafe(expectedVersion)) {
        if (isReclaimAuthorized(expectedVersion)) armReclaim(reason);
        return cleanupPromise;
      }
      clearReclaimTimer();
      let cleanupSucceeded = false;
      cleanupPromise = Promise.resolve()
        .then(snapshot)
        .then(async (processes) => {
          // A fresh quiet window after the process snapshot lets queued turn
          // notifications invalidate this cleanup before the first kill.
          if (!await waitForReclaimBarrier(expectedVersion, completionGraceMs)) {
            return { reason, reclaimed: 0 };
          }
          const candidates = processes
            .filter(isReclaimable)
            .sort((left, right) => (right.depth ?? 0) - (left.depth ?? 0));
          let reclaimed = 0;
          let allKillsSucceeded = true;
          for (const processInfo of candidates) {
            // Yield once more immediately before each irreversible operation.
            if (!await waitForReclaimBarrier(expectedVersion, 0)) {
              break;
            }
            try {
              if (await kill(processInfo.pid) !== false) reclaimed += 1;
              else allKillsSucceeded = false;
            } catch {
              allKillsSucceeded = false;
            }
            if (!isReclaimSafe(expectedVersion)) break;
          }
          cleanupSucceeded =
            allKillsSucceeded &&
            reclaimed === candidates.length &&
            isReclaimSafe(expectedVersion);
          return { reason, reclaimed };
        })
        .catch(() => ({ reason, reclaimed: 0 }))
        .finally(() => {
          cleanupPromise = null;
          cancelReclaimBarrier();
          if (disposed) return;
          if (cleanupSucceeded && isReclaimSafe(expectedVersion)) {
            reclaimAuthorizedVersion = null;
            return;
          }
          if (reclaimAuthorizedVersion != null) {
            armReclaim(
              "turn-state-changed",
              reclaimAuthorizedVersion === expectedVersion ? reclaimRetryMs : 0,
            );
          }
        });
      return cleanupPromise;
    };

    const normalizedId = (value) =>
      typeof value === "string" && value.length > 0 ? value : null;
    const turnKey = (threadId, turnId) => `${threadId}\u0000${turnId}`;
    const markTurnActivity = (threadId, turnId, now) => {
      const key = turnKey(threadId, turnId);
      const turn = activeTurns.get(key);
      if (turn == null) return false;
      activeTurns.set(key, { ...turn, lastSeen: now });
      return true;
    };
    const removeThreadTurns = (threadId) => {
      let changed = false;
      for (const [key, turn] of activeTurns) {
        if (turn.threadId !== threadId) continue;
        activeTurns.delete(key);
        changed = true;
      }
      return changed;
    };

    let unsubscribe = connection.registerInternalNotificationHandler((notification) => {
      if (disposed) return;
      const method =
        typeof notification?.method === "string"
          ? notification.method.toLowerCase()
          : "";
      const params = notification?.params;
      const threadId = normalizedId(
        params?.threadId ?? params?.thread_id ?? params?.thread?.id,
      );
      const turnId = normalizedId(
        params?.turn?.id ?? params?.turnId ?? params?.turn_id,
      );
      const now = Date.now();
      const terminalTurnState =
        method.startsWith("turn/") && terminalTurnStates.has(method.slice(5));
      const terminalThread = terminalThreadMethods.has(method);

      if (method === "turn/started" && threadId != null && turnId != null) {
        recordTurnStateChange(now);
        activeTurns.set(turnKey(threadId, turnId), { threadId, turnId, lastSeen: now });
        return;
      }

      if (terminalTurnState || terminalThread) {
        let changed = false;
        if (terminalThread && threadId != null) {
          changed = removeThreadTurns(threadId);
        } else if (threadId != null && turnId != null) {
          changed = activeTurns.delete(turnKey(threadId, turnId));
        } else if (threadId != null) {
          changed = removeThreadTurns(threadId);
        }
        // A terminal event that does not match a turn observed by this
        // subscription cannot prove that the connection is globally idle.
        if (!changed) return;
        recordTurnStateChange(now);
        if (activeTurns.size > 0) return;
        reclaimAuthorizedVersion = turnStateVersion;
        armReclaim(`task-${method.slice(method.lastIndexOf("/") + 1)}`);
        return;
      }

      if (threadId == null || turnId == null) return;
      if (!markTurnActivity(threadId, turnId, now)) return;
      recordTurnStateChange(now);
    });
    return () => {
      if (disposed) return;
      disposed = true;
      turnStateVersion += 1;
      reclaimAuthorizedVersion = null;
      clearReclaimTimer();
      cancelReclaimBarrier();
      activeTurns.clear();
      const disposeNotifications = unsubscribe;
      unsubscribe = null;
      try { disposeNotifications?.(); } catch {}
    };
  };
  Object.defineProperty(globalThis, "__CODEY_INSTALL_EXECUTION_REAPER__", {
    configurable: false,
    value: installExecutionReaper,
    writable: false,
  });

  const optionalMainBundlePatchFailures = [];
  const hasOptionalMainBundlePatchFailure = (name) =>
    optionalMainBundlePatchFailures.some((failure) => failure.name === name);
  const applyOptionalMainBundlePatch = (name, patch, source) => {
    try {
      const patched = patch(source);
      const failureIndex = optionalMainBundlePatchFailures.findIndex(
        (failure) => failure.name === name,
      );
      if (failureIndex >= 0) {
        optionalMainBundlePatchFailures.splice(failureIndex, 1);
      }
      return patched;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const failure = { name, message };
      const failureIndex = optionalMainBundlePatchFailures.findIndex(
        (entry) => entry.name === name,
      );
      if (failureIndex >= 0) {
        optionalMainBundlePatchFailures[failureIndex] = failure;
      } else {
        optionalMainBundlePatchFailures.push(failure);
      }
      recordCodeyPatchFailure(`optional_main_bundle_patch:${name}`, error, {
        patchName: name,
      });
      console.warn(`[Codey] skipped incompatible ${name} patch: ${message}`);
      return source;
    }
  };
  Object.defineProperty(
    globalThis,
    "__CODEY_APPLY_OPTIONAL_MAIN_BUNDLE_PATCH__",
    {
      configurable: false,
      value: applyOptionalMainBundlePatch,
      writable: false,
    },
  );

  // Install the main-bundle lifecycle and telemetry patches before V8 compiles
  // the monolithic bundle. Slim-pet mode skips only the eager hidden overlay
  // prewarm; the manager and native composition bridge stay available for
  // explicit voice use.
  {
    const originalJsExtension = Module._extensions[".js"];
    Module._extensions[".js"] = function codeyMainBundleCompileHook(module, filename) {
      const isCodexMainBundle =
        /[\\/]\.vite[\\/]build[\\/]main-[^\\/]+\.js$/i.test(filename);
      if (!isCodexMainBundle) {
        return Reflect.apply(originalJsExtension, this, arguments);
      }

      try {
      const fs = process.getBuiltinModule("fs");
      let source = fs.readFileSync(filename, "utf8");
      source = applyOptionalMainBundlePatch(
        "desktopCesAnalytics",
        patchCodexMainDesktopAnalytics,
        source,
      );
      source = applyOptionalMainBundlePatch(
        "externalPluginFocusReconcile",
        patchCodexMainFocusReconcile,
        source,
      );
      source = applyOptionalMainBundlePatch(
        "appStateHeartbeat",
        patchCodexMainAppStateHeartbeat,
        source,
      );
      if (disablePet) {
        source = applyOptionalMainBundlePatch(
          "avatarOverlayPrewarm",
          patchCodexAvatarOverlayPrewarm,
          source,
        );
      }
      const presentationCall = source.match(
        /case`checkout-webview-presentation-changed`:([$A-Z_a-z][$\w]*)\(([$A-Z_a-z][$\w]*),([$A-Z_a-z][$\w]*)\);break/,
      );
      if (!presentationCall) {
        throw new Error("Codey temporary WebView close anchor not found");
      }
      const presentationFunctionName = presentationCall[1].replace(/[$]/g, "\\$&");
      const presentationFunction = new RegExp(
        "function " + presentationFunctionName +
          "\\(([$A-Z_a-z][$\\w]*),\\{partition:([$A-Z_a-z][$\\w]*),url:([$A-Z_a-z][$\\w]*)\\}\\)\\{",
      ).exec(source);
      if (!presentationFunction) {
        throw new Error("Codey temporary WebView presentation handler not found");
      }
      const ownerName = presentationFunction[1];
      const partitionName = presentationFunction[2];
      const urlName = presentationFunction[3];
      const closeBranch = `if(${urlName}==null){`;
      const closeBranchOffset = source.indexOf(closeBranch, presentationFunction.index);
      if (closeBranchOffset < 0 || closeBranchOffset > presentationFunction.index + 1000) {
        throw new Error("Codey temporary WebView close branch not found");
      }
      source =
        source.slice(0, closeBranchOffset + closeBranch.length) +
        `globalThis.__CODEY_TEMP_WEBVIEW_LIFECYCLE__.close(${ownerName},${partitionName});` +
        source.slice(closeBranchOffset + closeBranch.length);

      const attachFunctionPattern =
        /function [$A-Z_a-z][$\w]*\(\{getAuthToken:[$A-Z_a-z][$\w]*[^{}]{0,500},owner:([$A-Z_a-z][$\w]*)\}\)\{/g;
      let attachFunction = null;
      for (const candidate of source.matchAll(attachFunctionPattern)) {
        const nearby = source.slice(candidate.index, candidate.index + 2500);
        if (nearby.includes("will-attach-webview") && nearby.includes("did-attach-webview")) {
          attachFunction = candidate;
          break;
        }
      }
      if (!attachFunction) {
        throw new Error("Codey temporary WebView attach handler not found");
      }
      const attachOwnerName = attachFunction[1];
      const attachTail = source.slice(attachFunction.index, attachFunction.index + 3000);
      const shiftedEntry =
        /let ([$A-Z_a-z][$\w]*)=[$A-Z_a-z][$\w]*\.shift\(\);if\(\1==null\)return;/.exec(attachTail);
      if (!shiftedEntry) {
        throw new Error("Codey temporary WebView attachment queue not found");
      }
      const guestReference = /webContents:([$A-Z_a-z][$\w]*)/.exec(
        attachTail.slice(shiftedEntry.index + shiftedEntry[0].length),
      );
      if (!guestReference) {
        throw new Error("Codey temporary WebView guest reference not found");
      }
      const trackOffset = attachFunction.index + shiftedEntry.index + shiftedEntry[0].length;
      source =
        source.slice(0, trackOffset) +
        `globalThis.__CODEY_TEMP_WEBVIEW_LIFECYCLE__.track(${attachOwnerName},${shiftedEntry[1]}.partition,${guestReference[1]});` +
        source.slice(trackOffset);

      const reaperAnchorPattern =
        /([$A-Z_a-z][$\w]*)\.add\(([$A-Z_a-z][$\w]*)\(\{appServerConnection:([$A-Z_a-z][$\w]*)\(\),closeActiveTurn:([$A-Z_a-z][$\w]*)\.closeActiveTurn\}\)\);/;
      const reaperAnchor = reaperAnchorPattern.exec(source);
      if (!reaperAnchor) {
        throw new Error("Codey execution reaper completion anchor not found");
      }
      const reaperTail = source.slice(reaperAnchor.index, reaperAnchor.index + 5000);
      const processManagerReference =
        /new [$A-Z_a-z][$\w]*\(([$A-Z_a-z][$\w]*)\.getBrowserSessionRegistry\(\)\)/.exec(reaperTail);
      if (!processManagerReference) {
        throw new Error("Codey execution process manager anchor not found");
      }
      const disposerName = reaperAnchor[1];
      const connectionFactoryName = reaperAnchor[3];
      const processManagerName = processManagerReference[1];
      const reaperInstall =
        `${disposerName}.add(globalThis.__CODEY_INSTALL_EXECUTION_REAPER__({` +
        `connection:${connectionFactoryName}(),` +
        `snapshot:()=>${processManagerName}.listProcessManagerSnapshot(),` +
        `kill:async pid=>(await ${processManagerName}.handlers["child-process-kill"]({pid})).killed` +
        `}));`;
      const reaperOffset = reaperAnchor.index + reaperAnchor[0].length;
      source = source.slice(0, reaperOffset) + reaperInstall + source.slice(reaperOffset);

      globalThis.__CODEY_TEMP_WEBVIEW_SOURCE_PATCHED__ = true;
      globalThis.__CODEY_EXECUTION_REAPER_SOURCE_PATCHED__ = true;
      globalThis.__CODEY_EXTERNAL_PLUGIN_FOCUS_RECONCILE_SOURCE_PATCHED__ =
        !hasOptionalMainBundlePatchFailure("externalPluginFocusReconcile");
      globalThis.__CODEY_DESKTOP_ANALYTICS_SOURCE_PATCHED__ =
        !hasOptionalMainBundlePatchFailure("desktopCesAnalytics");
      globalThis.__CODEY_APP_STATE_HEARTBEAT_SOURCE_PATCHED__ =
        !hasOptionalMainBundlePatchFailure("appStateHeartbeat");
      module._compile(source, filename);
      } catch (error) {
        recordCodeyPatchFailure("patch_codex_main_bundle", error, { filename });
        throw error;
      }
    };
  }

  const microStub = {
    __codexMicroDisabledLocal: true,
    ConnectionEventType: {
      CONNECTED: "CONNECTED",
      DISCONNECTED: "DISCONNECTED",
      ERROR: "ERROR",
    },
    DeviceType: { Project2077: "Project2077" },
    OAILightingEffect: { off: 0, breath: 1, solid: 2, snake: 3 },
    WLDeviceDiscovery: class NoCodexMicroDeviceDiscovery {
      findWLDevices() { return []; }
    },
    WLDeviceCommImpl: class NoCodexMicroDeviceComm {
      onConnectionEvent() { return () => {}; }
      async connect() {}
      async disconnect() {}
    },
    RPCApiOAI: class NoCodexMicroApi {
      onHidReceived() { return () => {}; }
      onJoystickMove() { return () => {}; }
      async sendLightingConfig() { return true; }
      async sendThreadsLighting() { return true; }
      async getDeviceStatus() { return {}; }
    },
  };

  let electronProxy = null;
  let electronProtocolProxy = null;
  Module._load = function codeyStartupPatchLoader(request, parent, isMain) {
    if (disableMicro && request === "@worklouder/device-kit-oai") return microStub;

    const loaded = Reflect.apply(originalLoad, this, arguments);
    if (request !== "electron" || !loaded?.BrowserWindow) return loaded;
    if (electronProxy) return electronProxy;

    if (loaded.protocol) {
      electronProtocolProxy = new Proxy(loaded.protocol, {
        get(target, property, receiver) {
          if (property === "handle") {
            return (scheme, handler) => {
              const effectiveHandler =
                scheme === "app" && typeof handler === "function"
                  ? async (request) =>
                      patchCodexRendererResponse(request, await handler(request))
                  : handler;
              return target.handle(scheme, effectiveHandler);
            };
          }
          const value = Reflect.get(target, property, receiver);
          return typeof value === "function" ? value.bind(target) : value;
        },
      });
    }
    electronProxy = new Proxy(loaded, {
      get(target, property, receiver) {
        if (property === "protocol" && electronProtocolProxy) return electronProtocolProxy;
        return Reflect.get(target, property, receiver);
      },
    });
    return electronProxy;
  };
  globalThis.__CODEY_CODEX_STARTUP_PATCH__ = Object.freeze({
    disableWindowsOptimizations,
    disableMicro,
    disablePet,
    fastCodexStartup,
    statsigBootstrapTimeoutMs,
    disableAppServerAnalytics: true,
    get disableDesktopCesAnalytics() {
      return !hasOptionalMainBundlePatchFailure("desktopCesAnalytics");
    },
    get appServerAnalyticsPatchCount() {
      return appServerAnalyticsPatchCount;
    },
    get throttleExternalPluginFocusReconcile() {
      return !hasOptionalMainBundlePatchFailure(
        "externalPluginFocusReconcile",
      );
    },
    get externalPluginFocusReconcileSuppressedCount() {
      return externalPluginFocusReconcileSuppressedCount;
    },
    get disableAppStateHeartbeat() {
      return !hasOptionalMainBundlePatchFailure("appStateHeartbeat");
    },
    get optionalMainBundlePatchFailures() {
      return optionalMainBundlePatchFailures.map((failure) => ({ ...failure }));
    },
    reclaimExecutionEnvironments: true,
    restoreNativeModelAndSpeedControls: true,
    destroyTemporaryWebViews: true,
    disableWindowsWmiSampler,
  });
  setImmediate(() => {
    try { process.getBuiltinModule("inspector").close(); } catch {}
  });
  return "codey-startup-patch-installed-v20";
})()
