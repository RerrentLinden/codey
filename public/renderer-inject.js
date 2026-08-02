// Lightweight renderer bootstrap injected by the Codey CDP launcher.
// The heavier session/sidebar tools live in codey-inject.js and are loaded
// only after Codex's sidebar is present.
(() => {
  const rendererCoreAlreadyLoaded = window.__codeyRendererCoreLoaded === true;
  window.__codeyRendererCoreLoaded = true;
  window.__codeyRendererModuleReady = true;

  const sessionToolsLoadPath = "/internal/codey/session-tools/load";
  const updateCheckPath = "/api/check_for_updates";
  const accountUsagePath = "/account/usage";
  const buttonId = "codey-settings-button";
  const accountUsageId = "codey-account-usage";
  const styleId = "codey-core-injected-style";
  const updateAvailableEvent = "codey-update-availability-changed";
  const configChangedEvent = "codey:config-changed";
  const updateCheckIntervalMs = 30 * 60 * 1000;
  const updateCheckTimeoutMs = 12_000;
  const accountUsageRefreshIntervalMs = 60_000;
  const accountUsageTimeoutMs = 8_000;
  const sidebarSelector = [
    "[data-app-action-sidebar-section]",
    "[data-app-action-sidebar-thread-row]",
    "[data-app-action-sidebar-project-row]",
    "[data-app-action-sidebar-thread-id][data-app-action-sidebar-thread-title]",
  ].join(", ");
  const headerSelector = "header, nav";
  const bootstrapProbeSelector = `${headerSelector}, ${sidebarSelector}`;
  const settingsIcon = `
    <svg viewBox="0 0 350 350" aria-hidden="true" focusable="false">
      <rect x="0" y="0" width="350" height="350" rx="34" fill="#fff" stroke="none"></rect>
      <path d="M70 301c-16 0-24-18-13-30l73-77c8-8 8-20 0-28L65 101C50 86 57 61 78 57c9-2 18 1 25 8l91 91c18 18 18 46 0 64l-66 66c-6 6-2 15 7 15h183" fill="none" stroke="currentColor" stroke-width="22" stroke-linecap="round" stroke-linejoin="round"></path>
    </svg>
  `;
  const defaultChineseLocale = "zh-CN";
  const defaultChineseLanguages = [defaultChineseLocale, "zh", "en-US", "en"];
  const statsigI18nDynamicConfigId = "72216192";
  const localeReloadStorageKey = "codey.defaultChineseLocale.reload.v1";

  let sessionToolsLoadPromise = null;
  let scanTimer = 0;
  let updateCheckTimer = 0;
  let updateCheckInFlight = false;
  let accountUsageTimer = 0;
  let accountUsageCheckInFlight = false;
  let accountUsageLastResult = null;
  let accountUsageMountedHeader = null;
  let sessionToolsInteractionArmed = false;
  let bootstrapObserver = null;
  let headerMountDirty = true;

  const queryWithin = (root, selector) => {
    const matches = [];
    if (root instanceof HTMLElement && typeof root.matches === "function" && root.matches(selector)) {
      matches.push(root);
    }
    if (root && typeof root.querySelectorAll === "function") {
      matches.push(...root.querySelectorAll(selector));
    }
    return matches;
  };

  const callBridge = (path, payload = {}) => {
    if (typeof window.__codexSessionDeleteBridge === "function") {
      return window.__codexSessionDeleteBridge(path, payload);
    }
    return Promise.resolve({ status: "failed", message: "Codey bridge unavailable" });
  };

  const addStyle = () => {
    if (document.getElementById(styleId)) return;
    const style = document.createElement("style");
    style.id = styleId;
    style.textContent = `
      #${buttonId} { -webkit-app-region: no-drag !important; pointer-events: auto !important; position: relative; z-index: 2147483641; display: inline-grid; place-items: center; flex: 0 0 auto; width: 32px; height: 32px; border: 0; border-radius: 8px; padding: 0; margin-inline-start: 8px; margin-inline-end: 18px; background: transparent; color: inherit; cursor: pointer; opacity: .86; user-select: none; transition: background .15s ease, opacity .15s ease, transform .15s ease; }
      #${buttonId}[data-codey-header-actions="true"] { width: 28px; height: 28px; margin-inline-start: 0; margin-inline-end: 6px; }
      #${buttonId}:hover { background: rgba(127, 127, 127, .14); opacity: 1; }
      #${buttonId}:active { transform: translateY(1px); }
      #${buttonId}:focus-visible { outline: 2px solid rgba(139, 151, 255, .72); outline-offset: 2px; }
      #${buttonId} svg { display: block; width: 19px; height: 19px; fill: none; stroke: currentColor; stroke-width: 22; stroke-linecap: round; stroke-linejoin: round; }
      #${buttonId} .codey-settings-label { position: absolute; width: 1px; height: 1px; margin: -1px; padding: 0; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; border: 0; }
      #${buttonId}::after { content: ""; position: absolute; top: 5px; right: 5px; width: 7px; height: 7px; border-radius: 999px; background: #ff3b30; box-shadow: 0 0 0 2px Canvas; opacity: 0; transform: scale(.7); transition: opacity .15s ease, transform .15s ease; pointer-events: none; }
      #${buttonId}[data-codey-update-available="true"]::after { opacity: 1; transform: scale(1); }
      #${buttonId}[data-codey-header-actions="true"]::after { top: 4px; right: 4px; }
      #${accountUsageId} { -webkit-app-region: no-drag !important; pointer-events: none; position: relative; z-index: 2147483640; display: inline-flex; min-height: 40px; max-width: min(360px, 38vw); flex: 0 0 auto; align-items: stretch; overflow: hidden; border: 1px solid color-mix(in srgb, CanvasText 11%, transparent); border-radius: 9px; margin-inline: 4px 2px; background: color-mix(in srgb, Canvas 91%, transparent); box-shadow: 0 1px 3px color-mix(in srgb, CanvasText 8%, transparent), inset 0 1px 0 color-mix(in srgb, Canvas 72%, transparent); color: CanvasText; font-family: -apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif; font-size: 11px; line-height: 1.1; opacity: .94; backdrop-filter: blur(14px); -webkit-backdrop-filter: blur(14px); user-select: none; }
      #${accountUsageId}[data-state="stale"] { opacity: .68; }
      #${accountUsageId}[data-state="error"] { min-width: 118px; align-items: center; justify-content: center; padding: 0 10px; color: color-mix(in srgb, CanvasText 66%, transparent); }
      #${accountUsageId} .codey-usage-segment { display: grid; min-width: 104px; grid-template-columns: minmax(0, 1fr) auto; align-content: center; column-gap: 8px; padding: 5px 9px 4px; }
      #${accountUsageId} .codey-usage-segment + .codey-usage-segment { border-inline-start: 1px solid color-mix(in srgb, CanvasText 9%, transparent); }
      #${accountUsageId} .codey-usage-window { display: flex; min-width: 0; align-items: center; gap: 4px; overflow: hidden; color: color-mix(in srgb, CanvasText 62%, transparent); font-size: 9px; font-weight: 550; white-space: nowrap; }
      #${accountUsageId} .codey-usage-window-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
      #${accountUsageId} .codey-usage-plan { flex: 0 0 auto; border: 1px solid color-mix(in srgb, #0a84ff 24%, transparent); border-radius: 4px; padding: 1px 4px; background: color-mix(in srgb, #0a84ff 9%, transparent); color: color-mix(in srgb, #0a84ff 78%, CanvasText); font-size: 8px; font-weight: 700; letter-spacing: .01em; line-height: 1.15; }
      #${accountUsageId} .codey-usage-value { font-variant-numeric: tabular-nums; font-weight: 650; letter-spacing: -.01em; white-space: nowrap; }
      #${accountUsageId} .codey-usage-meter { grid-column: 1 / -1; height: 2px; margin-top: 4px; overflow: hidden; border-radius: 999px; background: color-mix(in srgb, CanvasText 10%, transparent); }
      #${accountUsageId} .codey-usage-meter > span { display: block; width: 100%; height: 100%; border-radius: inherit; background: #0a84ff; transform: scaleX(var(--codey-usage-remaining)); transform-origin: left center; }
      #${accountUsageId} .codey-usage-reset { grid-column: 1 / -1; overflow: hidden; margin-top: 3px; color: color-mix(in srgb, CanvasText 48%, transparent); font-size: 8px; font-variant-numeric: tabular-nums; text-overflow: ellipsis; white-space: nowrap; }
      #${accountUsageId} .codey-usage-segment[data-tone="healthy"] .codey-usage-meter > span { background: #30d158; }
      #${accountUsageId} .codey-usage-segment[data-tone="warning"] .codey-usage-meter > span { background: #ffd60a; }
      #${accountUsageId} .codey-usage-segment[data-tone="critical"] .codey-usage-meter > span { background: #ff453a; }
      @media (max-width: 860px) {
        #${accountUsageId} { max-width: 34vw; }
        #${accountUsageId} .codey-usage-segment { min-width: 82px; padding-inline: 7px; }
        #${accountUsageId} .codey-usage-window { display: none; }
        #${accountUsageId} .codey-usage-value { grid-column: 1 / -1; text-align: center; }
        #${accountUsageId} .codey-usage-reset { text-align: center; }
      }
      @media (max-width: 680px) {
        #${accountUsageId} { display: none; }
      }
      @media (prefers-reduced-motion: reduce) {
        #${accountUsageId}, #${accountUsageId} * { transition: none !important; }
      }
    `;
    document.documentElement.appendChild(style);
  };

  const hasDetectedUpdate = () =>
    window.__codeyUpdateAvailability?.updateAvailable === true;

  const dispatchUpdateAvailability = () => {
    if (
      typeof window.dispatchEvent !== "function"
      || typeof CustomEvent !== "function"
    ) return;
    window.dispatchEvent(new CustomEvent(updateAvailableEvent, {
      detail: hasDetectedUpdate() ? window.__codeyUpdateAvailability : null,
    }));
  };

  const applyUpdateBadge = (button = document.getElementById(buttonId)) => {
    if (!(button instanceof HTMLElement)) return;
    if (hasDetectedUpdate()) {
      button.setAttribute("data-codey-update-available", "true");
      button.setAttribute("aria-label", "打开 Codey 配置，有可用更新");
      button.title = "打开 Codey 配置（发现新版本）";
      return;
    }
    button.removeAttribute?.("data-codey-update-available");
    button.setAttribute("aria-label", "打开 Codey 配置");
    button.title = "打开 Codey 配置";
  };

  const setUpdateAvailability = (result, { dispatch = true } = {}) => {
    window.__codeyUpdateAvailability = result?.updateAvailable === true
      ? result
      : null;
    applyUpdateBadge();
    if (hasDetectedUpdate()) {
      window.clearTimeout(updateCheckTimer);
      updateCheckTimer = 0;
    }
    if (dispatch) dispatchUpdateAvailability();
  };

  const withTimeout = (promise, timeoutMs) => new Promise((resolve, reject) => {
    const timer = window.setTimeout(
      () => reject(new Error("检查更新超时")),
      timeoutMs,
    );
    Promise.resolve(promise).then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        window.clearTimeout(timer);
        reject(error);
      },
    );
  });

  const scheduleUpdateCheck = (delayMs = updateCheckIntervalMs) => {
    if (hasDetectedUpdate()) return;
    window.clearTimeout(updateCheckTimer);
    updateCheckTimer = window.setTimeout(() => {
      updateCheckTimer = 0;
      void checkForUpdatesSilently();
    }, delayMs);
  };

  const checkForUpdatesSilently = async () => {
    if (updateCheckInFlight || hasDetectedUpdate()) return;
    updateCheckInFlight = true;
    try {
      const result = await withTimeout(
        callBridge(updateCheckPath, {}),
        updateCheckTimeoutMs,
      );
      if (result?.status !== "failed" && result?.updateAvailable === true) {
        setUpdateAvailability(result);
        return;
      }
    } catch {
      // 后台更新检测保持静默。
    } finally {
      updateCheckInFlight = false;
      if (!hasDetectedUpdate()) scheduleUpdateCheck();
    }
  };

  const accountUsageWindowLabel = (minutes) => {
    const value = Math.max(1, Math.round(Number(minutes) || 0));
    if (value % (24 * 60) === 0) return `${value / (24 * 60)} 天`;
    if (value % 60 === 0) return `${value / 60} 小时`;
    return `${value} 分钟`;
  };

  const accountUsageResetLabel = (resetsAt) => {
    const timestamp = Number(resetsAt);
    if (!Number.isFinite(timestamp) || timestamp <= 0) return "";
    const remainingMinutes = Math.max(
      0,
      Math.ceil((timestamp * 1000 - Date.now()) / 60_000),
    );
    if (remainingMinutes < 60) return `${remainingMinutes} 分钟后重置`;
    if (remainingMinutes < 24 * 60) {
      const hours = Math.floor(remainingMinutes / 60);
      const minutes = remainingMinutes % 60;
      return minutes ? `${hours} 小时 ${minutes} 分钟后重置` : `${hours} 小时后重置`;
    }
    const days = Math.floor(remainingMinutes / (24 * 60));
    const hours = Math.floor((remainingMinutes % (24 * 60)) / 60);
    return hours ? `${days} 天 ${hours} 小时后重置` : `${days} 天后重置`;
  };

  const accountUsageResetTimeLabel = (resetsAt) => {
    const timestamp = Number(resetsAt);
    if (!Number.isFinite(timestamp) || timestamp <= 0) return "";
    const resetAt = new Date(timestamp * 1000);
    if (Number.isNaN(resetAt.getTime())) return "";
    const now = new Date();
    const startOfToday = new Date(
      now.getFullYear(),
      now.getMonth(),
      now.getDate(),
    ).getTime();
    const startOfResetDay = new Date(
      resetAt.getFullYear(),
      resetAt.getMonth(),
      resetAt.getDate(),
    ).getTime();
    const dayOffset = Math.round(
      (startOfResetDay - startOfToday) / (24 * 60 * 60 * 1000),
    );
    const time = resetAt.toLocaleTimeString("zh-CN", {
      hour: "2-digit",
      minute: "2-digit",
      hour12: false,
    });
    if (dayOffset === 0) return `今天 ${time} 刷新`;
    if (dayOffset === 1) return `明天 ${time} 刷新`;
    return `${resetAt.getMonth() + 1}月${resetAt.getDate()}日 ${time} 刷新`;
  };

  const accountUsagePlan = (planType) => {
    const raw = String(planType || "").trim();
    if (!raw) return null;
    const compact = raw.toLowerCase().replace(/[\s_$-]+/g, "");
    if (
      compact === "5x"
      || compact.includes("pro5x")
      || compact.includes("pro100")
    ) {
      return { key: "pro-5x", label: "Pro 5x" };
    }
    if (
      compact === "pro"
      || compact.includes("pro20x")
      || compact.includes("pro200")
    ) {
      return { key: "pro-20x", label: "Pro 20x" };
    }
    if (compact.includes("plus")) return { key: "plus", label: "Plus" };
    if (compact.includes("free")) return { key: "free", label: "Free" };
    return {
      key: "other",
      label: raw
        .replace(/[_-]+/g, " ")
        .replace(/\b\w/g, (character) => character.toUpperCase()),
    };
  };

  const escapeAccountUsageText = (value) => String(value ?? "")
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");

  const accountUsagePlanMarkup = (plan) => plan
    ? `<span class="codey-usage-plan" data-plan="${plan.key}">${escapeAccountUsageText(plan.label)}</span>`
    : "";

  const accountUsageWindowSegment = (window, plan = null) => {
    if (!window || !Number.isFinite(Number(window.usedPercent))) return null;
    const remaining = Math.max(0, Math.min(100, 100 - Number(window.usedPercent)));
    const roundedRemaining = Math.round(remaining);
    const label = accountUsageWindowLabel(window.windowMinutes);
    const reset = accountUsageResetLabel(window.resetsAt);
    const resetTime = accountUsageResetTimeLabel(window.resetsAt);
    const tone = roundedRemaining <= 20
      ? "critical"
      : roundedRemaining <= 40
        ? "warning"
        : roundedRemaining <= 70
          ? "normal"
          : "healthy";
    return {
      aria: `${label}额度剩余 ${roundedRemaining}%${reset ? `，${reset}` : ""}`,
      html: `
        <span class="codey-usage-segment" data-tone="${tone}" style="--codey-usage-remaining:${remaining / 100}">
          <span class="codey-usage-window">
            ${accountUsagePlanMarkup(plan)}
            <span class="codey-usage-window-label">${label}</span>
          </span>
          <span class="codey-usage-value">${roundedRemaining}%</span>
          <span class="codey-usage-meter"><span></span></span>
          ${resetTime ? `<span class="codey-usage-reset">${resetTime}</span>` : ""}
        </span>
      `,
    };
  };

  const accountCreditsSegment = (credits, plan = null) => {
    if (!credits || (!credits.hasCredits && !credits.unlimited)) return null;
    const balance = credits.unlimited ? "不限" : String(credits.balance || "0");
    return {
      aria: `账号额度余额 ${balance}`,
      html: `
        <span class="codey-usage-segment" style="--codey-usage-remaining:1">
          <span class="codey-usage-window">
            ${accountUsagePlanMarkup(plan)}
            <span class="codey-usage-window-label">余额</span>
          </span>
          <span class="codey-usage-value">${escapeAccountUsageText(balance)}</span>
          <span class="codey-usage-meter"><span></span></span>
        </span>
      `,
    };
  };

  const removeAccountUsage = () => {
    document.getElementById(accountUsageId)?.remove?.();
    accountUsageMountedHeader?.removeAttribute?.("data-codey-usage-host");
    accountUsageMountedHeader = null;
  };

  const accountUsageMount = () => {
    const header = findHeaderMount()?.header;
    if (!(header instanceof HTMLElement)) return null;
    let usage = document.getElementById(accountUsageId);
    if (!(usage instanceof HTMLElement)) {
      usage = document.createElement("div");
      usage.id = accountUsageId;
      usage.setAttribute("role", "status");
      usage.setAttribute("aria-live", "polite");
      usage.setAttribute("aria-atomic", "true");
    }
    if (accountUsageMountedHeader && accountUsageMountedHeader !== header) {
      accountUsageMountedHeader.removeAttribute?.("data-codey-usage-host");
    }
    header.setAttribute("data-codey-usage-host", "true");
    const settingsButton = document.getElementById(buttonId);
    if (
      settingsButton instanceof HTMLElement
      && settingsButton.parentElement === header
    ) {
      if (
        usage.parentElement !== header
        || usage.nextElementSibling !== settingsButton
      ) {
        header.insertBefore(usage, settingsButton);
      }
    } else if (usage.parentElement !== header) {
      header.appendChild(usage);
    }
    accountUsageMountedHeader = header;
    return usage;
  };

  const renderAccountUsage = (result) => {
    if (!result || result.status === "disabled" || result.status === "unavailable") {
      accountUsageLastResult = null;
      removeAccountUsage();
      return;
    }
    if (result.status === "error") {
      const usage = accountUsageMount();
      if (!usage) return;
      if (accountUsageLastResult?.status === "ok") {
        usage.dataset.state = "stale";
        usage.title = "官方账号额度暂时无法更新，当前显示上次获取结果";
        return;
      }
      usage.dataset.state = "error";
      usage.setAttribute("aria-label", "官方账号额度暂不可用");
      usage.title = String(result.message || "官方账号额度暂不可用");
      usage.textContent = "额度暂不可用";
      return;
    }
    if (result.status !== "ok") return;

    const plan = accountUsagePlan(result.planType);
    const primary = accountUsageWindowSegment(result.primary, plan);
    const secondary = accountUsageWindowSegment(
      result.secondary,
      primary ? null : plan,
    );
    const credits = accountCreditsSegment(
      result.credits,
      primary || secondary ? null : plan,
    );
    const segments = [primary, secondary, credits].filter(Boolean);
    if (!segments.length) {
      renderAccountUsage({
        status: "error",
        message: "官方账号额度响应中没有可展示的信息",
      });
      return;
    }
    accountUsageLastResult = result;
    const usage = accountUsageMount();
    if (!usage) return;
    const aria = [
      plan ? `当前套餐 ${plan.label}` : null,
      ...segments.map((segment) => segment.aria),
    ].filter(Boolean).join("；");
    usage.dataset.state = "ready";
    if (plan) usage.dataset.plan = plan.key;
    else delete usage.dataset.plan;
    usage.setAttribute("aria-label", aria);
    usage.title = aria;
    const nextHtml = segments.map((segment) => segment.html).join("");
    // 额度未变化时跳过重建，避免每 60 秒的轮询都触发 DOM 重排和 aria-live
    // 重复播报。
    if (usage.__codeyLastUsageHtml !== nextHtml) {
      usage.__codeyLastUsageHtml = nextHtml;
      usage.innerHTML = nextHtml;
    }
  };

  const scheduleAccountUsageCheck = (delayMs = accountUsageRefreshIntervalMs) => {
    window.clearTimeout(accountUsageTimer);
    accountUsageTimer = window.setTimeout(() => {
      accountUsageTimer = 0;
      void checkAccountUsage();
    }, delayMs);
  };

  const checkAccountUsage = async () => {
    if (accountUsageCheckInFlight || document.visibilityState === "hidden") {
      scheduleAccountUsageCheck();
      return null;
    }
    accountUsageCheckInFlight = true;
    try {
      const result = await withTimeout(
        callBridge(accountUsagePath, {}),
        accountUsageTimeoutMs,
      );
      renderAccountUsage(result);
      return result;
    } catch (error) {
      renderAccountUsage({
        status: "error",
        message: error instanceof Error ? error.message : String(error),
      });
      return null;
    } finally {
      accountUsageCheckInFlight = false;
      scheduleAccountUsageCheck();
    }
  };

  const syncAccountUsageMount = () => {
    if (accountUsageLastResult?.status === "ok") {
      renderAccountUsage(accountUsageLastResult);
    }
  };

  const openSettings = () => {
    if (window.__codeySettingsOverlay?.toggle) {
      window.__codeySettingsOverlay.toggle();
      return;
    }
    const detail = String(window.__codeyOverlayError || "").split("\n")[0];
    window.alert(detail
      ? `Codey 内嵌配置面板加载失败：${detail}`
      : "Codey 内嵌配置面板尚未加载，请退出 Codex 后重新启动 Codey");
  };

  const installDefaultChineseLocale = () => {
    const existing = window.__codeyDefaultChineseLocale;
    if (existing?.version === 4 && existing.locale === defaultChineseLocale) {
      existing.ensureSynced?.();
      return;
    }

    const state = {
      version: 4,
      locale: defaultChineseLocale,
      navigatorPatched: false,
      statsigClientsPatched: 0,
      statsigRootPatched: false,
      settingSyncStarted: false,
      settingSynced: false,
      settingSyncInFlight: false,
      settingSyncAttempts: 0,
      settingSyncError: null,
      ensureSynced: null,
      snapshot() {
        return {
          version: this.version,
          locale: this.locale,
          rendererAssetPatched:
            globalThis.__CODEY_DEFAULT_CHINESE_LOCALE_RENDERER_PATCH__ === true,
          navigatorPatched: this.navigatorPatched,
          statsigClientsPatched: this.statsigClientsPatched,
          statsigRootPatched: this.statsigRootPatched,
          settingSyncStarted: this.settingSyncStarted,
          settingSynced: this.settingSynced,
          settingSyncInFlight: this.settingSyncInFlight,
          settingSyncAttempts: this.settingSyncAttempts,
          settingSyncError: this.settingSyncError,
        };
      },
    };
    window.__codeyDefaultChineseLocale = state;

    const defineNavigatorGetter = (target, name, value) => {
      if (!target || (typeof target !== "object" && typeof target !== "function")) return false;
      try {
        Object.defineProperty(target, name, {
          configurable: true,
          get: () => value,
        });
        return true;
      } catch {
        return false;
      }
    };

    const patchNavigatorLocale = () => {
      const navigatorTargets = [];
      try {
        if (typeof Navigator === "function" && Navigator.prototype) {
          navigatorTargets.push(Navigator.prototype);
        }
      } catch {
      }
      try {
        if (window.navigator) navigatorTargets.push(window.navigator);
      } catch {
      }
      state.navigatorPatched = navigatorTargets
        .some((target) => (
          defineNavigatorGetter(target, "language", defaultChineseLocale)
          && defineNavigatorGetter(target, "languages", defaultChineseLanguages)
        ));
    };

    const patchDynamicConfig = (dynamicConfig) => {
      if (!dynamicConfig || typeof dynamicConfig !== "object") return dynamicConfig;
      const value = dynamicConfig.value && typeof dynamicConfig.value === "object"
        ? dynamicConfig.value
        : {};
      try {
        dynamicConfig.value = {
          ...value,
          enable_i18n: true,
          locale_source: "SYSTEM",
        };
      } catch {
      }
      if (typeof dynamicConfig.get === "function" && !dynamicConfig.__codeyDefaultChineseLocaleGetPatched) {
        const originalGet = dynamicConfig.get.bind(dynamicConfig);
        dynamicConfig.get = (key, fallback) => {
          if (key === "enable_i18n") return true;
          if (key === "locale_source") return "SYSTEM";
          return originalGet(key, fallback);
        };
        dynamicConfig.__codeyDefaultChineseLocaleGetPatched = true;
      }
      return dynamicConfig;
    };

    const statsigClients = () => {
      const clients = [];
      for (const root of [window.__STATSIG__, globalThis.__STATSIG__]) {
        if (!root || typeof root !== "object") continue;
        try {
          clients.push(root.firstInstance);
        } catch {
        }
        try {
          if (typeof root.instance === "function") clients.push(root.instance());
        } catch {
        }
        try {
          if (root.instances && typeof root.instances === "object") {
            clients.push(...Object.values(root.instances));
          }
        } catch {
        }
      }
      return clients.filter(
        (client, index, array) =>
          client && typeof client === "object" && array.indexOf(client) === index,
      );
    };

    const patchStatsigClient = (client) => {
      if (!client || typeof client !== "object") return;
      if (typeof client.getDynamicConfig !== "function") return;
      if (!client.__codeyDefaultChineseLocalePatched) {
        const originalGetDynamicConfig = client.getDynamicConfig.bind(client);
        try {
          client.getDynamicConfig = (name, options) => {
            const result = originalGetDynamicConfig(name, options);
            return name === statsigI18nDynamicConfigId ? patchDynamicConfig(result) : result;
          };
          client.__codeyDefaultChineseLocalePatched = true;
          state.statsigClientsPatched += 1;
        } catch {
        }
      }
      try {
        patchDynamicConfig(client.getDynamicConfig(statsigI18nDynamicConfigId, {
          disableExposureLog: true,
        }));
      } catch {
      }
    };

    const patchStatsigRoot = (root) => {
      if (!root || typeof root !== "object") return;
      if (root.__codeyDefaultChineseLocaleRootPatched) return;
      root.__codeyDefaultChineseLocaleRootPatched = true;
      state.statsigRootPatched = true;
      for (const key of ["firstInstance", "instance"]) {
        let current;
        try {
          current = root[key];
        } catch {
          continue;
        }
        patchStatsigClient(typeof current === "function" && key === "instance" ? current.call(root) : current);
        try {
          Object.defineProperty(root, key, {
            configurable: true,
            get: () => current,
            set: (next) => {
              current = next;
              patchStatsigClient(typeof next === "function" && key === "instance" ? next.call(root) : next);
            },
          });
        } catch {
        }
      }
    };

    const installStatsigRootSetter = () => {
      let descriptor;
      try {
        descriptor = Object.getOwnPropertyDescriptor(window, "__STATSIG__");
      } catch {
        descriptor = null;
      }
      if (descriptor && descriptor.configurable === false) {
        patchStatsigRoot(window.__STATSIG__);
        return;
      }
      let currentRoot = window.__STATSIG__;
      patchStatsigRoot(currentRoot);
      try {
        Object.defineProperty(window, "__STATSIG__", {
          configurable: true,
          get: () => currentRoot,
          set: (next) => {
            currentRoot = next;
            patchStatsigRoot(next);
            patchStatsigClients();
          },
        });
      } catch {
      }
    };

    const patchStatsigClients = () => {
      installStatsigRootSetter();
      patchStatsigRoot(window.__STATSIG__ || globalThis.__STATSIG__);
      for (const client of statsigClients()) patchStatsigClient(client);
    };

    const waitForElectronBridge = () => new Promise((resolve) => {
      if (typeof window.setTimeout !== "function") {
        resolve(null);
        return;
      }
      const startedAt = Date.now();
      const check = () => {
        const bridge = window.electronBridge;
        if (bridge && typeof bridge.sendMessageFromView === "function") {
          resolve(bridge);
          return;
        }
        if (Date.now() - startedAt >= 5000) {
          resolve(null);
          return;
        }
        window.setTimeout(check, 50);
      };
      check();
    });

    const callCodexSettingApi = (bridge, method, params) => new Promise((resolve, reject) => {
      const requestId = globalThis.crypto && typeof globalThis.crypto.randomUUID === "function"
        ? globalThis.crypto.randomUUID()
        : `codey-locale-${Date.now()}-${Math.random().toString(16).slice(2)}`;
      let timeout = 0;
      const cleanup = () => {
        window.clearTimeout?.(timeout);
        window.removeEventListener?.("message", onMessage);
      };
      const onMessage = (event) => {
        const message = event?.data;
        if (!message || message.type !== "fetch-response" || message.requestId !== requestId) return;
        cleanup();
        if (message.responseType !== "success") {
          reject(new Error(message.error || `Codex ${method} failed`));
          return;
        }
        try {
          resolve(JSON.parse(message.bodyJsonString || "null"));
        } catch (error) {
          reject(error);
        }
      };
      window.addEventListener?.("message", onMessage);
      timeout = window.setTimeout?.(() => {
        cleanup();
        reject(new Error(`Codex ${method} timed out`));
      }, 5000);
      const message = {
        type: "fetch",
        requestId,
        method: "POST",
        url: `vscode://codex/${method}`,
        body: JSON.stringify(params),
      };
      Promise.resolve(bridge.sendMessageFromView(message)).catch((error) => {
        cleanup();
        reject(error);
      });
    });

    const reloadAfterLocaleChange = () => {
      try {
        if (window.sessionStorage?.getItem(localeReloadStorageKey) === defaultChineseLocale) {
          return;
        }
        window.sessionStorage?.setItem(localeReloadStorageKey, defaultChineseLocale);
      } catch {
      }
      window.location?.reload?.();
    };

    const clearLocaleReloadMarker = () => {
      try {
        window.sessionStorage?.removeItem(localeReloadStorageKey);
      } catch {
      }
    };

    const syncCodexLocaleSettingOnce = async () => {
      state.settingSyncStarted = true;
      const bridge = await waitForElectronBridge();
      if (!bridge) throw new Error("Codex Electron bridge unavailable");
      const response = await callCodexSettingApi(bridge, "get-setting", { key: "localeOverride" });
      if (response?.value === defaultChineseLocale) {
        state.settingSynced = true;
        state.settingSyncError = null;
        clearLocaleReloadMarker();
        return;
      }
      await callCodexSettingApi(bridge, "set-setting", {
        key: "localeOverride",
        value: defaultChineseLocale,
      });
      const verification = await callCodexSettingApi(
        bridge,
        "get-setting",
        { key: "localeOverride" },
      );
      if (verification?.value !== defaultChineseLocale) {
        throw new Error("Codex localeOverride was not persisted");
      }
      state.settingSynced = true;
      state.settingSyncError = null;
      reloadAfterLocaleChange();
    };

    const ensureCodexLocaleSetting = () => {
      if (state.settingSynced || state.settingSyncInFlight) return;
      state.settingSyncInFlight = true;
      void (async () => {
        const retryDelays = [0, 250, 750, 1500, 3000, 5000];
        for (const delay of retryDelays) {
          if (delay > 0) {
            await new Promise((resolve) => {
              if (typeof window.setTimeout === "function") {
                window.setTimeout(resolve, delay);
              } else {
                resolve();
              }
            });
          }
          state.settingSyncAttempts += 1;
          try {
            await syncCodexLocaleSettingOnce();
            return;
          } catch (error) {
            state.settingSyncError = error instanceof Error ? error.message : String(error);
          }
        }
        console.warn(
          "[Codey] Codex 中文语言设置同步失败，将在窗口重新聚焦时重试",
          state.settingSyncError,
        );
      })().finally(() => {
        state.settingSyncInFlight = false;
      });
    };
    state.ensureSynced = ensureCodexLocaleSetting;

    patchNavigatorLocale();
    patchStatsigClients();
    ensureCodexLocaleSetting();
    window.addEventListener?.("focus", ensureCodexLocaleSetting);
    window.addEventListener?.("pageshow", ensureCodexLocaleSetting);

    const startedAt = Date.now();
    const scanStatsigUntilReady = () => {
      patchStatsigClients();
      const elapsed = Date.now() - startedAt;
      if (elapsed >= 15000) return;
      window.setTimeout?.(scanStatsigUntilReady, elapsed < 1000 ? 50 : 250);
    };
    window.setTimeout?.(scanStatsigUntilReady, 50);
  };

  const visibleMountRect = (element) => {
    if (!(element instanceof HTMLElement)) return null;
    if (element.closest("[hidden], [aria-hidden=true]")) return null;
    const style = window.getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return style.display !== "none"
      && style.visibility !== "hidden"
      && rect.width > 0
      && rect.height > 0
      ? rect
      : null;
  };

  const isTopChromeMountTarget = (element) => {
    const rect = visibleMountRect(element);
    if (!rect) return false;
    const viewportWidth = Math.max(
      window.innerWidth || 0,
      document.documentElement?.clientWidth || 0,
      document.documentElement?.getBoundingClientRect?.().width || 0,
      rect.right,
    );
    return rect.top <= 96
      && rect.height <= 120
      && rect.width >= 48
      && rect.right >= viewportWidth - 48;
  };

  const findHeaderMount = () => {
    const header = [...document.querySelectorAll("header")].find(isTopChromeMountTarget)
      || [...document.querySelectorAll("nav")].find(isTopChromeMountTarget);
    if (!header) return null;

    const rightmostControl = [...header.querySelectorAll("button, [role=button], a[href]")]
      .reduce((rightmost, control) => {
        if (control.id === buttonId) return rightmost;
        const rect = visibleMountRect(control);
        if (!rect || (rightmost && rect.right <= rightmost.right)) return rightmost;
        return { control, right: rect.right };
      }, null)?.control || null;
    if (!rightmostControl) return { header, target: header };

    let headerChild = rightmostControl;
    while (headerChild.parentElement && headerChild.parentElement !== header) {
      headerChild = headerChild.parentElement;
    }
    const headerRect = header.getBoundingClientRect();
    const childRect = headerChild.getBoundingClientRect();
    const hasTrailingActionRegion = headerChild !== rightmostControl
      && childRect.width <= 240
      && childRect.right >= headerRect.right - 24;
    return {
      header,
      target: header,
      before: hasTrailingActionRegion ? headerChild : null,
    };
  };

  const mountedButtonIsUsable = (button) => {
    if (headerMountDirty || !(button instanceof HTMLElement) || button.isConnected !== true) {
      return false;
    }
    const parent = button.parentElement;
    if (!(parent instanceof HTMLElement) || button.closest("[hidden], [aria-hidden=true]")) {
      return false;
    }
    const validParent = parent.matches?.(headerSelector);
    const anchored = button.dataset.codeyHeaderActions !== "true"
      || (
        !!button.nextElementSibling
        && button.nextElementSibling === button.__codeyHeaderAnchor
      );
    return !!validParent && anchored;
  };

  const mountButton = () => {
    addStyle();
    const existingButton = document.getElementById(buttonId);
    if (mountedButtonIsUsable(existingButton)) return;
    const mount = findHeaderMount();
    if (!mount) {
      existingButton?.remove?.();
      return;
    }
    let button = existingButton;
    if (!button) {
      button = document.createElement("button");
      button.id = buttonId;
      button.type = "button";
      button.setAttribute("aria-label", "打开 Codey 配置");
      button.innerHTML = `${settingsIcon}<span class="codey-settings-label">Codey</span>`;
      button.title = "打开 Codey 配置";
      button.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        openSettings();
      }, true);
    }
    if (mount.before) {
      button.dataset.codeyHeaderActions = "true";
    } else {
      delete button.dataset.codeyHeaderActions;
    }
    if (mount.before) {
      if (button.parentElement !== mount.target || button.nextElementSibling !== mount.before) {
        mount.target.insertBefore(button, mount.before);
      }
    } else if (button.parentElement !== mount.target) {
      mount.target.appendChild(button);
    }
    button.__codeyHeaderAnchor = mount.before || null;
    applyUpdateBadge(button);
    headerMountDirty = false;
  };

  const sidebarDetected = (root = document) => queryWithin(root, sidebarSelector).length > 0;

  const loadSessionTools = () => {
    if (window.__codeySessionToolsInjectLoaded === true) return Promise.resolve(true);
    if (sessionToolsLoadPromise) return sessionToolsLoadPromise;
    sessionToolsLoadPromise = Promise.resolve(callBridge(sessionToolsLoadPath, {}))
      .then((result) => {
        if (!result || result.status !== "ok") {
          throw new Error(result?.message || "会话工具加载请求失败");
        }
        if (window.__codeySessionToolsInjectLoaded !== true) {
          throw new Error(window.__codeySessionToolsError || "会话工具未完成初始化");
        }
        disarmSessionToolsInteraction();
        bootstrapObserver?.disconnect();
        bootstrapObserver = null;
        return true;
      })
      .catch((error) => {
        sessionToolsLoadPromise = null;
        console.warn("[Codey] session tools lazy load failed", error);
        return false;
      });
    return sessionToolsLoadPromise;
  };

  const loadSessionToolsFromInteraction = (event) => {
    const target = event?.target instanceof Element
      ? event.target
      : event?.target?.parentElement;
    if (!target?.closest?.(sidebarSelector)) return;
    void loadSessionTools();
  };

  const armSessionToolsInteraction = () => {
    if (
      sessionToolsInteractionArmed
      || sessionToolsLoadPromise
      || window.__codeySessionToolsInjectLoaded === true
    ) return;
    sessionToolsInteractionArmed = true;
    document.addEventListener("pointerover", loadSessionToolsFromInteraction, {
      capture: true,
      passive: true,
    });
    document.addEventListener("pointerdown", loadSessionToolsFromInteraction, {
      capture: true,
      passive: true,
    });
    document.addEventListener("focusin", loadSessionToolsFromInteraction, true);
  };

  const disarmSessionToolsInteraction = () => {
    if (!sessionToolsInteractionArmed) return;
    sessionToolsInteractionArmed = false;
    document.removeEventListener("pointerover", loadSessionToolsFromInteraction, true);
    document.removeEventListener("pointerdown", loadSessionToolsFromInteraction, true);
    document.removeEventListener("focusin", loadSessionToolsFromInteraction, true);
  };

  const scan = (root = document) => {
    mountButton();
    syncAccountUsageMount();
    if (sidebarDetected(root)) armSessionToolsInteraction();
  };

  const scheduleScan = (root = document) => {
    window.clearTimeout(scanTimer);
    scanTimer = window.setTimeout(() => {
      scanTimer = 0;
      scan(root);
    }, 60);
  };

  const invalidateHeaderMount = (root = document) => {
    headerMountDirty = true;
    scheduleScan(root || document);
  };

  installDefaultChineseLocale();
  if (rendererCoreAlreadyLoaded) return;
  window.addEventListener?.(updateAvailableEvent, (event) => {
    const result = "detail" in event
      ? event.detail
      : window.__codeyUpdateAvailability;
    setUpdateAvailability(result, { dispatch: false });
    if (!hasDetectedUpdate()) scheduleUpdateCheck();
  });
  window.addEventListener?.(configChangedEvent, () => {
    scheduleAccountUsageCheck(0);
  });
  scan();
  scheduleUpdateCheck(0);
  scheduleAccountUsageCheck(250);

  bootstrapObserver = new MutationObserver((mutations) => {
    for (const mutation of mutations) {
      const target = mutation.target instanceof HTMLElement
        ? mutation.target
        : mutation.target?.parentElement;
      if (
        target?.id === accountUsageId
        || target?.closest?.(`#${accountUsageId}`)
      ) {
        continue;
      }
      if (mutation.type === "attributes") {
        if (target?.matches?.(headerSelector) || target?.matches?.(sidebarSelector)) {
          if (target.matches?.(headerSelector)) headerMountDirty = true;
          scheduleScan(target);
          return;
        }
        continue;
      }
      const targetHeader = target?.matches?.(headerSelector)
        ? target
        : target?.closest?.(headerSelector);
      const headerChildrenChanged = targetHeader && [
        ...(mutation.addedNodes || []),
        ...(mutation.removedNodes || []),
      ].some((node) => (
        node instanceof HTMLElement
        && node.id !== buttonId
        && node.id !== accountUsageId
      ));
      if (headerChildrenChanged) {
        headerMountDirty = true;
        scheduleScan(targetHeader);
        return;
      }
      for (const node of mutation.addedNodes || []) {
        const element = node instanceof HTMLElement ? node : null;
        if (!element) continue;
        // One combined probe rejects the overwhelmingly common streaming case
        // in two subtree walks instead of four.
        const matched = element.matches?.(bootstrapProbeSelector)
          ? element
          : element.querySelector?.(bootstrapProbeSelector);
        if (!matched) continue;
        if (element.matches?.(headerSelector) || element.querySelector?.(headerSelector)) {
          headerMountDirty = true;
        }
        scheduleScan(element);
        return;
      }
    }
  });
  bootstrapObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: [
      "data-app-action-sidebar-section",
      "data-app-action-sidebar-thread-id",
      "data-app-action-sidebar-thread-title",
      "data-app-action-sidebar-project-id",
      "data-app-action-sidebar-project-row",
      "hidden",
      "aria-hidden",
    ],
    childList: true,
    subtree: true,
  });

  window.__codeyLoadSessionTools = loadSessionTools;
  window.__codeyRendererScan = scan;
  window.__codeyRendererInvalidateHeaderMount = invalidateHeaderMount;
  window.__codeyRefreshAccountUsage = checkAccountUsage;

  window.addEventListener?.("focus", () => {
    scan();
    scheduleAccountUsageCheck(0);
  });
  window.addEventListener?.("pageshow", () => scan());
})();
