(() => {
  "use strict";

  const guardKey = "__codeyWindowsWmiSamplerGuard";
  const scriptId = "windows-wmi-sampler";
  const version = 2;
  const statusRequestType = "codey-windows-wmi-sampler-status";
  const statusResponseType = "codey-windows-wmi-sampler-status-response";
  const probeTimeoutMs = 1_000;
  const observationWindowMs = 45_000;

  const existing = window[guardKey];
  if (existing && typeof existing.requestProbe === "function") {
    existing.requestProbe();
    return;
  }

  const platformText = [
    window.navigator?.userAgentData?.platform,
    window.navigator?.platform,
    window.navigator?.userAgent,
  ]
    .filter((value) => typeof value === "string")
    .join(" ");
  const enabled = /\bwin(?:32|64|dows)?\b/i.test(platformText);

  let mainProcessSnapshot = null;
  let probePending = null;
  let probeAttempts = 0;
  let probeError = "";
  let probeTransport = "";

  const snapshot = () => {
    const installed = enabled
      ? mainProcessSnapshot?.installed === true &&
        mainProcessSnapshot?.workerWrapperPatched === true
      : true;
    const blocked = Number(mainProcessSnapshot?.blocked) || 0;
    const selfTestPassed = mainProcessSnapshot?.selfTestPassed === true;
    const observationMs = Number(mainProcessSnapshot?.observationMs) || 0;
    const sourceReadFailures =
      Number(mainProcessSnapshot?.sourceReadFailures) || 0;
    return {
      version,
      enabled,
      installed,
      confirmed: !enabled || (installed && (selfTestPassed || blocked > 0)),
      blocked,
      selfTestPassed,
      observationMs,
      observationWindowMs,
      sourceInspections:
        Number(mainProcessSnapshot?.sourceInspections) || 0,
      sourceSignatureMisses:
        Number(mainProcessSnapshot?.sourceSignatureMisses) || 0,
      sourceReadFailures,
      probeAttempts,
      probeError,
      probeTransport,
      mainProcessSnapshot,
    };
  };

  const updateInjectionEntry = () => {
    const entry = window.__codeyInjectionStatus?.[scriptId];
    if (!entry) return;
    const current = snapshot();
    let status = "executed";
    let detail = "";
    let error = null;

    if (!current.enabled) {
      status = "effective";
      detail = "WMI 周期采样保护已就绪，当前平台无需启用";
    } else if (!current.installed && current.mainProcessSnapshot) {
      status = "failed";
      detail = "WMI 周期采样 Worker 拦截器未安装";
      error = detail;
    } else if (current.blocked > 0) {
      const matchReason = current.mainProcessSnapshot?.lastMatchReason;
      status = "effective";
      detail =
        `已阻止 ${current.blocked} 次 WMI 周期进程采样` +
        (matchReason === "source-signature"
          ? "（通过 Worker 源码特征识别）"
          : matchReason === "worker-option-name"
            ? "（通过 Worker 语义名称识别）"
            : "");
    } else if (current.selfTestPassed) {
      status = "effective";
      const workersObserved =
        Number(current.mainProcessSnapshot?.workersObserved) || 0;
      detail =
        "WMI 周期采样保护一次性自检通过" +
        (workersObserved > 0
          ? `；已观察 ${workersObserved} 个其他 Worker，实际目标采样尚未触发`
          : "；实际目标采样尚未触发");
    } else if (current.mainProcessSnapshot?.selfTestError) {
      status = "failed";
      detail = `WMI 周期采样保护自检失败：${current.mainProcessSnapshot.selfTestError}`;
      error = detail;
    } else if (current.sourceReadFailures > 0) {
      detail =
        `有 ${current.sourceReadFailures} 个 Worker 源码无法检查，` +
        "WMI 周期采样保护尚不能确认";
    } else if (
      current.installed &&
      current.observationMs >= observationWindowMs
    ) {
      detail = current.sourceInspections > 0
        ? `已检查 ${current.sourceInspections} 个 Worker，` +
          "尚未命中完整 WMI 周期采样特征；若 WMI 仍高占用，当前来源尚未被识别"
        : "WMI 周期采样保护已安装，但观察窗内未匹配到可识别的目标 Worker";
    } else if (current.installed) {
      detail =
        `WMI 周期采样保护已安装，等待首次采样确认` +
        `（已观察 ${Math.floor(current.observationMs / 1_000)} 秒）`;
    } else {
      detail = probeError
        ? `WMI 周期采样保护待确认：${probeError}`
        : "WMI 周期采样保护正在连接主进程确认";
    }

    const changed =
      entry.status !== status ||
      entry.detail !== detail ||
      entry.error !== error;
    entry.status = status;
    entry.detail = detail;
    entry.error = error;
    if (changed && typeof window.dispatchEvent === "function") {
      window.dispatchEvent(
        new CustomEvent("codey-injection-status-changed", {
          detail: { id: scriptId, status },
        }),
      );
    }
  };

  const requestProbe = () => {
    if (!enabled) {
      updateInjectionEntry();
      return null;
    }
    if (probePending) return probePending;
    const bridge = window.electronBridge;
    const sendStatusRequest = bridge?.sendMessageFromView;
    if (typeof sendStatusRequest !== "function") {
      probeError = "主进程状态通道不可用";
      updateInjectionEntry();
      return null;
    }

    probeAttempts += 1;
    const requestId =
      window.crypto?.randomUUID?.() ??
      `codey-wmi-guard-${Date.now()}-${probeAttempts}`;
    let responseListener = null;
    let responseTimer = 0;
    let finishResponseWait = null;
    const responseWait = new Promise((resolve) => {
      let settled = false;
      finishResponseWait = (value) => {
        if (settled) return;
        settled = true;
        if (responseListener) {
          window.removeEventListener?.("message", responseListener);
        }
        if (responseTimer) window.clearTimeout?.(responseTimer);
        resolve(value);
      };
      responseListener = (event) => {
        const message = event?.data;
        if (
          message?.type === statusResponseType &&
          message?.requestId === requestId
        ) {
          finishResponseWait(message);
        }
      };
      if (typeof window.addEventListener !== "function") {
        finishResponseWait(null);
        return;
      }
      window.addEventListener("message", responseListener);
      responseTimer = window.setTimeout?.(
        () => finishResponseWait(null),
        probeTimeoutMs,
      ) ?? 0;
    });
    const request = Promise.resolve()
      .then(() =>
        Reflect.apply(sendStatusRequest, bridge, [
          { type: statusRequestType, version, requestId },
        ]),
      )
      .then(async (directResult) => {
        const result = directResult === undefined
          ? await responseWait
          : directResult;
        if (result?.status !== "ok" || !result?.sampler) {
          probeError = "主进程未回传 WMI 保护状态";
          return;
        }
        probeTransport =
          directResult === undefined ? "renderer-event" : "invoke-return";
        mainProcessSnapshot = result.sampler;
        probeError = "";
      })
      .catch((error) => {
        probeError = String(
          error instanceof Error
            ? error.message
            : error || "主进程状态查询失败",
        ).slice(0, 160);
      })
      .finally(() => {
        finishResponseWait?.(null);
        if (probePending === request) probePending = null;
        updateInjectionEntry();
      });
    probePending = request;
    return request;
  };

  const api = Object.freeze({
    version,
    enabled,
    requestProbe,
    snapshot,
  });
  Object.defineProperty(window, guardKey, {
    configurable: false,
    value: api,
    writable: false,
  });

  requestProbe();
})();
