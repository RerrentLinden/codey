import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  IconCheck,
  IconDeviceFloppy as Save,
  IconGitBranch as GitBranch,
  IconLoader2 as LoaderCircle,
  IconRefresh as RefreshCw,
  IconX,
} from "@tabler/icons-react";
import SemiModal from "@douyinfe/semi-ui/lib/es/modal";
import { invoke } from "./api";
import {
  formatBytes,
  TraceLogModule,
  type CrashpadPendingStats,
  type TraceLogStats,
} from "./TraceLogModule";
import {
  ModelPickerDialog,
} from "./AppDialogs";
import {
  AppUpdateCard,
  FeaturePolicyCard,
  ModelSection,
  OperationsPanel,
} from "./AppSections";
import { NotificationChannelsCard } from "./notifications";
import type { NotificationChannel } from "./notifications";
import { errorText, withTimeout } from "./appUtils";
import { useModelSelection } from "./useModelSelection";
import { useNotifications } from "./useNotifications";
import { useRuntimeStatus } from "./useRuntimeStatus";
import { useAppUpdates } from "./useAppUpdates";
import {
  NoticeLoadingText,
  NoticeToast,
  useAppNoticeController,
} from "./useAppNotice";
import {
  ConfirmationDialogHost,
  useConfirmationController,
} from "./useConfirmationDialog";
import {
  CodexAppPathDialogHost,
  useCodexAppPathDialogController,
} from "./CodexAppPathDialogHost";
import type {
  AppProps,
  CcSwitchStatus,
  CodexAppDirectorySelection,
  Config,
  CrashpadCleanup,
  ModelState,
  PluginMarketplaceStatus,
  TraceLogCleanup,
} from "./App.types";
import { Badge, Button, Button as SaveButton } from "./components/semi";

const Check = IconCheck;
const X = IconX;
const SETTINGS_OVERLAY_Z_INDEX = 2147483647;

function CodeyBrandMark() {
  return (
    <svg
      className="config-brand-mark"
      viewBox="0 0 350 350"
      aria-hidden="true"
      focusable="false"
    >
      <rect x="0" y="0" width="350" height="350" rx="34" fill="#fff" />
      <path
        d="M70 301c-16 0-24-18-13-30l73-77c8-8 8-20 0-28L65 101C50 86 57 61 78 57c9-2 18 1 25 8l91 91c18 18 18 46 0 64l-66 66c-6 6-2 15 7 15h183"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="22"
      />
    </svg>
  );
}

type SettingsModalShellProps = {
  afterClose?: () => void;
  children: React.ReactNode;
  container?: HTMLElement | null;
  header?: React.ReactNode;
  onCancel: () => void;
  title?: React.ReactNode;
  visible: boolean;
};

function SettingsModalShell({
  afterClose,
  children,
  container,
  header,
  onCancel,
  title,
  visible,
}: SettingsModalShellProps) {
  const headingProps = header === undefined ? { title } : { header };
  return (
    <SemiModal
      {...headingProps}
      afterClose={afterClose}
      centered
      className="codey-settings-modal-layer"
      closeOnEsc={false}
      closable={header === undefined}
      footer={null}
      getPopupContainer={container ? () => container : undefined}
      mask
      maskClosable={false}
      modalContentClass="codey-settings-modal-content"
      onCancel={onCancel}
      visible={visible}
      width={1040}
      zIndex={SETTINGS_OVERLAY_Z_INDEX}
    >
      {children}
    </SemiModal>
  );
}

function useStableEvent<Args extends unknown[], Result>(
  callback: (...args: Args) => Result,
): (...args: Args) => Result {
  const callbackRef = useRef(callback);
  useLayoutEffect(() => {
    callbackRef.current = callback;
  }, [callback]);
  return useCallback(
    (...args: Args) => callbackRef.current(...args),
    [],
  );
}

export function App({
  embedded = false,
  modalContainer,
  modalVisible = true,
  onAfterClose,
  onClose,
}: AppProps) {
  const [config, setConfig] = useState<Config | null>(null);
  const persistedConfigRef = useRef<Config | null>(null);
  const { status, setStatus, refreshStatus, refreshStatusForLoad } =
    useRuntimeStatus({ embedded });
  const [pluginMarketplaceStatus, setPluginMarketplaceStatus] =
    useState<PluginMarketplaceStatus | null>(null);
  const [ccSwitchStatus, setCcSwitchStatus] = useState<CcSwitchStatus | null>(
    null,
  );
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [portalContainer, setPortalContainer] = useState<HTMLElement | null>(
    null,
  );
  const [traceSnapshotStale, setTraceSnapshotStale] = useState(false);
  const noticeController = useAppNoticeController();
  const confirmationController = useConfirmationController();
  const codexAppPathDialog = useCodexAppPathDialogController();
  const setNotice = noticeController.setNotice;
  const setConfirmation = confirmationController.setConfirmation;

  const provider = ccSwitchStatus?.provider;
  const isBusy = busy !== null;
  const configLoaded = config !== null;
  const setPersistedConfig = useCallback((next: Config) => {
    persistedConfigRef.current = next;
    setConfig(next);
  }, []);
  const setSubagentOptimization = useCallback((enabled: boolean) => {
    setConfig((current) =>
      current ? { ...current, subagentOptimization: enabled } : current,
    );
    setDirty(true);
  }, []);
  const runOperation = useCallback(
    async (name: string, action: () => Promise<void>) => {
      if (isBusy) return;
      setBusy(name);
      try {
        await action();
      } catch (error) {
        setNotice({ tone: "error", text: errorText(error) });
      } finally {
        setBusy(null);
      }
    },
    [isBusy, setNotice],
  );
  const operationsStatus = useMemo(
    () => ({
      running: status.running,
      codexAppVersion: status.codexAppVersion,
      clientPlatform: status.clientPlatform,
      restartRequired: status.restartRequired,
      restartInProgress: status.restartInProgress,
      startupError: status.startupError,
      codexAppPath: status.codexAppPath,
      maintenance: status.maintenance,
      injectionScripts: status.injectionScripts,
    }),
    [
      status.running,
      status.codexAppVersion,
      status.clientPlatform,
      status.restartRequired,
      status.restartInProgress,
      status.startupError,
      status.codexAppPath,
      status.maintenance,
      status.injectionScripts,
    ],
  );
  const {
    subagentModelOptions,
    modelState,
    setModelState,
    modelPickerVisible,
    setModelPickerVisible,
    customModelInput,
    modelInputError,
    modelSyncWarning,
    draftModelSet,
    draftManualThirdPartyModelKeys,
    manualThirdPartyModelKeys,
    thirdPartyModelOptions,
    fetchCurrentModels,
    updateSubagentOptimization,
    toggleDraftModel,
    deleteDraftThirdPartyModel,
    updateCustomModelInput,
    addCustomModel,
    saveModelSelection,
    deleteThirdPartyModel,
    setDefaultModel,
  } = useModelSelection({
    provider,
    runOperation,
    setPersistedConfig,
    setStatus,
    setNotice,
    setSubagentOptimization,
  });
  const {
    addNotificationChannel,
    updateNotificationChannel,
    removeNotificationChannel,
  } = useNotifications({
    setConfig,
    setDirty,
  });
  const {
    updateResult,
    updateCheck,
    downloadedUpdate,
    checkForUpdates,
    downloadUpdate,
    askInstallDownloadedUpdate,
  } = useAppUpdates({
    embedded,
    configLoaded,
    isBusy,
    setBusy,
    setNotice,
    setConfirmation,
  });

  useEffect(() => {
    void load();
  }, []);

  async function load() {
    try {
      const result = await invoke<{
        config: Config;
        modelState?: ModelState;
        startupError?: string;
        codexAppPathSelectionRequired?: boolean;
        ccSwitch?: CcSwitchStatus;
      }>("load_codey_config");
      setPersistedConfig(result.config);
      setCcSwitchStatus(result.ccSwitch ?? null);
      if (result.modelState) setModelState(result.modelState);
      const [next] = await Promise.all([
        refreshStatusForLoad(),
        refreshPluginMarketplaceStatus(),
      ]);
      const startupError = next.startupError || result.startupError;
      codexAppPathDialog.setOpen(Boolean(result.codexAppPathSelectionRequired));
      if (startupError) {
        setNotice({ tone: "error", text: `自动启动失败：${startupError}` });
      } else if (next.restartRequired) {
        setNotice({ tone: "info", text: "已保存的配置需重启 Codex 后生效" });
      } else {
        setNotice({
          tone: next.running ? "success" : "info",
          text: next.running
            ? "当前线路和模型目录已同步"
            : "Codey 运行时已就绪",
        });
      }
    } catch (error) {
      setNotice({ tone: "error", text: errorText(error) });
    }
  }

  async function refreshPluginMarketplaceStatus() {
    try {
      const next = await invoke<PluginMarketplaceStatus>(
        "plugin_marketplace_status",
      );
      setPluginMarketplaceStatus(next);
      return next;
    } catch (error) {
      const next: PluginMarketplaceStatus = {
        status: "error",
        needsRepair: true,
        message: errorText(error),
      };
      setPluginMarketplaceStatus(next);
      return next;
    }
  }

  function editConfig(next: Config) {
    setConfig(next);
    setDirty(true);
  }

  async function persist(next: Config) {
    const result = await invoke<{
      config: Config;
      ccSwitch?: CcSwitchStatus;
      modelState?: ModelState;
      restartRequired?: boolean;
      subagentDefaultsHotReloaded?: boolean;
      subagentDefaultsHotReloadError?: string;
    }>("save_codey_config", { config: next });
    setPersistedConfig(result.config);
    window.dispatchEvent(
      new CustomEvent("codey:config-changed", {
        detail: { config: result.config },
      }),
    );
    if (result.ccSwitch) setCcSwitchStatus(result.ccSwitch);
    if (result.modelState) setModelState(result.modelState);
    if (typeof result.restartRequired === "boolean") {
      setStatus((current) => ({
        ...current,
        restartRequired: result.restartRequired,
      }));
    }
    setDirty(false);
    return result;
  }

  async function chooseCodexAppDirectory() {
    if (isBusy) return;
    setBusy("pick-codex-app-directory");
    codexAppPathDialog.setError("");
    try {
      const result = await invoke<CodexAppDirectorySelection>(
        "pick_codex_app_directory",
      );
      if (result.status === "selected" && result.path) {
        codexAppPathDialog.setSelectedPath(result.path);
      }
    } catch (error) {
      codexAppPathDialog.setError(errorText(error));
    } finally {
      setBusy(null);
    }
  }

  async function confirmCodexAppPath() {
    const selectedCodexAppPath =
      codexAppPathDialog.getSnapshot().selectedPath;
    if (!selectedCodexAppPath || isBusy) return;
    setBusy("set-codex-app-path");
    codexAppPathDialog.setError("");
    try {
      const result = await invoke<{
        config: Config;
        ccSwitch?: CcSwitchStatus;
        modelState?: ModelState;
      }>("set_codex_app_path", { path: selectedCodexAppPath });
      setPersistedConfig(result.config);
      if (result.ccSwitch) setCcSwitchStatus(result.ccSwitch);
      if (result.modelState) setModelState(result.modelState);
      await invoke("launch_codey");
      await refreshStatus();
      codexAppPathDialog.setOpen(false);
      codexAppPathDialog.setSelectedPath("");
      setNotice({
        tone: "success",
        text: "Codex 应用路径已校验并保存，客户端已启动",
      });
    } catch (error) {
      codexAppPathDialog.setError(errorText(error));
    } finally {
      setBusy(null);
    }
  }

  async function syncCurrentProvider() {
    if (dirty || isBusy) return;
    await runOperation("sync-provider", async () => {
      const result = await invoke<{
        config: Config;
        ccSwitch: CcSwitchStatus;
        modelState: ModelState;
        restartRequired?: boolean;
      }>("sync_current_provider");
      setPersistedConfig(result.config);
      setCcSwitchStatus(result.ccSwitch);
      setModelState(result.modelState);
      setStatus((current) => ({
        ...current,
        restartRequired: result.restartRequired ?? current.restartRequired,
      }));
      setNotice({
        tone: result.restartRequired ? "info" : "success",
        text: result.restartRequired
          ? `已读取「${result.ccSwitch.provider.name}」，重启 Codex 后应用线路`
          : `已同步「${result.ccSwitch.provider.name}」`,
      });
    });
  }

  async function testCurrentProvider() {
    await runOperation("test-provider", async () => {
      const result = await invoke<{
        model: string;
        protocol: string;
        httpStatus: number;
      }>("test_current_provider");
      setNotice({
        tone: "success",
        text: `模型「${result.model}」对话测试通过（HTTP ${result.httpStatus}）`,
      });
    });
  }

  async function saveCurrent() {
    if (!config) return;
    await runOperation("save", async () => {
      const result = await persist(config);
      const subagentHotReloadFailed = Boolean(
        result.subagentDefaultsHotReloadError,
      );
      setNotice({
        tone:
          result.restartRequired || subagentHotReloadFailed
            ? "info"
            : "success",
        text: result.subagentDefaultsHotReloaded
          ? "Codey 设置已保存；子代理模型和思考深度已实时更新"
          : subagentHotReloadFailed
            ? "Codey 设置已保存；子代理配置暂未能热更新，重启 Codex 后生效"
            : result.restartRequired
              ? "Codey 设置已保存，启动参数将在重启 Codex 后生效"
              : "Codey 设置已保存",
      });
    });
  }

  function closeSettings() {
    if (persistedConfigRef.current) {
      setConfig(persistedConfigRef.current);
    }
    setDirty(false);
    setModelPickerVisible(false);
    confirmationController.clear();
    onClose?.();
  }

  function askClearTraceLogs() {
    setConfirmation({
      action: "clear",
      title: "清理 Codex 诊断存储？",
      description:
        "将清空并压缩 logs_*.sqlite，同时删除已稳定写入的 Crashpad 待处理报告。最近写入、未知文件和其他 Crashpad 目录会保留；聊天历史、账号、配置及插件不受影响。清理后的诊断记录无法恢复。",
      confirmLabel: "确认清理",
      run: () => void clearTraceLogs(),
    });
  }

  function askRestartCodex() {
    setConfirmation({
      action: "restart",
      title: "重启 Codex？",
      description:
        "当前 Codex 客户端将被关闭并由 Codey 自动重新拉起，正在执行的本地任务会被中断。",
      confirmLabel: "重启 Codex",
      run: () => void restartCodex(),
    });
  }

  function askRemoveNotificationChannel(channel: NotificationChannel) {
    const channelName = channel.kind === "telegram" ? "Telegram" : "飞书";
    setConfirmation({
      action: "delete-notification-channel",
      title: `删除${channelName}通知渠道？`,
      description:
        "将从当前设置中移除这个通知渠道。删除后不会再接收自动通知；如果尚未保存，可关闭设置页面撤销本次更改。",
      confirmLabel: "删除渠道",
      run: () => removeNotificationChannel(channel.id),
    });
  }

  async function restartCodex() {
    if (!config) return;
    await runOperation("restart", async () => {
      if (dirty) await persist(config);
      setNotice({
        tone: "info",
        text: "正在重启 Codex，Codey 将自动重新拉起客户端…",
      });
      await invoke("restart_codey");
      setStatus((current) => ({
        ...current,
        restartInProgress: true,
      }));
    });
  }

  async function repairPluginMarketplace() {
    await runOperation("repair-plugin-marketplace", async () => {
      const result = await withTimeout(
        invoke<PluginMarketplaceStatus>("repair_plugin_marketplace"),
        30_000,
        "插件市场修复超时，请稍后重试",
      );
      setPluginMarketplaceStatus(result);
      if (result.status === "ready") {
        setNotice({
          tone: "success",
          text:
            result.configChanged || result.initializedRemote
              ? "插件市场已修复并立即生效，无需重启 Codex"
              : "插件市场状态正常，无需修改",
        });
        return;
      }
      setNotice({
        tone: "error",
        text: "插件市场仍有缺失项，请检查本地市场文件后重试",
      });
    });
  }

  async function clearTraceLogs() {
    await runOperation("clear-trace-logs", async () => {
      const result = await invoke<{
        status: "ok" | "partial";
        traceCleanup?: TraceLogCleanup;
        crashpadCleanup: CrashpadCleanup;
        traceProtectionEnabled: boolean;
        crashpadProtectionEnabled: boolean;
        errors: string[];
        traceLogStats: TraceLogStats;
        crashpadPendingStats: CrashpadPendingStats;
      }>("clear_diagnostic_storage");
      setStatus((current) => ({
        ...current,
        traceLogStats: result.traceLogStats,
        crashpadPendingStats: result.crashpadPendingStats,
      }));
      setTraceSnapshotStale(false);
      const traceCleanup = result.traceCleanup;
      const crashpadCleanup = result.crashpadCleanup;
      if (
        (traceCleanup?.databasesFound ?? 0) === 0 &&
        crashpadCleanup.reportsFound === 0
      ) {
        setNotice({
          tone: "info",
          text: "未发现可清理的 Trace 日志或 Crashpad 待处理报告",
        });
        return;
      }
      const traceDetail = traceCleanup
        ? `${traceCleanup.databasesCleaned} 个日志库、${traceCleanup.rowsDeleted} 条记录`
        : "Trace 日志清理未完成";
      const crashpadDetail =
        `${crashpadCleanup.reportsDeleted} 份 Crashpad 报告、` +
        `${crashpadCleanup.filesDeleted} 个文件`;
      const reclaimed =
        (traceCleanup?.bytesReclaimed ?? 0) +
        crashpadCleanup.bytesReclaimed;
      const protectionDetail =
        result.traceProtectionEnabled && result.crashpadProtectionEnabled
          ? "双重保护保持开启"
          : "当前仅启用了部分保护";
      setNotice({
        tone: result.errors.length ? "error" : "success",
        text: `已处理 ${traceDetail}；${crashpadDetail}，释放 ${formatBytes(reclaimed)}；${protectionDetail}${result.errors.length ? `，另有 ${result.errors.length} 项未完成` : ""}`,
      });
    });
  }

  async function updateTraceLogStatsSnapshot() {
    const result = await invoke<{
      status: "ok" | "pending";
      traceLogStats: TraceLogStats;
      crashpadPendingStats: CrashpadPendingStats;
    }>("refresh_diagnostic_storage_stats");
    setStatus((current) => ({
      ...current,
      traceLogStats: result.traceLogStats,
      crashpadPendingStats: result.crashpadPendingStats,
    }));
    return result;
  }

  async function refreshTraceLogStats() {
    await runOperation("refresh-trace-stats", async () => {
      const result = await updateTraceLogStatsSnapshot();
      if (result.status === "pending") {
        setNotice({ tone: "info", text: "诊断存储正在统计，请稍候" });
        return;
      }
      setTraceSnapshotStale(false);
      setNotice({ tone: "success", text: "诊断存储统计已更新" });
    });
  }

  const handleCloseSettings = useStableEvent(closeSettings);
  const handleSaveCurrent = useStableEvent(() => void saveCurrent());
  const handleRepairPluginMarketplace = useStableEvent(
    () => void repairPluginMarketplace(),
  );
  const handleRestartCodex = useStableEvent(askRestartCodex);
  const handleCheckForUpdates = useStableEvent(
    () => void checkForUpdates(),
  );
  const handleDownloadUpdate = useStableEvent(
    () => void downloadUpdate(),
  );
  const handleInstallDownloadedUpdate = useStableEvent(
    askInstallDownloadedUpdate,
  );
  const handleConfigChange = useStableEvent(editConfig);
  const handleAddNotificationChannel = useStableEvent(addNotificationChannel);
  const handleNotificationChannelChange = useStableEvent(
    updateNotificationChannel,
  );
  const handleRequestRemoveNotificationChannel = useStableEvent(
    askRemoveNotificationChannel,
  );
  const handleSubagentOptimizationChange = useCallback(
    (checked: boolean) => {
      void updateSubagentOptimization(
        checked,
        config?.subagentModel ?? "",
      );
    },
    [config?.subagentModel, updateSubagentOptimization],
  );
  const handleSyncCurrentProvider = useStableEvent(
    () => void syncCurrentProvider(),
  );
  const handleShowAccountUsageInHeaderChange = useStableEvent(
    (checked: boolean) => {
      if (config) {
        editConfig({ ...config, showAccountUsageInHeader: checked });
      }
    },
  );
  const handleTestCurrentProvider = useStableEvent(
    () => void testCurrentProvider(),
  );
  const handleClearTraceLogs = useStableEvent(askClearTraceLogs);
  const handleRefreshTraceLogStats = useStableEvent(
    () => void refreshTraceLogStats(),
  );
  const handleModelPickerOpenChange = useStableEvent((open: boolean) => {
    if (!isBusy || open) setModelPickerVisible(open);
  });
  const handleCodexAppPathOpenChange = useStableEvent((open: boolean) => {
    if (!isBusy) codexAppPathDialog.setOpen(open);
  });
  const handleChooseCodexAppDirectory = useStableEvent(
    () => void chooseCodexAppDirectory(),
  );
  const handleConfirmCodexAppPath = useStableEvent(
    () => void confirmCodexAppPath(),
  );

  if (!config || !provider) {
    const loadingContent = (
      <main className="app-shell loading-shell">
        <div className="loading-mark">
          <GitBranch size={17} />
        </div>
        <div>
          <strong>正在载入 Codey</strong>
          <p>
            <NoticeLoadingText controller={noticeController} />
          </p>
        </div>
        <LoaderCircle
          className="spinner loading-spinner"
          size={16}
          aria-hidden="true"
        />
      </main>
    );
    return embedded ? (
      <SettingsModalShell
        afterClose={onAfterClose}
        container={modalContainer}
        onCancel={handleCloseSettings}
        title="Codey 配置"
        visible={modalVisible}
      >
        {loadingContent}
      </SettingsModalShell>
    ) : loadingContent;
  }

  const configHeaderContent = (
    <div className="config-header-inner">
      <div className="config-brand">
        <CodeyBrandMark />
        <div className="config-brand-copy">
          <div className="config-brand-title-row">
            <h1 id={embedded ? "semi-modal-title" : undefined}>Codey 控制台</h1>
            <span className="app-version-tag">
              v{status.appVersion || "0.2.0"}
            </span>
            {dirty && (
              <Badge variant="warning" className="unsaved-badge">
                未保存更改
              </Badge>
            )}
          </div>
          <p>管理 Codex 线路、模型服务、运行策略与诊断日志</p>
        </div>
      </div>

      <div className="config-header-right">
        <div className="config-header-actions">
          {embedded && (
            <Button
              aria-label={status.running ? "重启 Codex" : "Codex 未运行"}
              className="title-restart-button"
              disabled={isBusy || status.restartInProgress || !status.running}
              onClick={handleRestartCodex}
              size="sm"
              variant="warning"
            >
              {busy === "restart" || status.restartInProgress ? (
                <LoaderCircle className="spinner" aria-hidden="true" />
              ) : (
                <RefreshCw aria-hidden="true" />
              )}
              <span className="title-action-label">
                {status.running ? "重启 Codex" : "未运行"}
              </span>
            </Button>
          )}
          <SaveButton
            aria-label={dirty ? "保存更改" : "已保存"}
            className={`save-button${embedded ? " title-save-button" : ""}${dirty ? " dirty" : ""}`}
            disabled={!dirty || isBusy}
            onClick={handleSaveCurrent}
          >
            {busy === "save" ? (
              <LoaderCircle className="spinner" aria-hidden="true" />
            ) : dirty ? (
              <Save aria-hidden="true" />
            ) : (
              <Check aria-hidden="true" />
            )}
            <span className="title-action-label">
              {dirty ? "保存更改" : "已保存"}
            </span>
          </SaveButton>
          {embedded && (
            <Button
              aria-label="关闭配置"
              className="codey-settings-modal-close"
              onClick={handleCloseSettings}
              size="icon-sm"
              variant="ghost"
            >
              <X aria-hidden="true" />
            </Button>
          )}
        </div>
      </div>
    </div>
  );

  const appContent = (
    <main
      className={`app-shell${embedded ? " embedded" : ""}`}
      ref={setPortalContainer}
    >
      <a className="skip-link" href="#codey-settings-content">
        跳至设置内容
      </a>

      {!embedded && (
        <div className="macos-titlebar">
          <div className="macos-traffic-lights">
            <button
              className="traffic-light close"
              title="关闭"
              aria-label="关闭窗口"
            />
            <button
              className="traffic-light minimize"
              title="最小化"
              aria-label="最小化窗口"
            />
            <button
              className="traffic-light zoom"
              title="缩放"
              aria-label="全屏缩放"
            />
          </div>
          <div className="macos-titlebar-title">
            <span className="app-title-text">Codey Control Panel</span>
            <span className="app-version-tag">
              v{status.appVersion || "0.2.0"}
            </span>
          </div>
          <div className="macos-titlebar-right" aria-hidden="true" />
        </div>
      )}

      {!embedded && (
        <header className="config-header">{configHeaderContent}</header>
      )}

      <div className="page-scroll">
        <div className="page" id="codey-settings-content">
          {/* 最上方：运行状态 (Codex 运行与维护，含 Codex 应用路径) */}
          <OperationsPanel
            config={config}
            status={operationsStatus}
            busy={busy}
            isBusy={isBusy}
            pluginMarketplaceStatus={pluginMarketplaceStatus}
            onRepairPluginMarketplace={handleRepairPluginMarketplace}
            onRestart={handleRestartCodex}
            showRestartAction={!embedded}
          />

          {/* 中间区域：分左右两栏 (左侧: 应用更新; 右侧: 消息通知与功能策略) */}
          <div className="upper-dashboard-grid">
            {/* 左侧栏：应用更新 */}
            <div className="dashboard-column upper-left-column">
              <AppUpdateCard
                appVersion={status.appVersion}
                updateResult={updateResult}
                updateCheck={updateCheck}
                downloadedUpdate={downloadedUpdate}
                busy={busy}
                isBusy={isBusy}
                onCheckUpdates={handleCheckForUpdates}
                onDownloadUpdate={handleDownloadUpdate}
                onInstallUpdate={handleInstallDownloadedUpdate}
              />
            </div>

            {/* 右侧栏：消息通知 */}
            <div className="dashboard-column upper-right-column">
              <NotificationChannelsCard
                config={config}
                container={portalContainer}
                isBusy={isBusy}
                onAddChannel={handleAddNotificationChannel}
                onChannelChange={handleNotificationChannelChange}
                onRequestRemoveChannel={handleRequestRemoveNotificationChannel}
              />
            </div>
          </div>

          {/* Codey 功能策略：整行独占排布 */}
          <div className="full-row-section feature-full-section">
            <FeaturePolicyCard
              config={config}
              isMacClient={status.clientPlatform === "macos"}
              busy={busy}
              isBusy={isBusy}
              subagentModelOptions={subagentModelOptions}
              onConfigChange={handleConfigChange}
              onSubagentOptimizationChange={handleSubagentOptimizationChange}
            />
          </div>

          {/* 线路与模型：整行独占排布 */}
          <div className="full-row-section model-full-section">
            <ModelSection
              provider={provider}
              modelState={modelState}
              dirty={dirty}
              isBusy={isBusy}
              busy={busy}
              showAccountUsageInHeader={config.showAccountUsageInHeader}
              onSyncCurrentProvider={handleSyncCurrentProvider}
              onFetchCurrentModels={fetchCurrentModels}
              onTestCurrentProvider={handleTestCurrentProvider}
              onSetDefaultModel={setDefaultModel}
              onDeleteThirdPartyModel={deleteThirdPartyModel}
              manualThirdPartyModelKeys={manualThirdPartyModelKeys}
              onShowAccountUsageInHeaderChange={
                handleShowAccountUsageInHeaderChange
              }
            />
          </div>

          {/* 诊断存储保护：整行独占排布 */}
          <div className="full-row-section trace-full-section">
            <TraceLogModule
              stats={status.traceLogStats}
              crashpadStats={status.crashpadPendingStats}
              crashpadSupported={status.clientPlatform === "macos"}
              snapshotStale={traceSnapshotStale}
              traceProtectionEnabled={config.disableTraceLogWrites}
              crashpadProtectionEnabled={config.protectCrashpadPending}
              clearBusy={busy === "clear-trace-logs"}
              refreshing={busy === "refresh-trace-stats"}
              disabled={isBusy}
              onClear={handleClearTraceLogs}
              onRefresh={handleRefreshTraceLogStats}
            />
          </div>
        </div>
      </div>

      <NoticeToast
        autoDismissEnabled={Boolean(config && provider)}
        controller={noticeController}
      />

      <ModelPickerDialog
        open={modelPickerVisible}
        isBusy={isBusy}
        busy={busy}
        container={portalContainer}
        customModelInput={customModelInput}
        modelInputError={modelInputError}
        modelSyncWarning={modelSyncWarning}
        thirdPartyModelOptions={thirdPartyModelOptions}
        modelState={modelState}
        draftModelSet={draftModelSet}
        manualThirdPartyModelKeys={draftManualThirdPartyModelKeys}
        onOpenChange={handleModelPickerOpenChange}
        onCustomModelInputChange={updateCustomModelInput}
        onAddCustomModel={addCustomModel}
        onToggleDraftModel={toggleDraftModel}
        onDeleteThirdPartyModel={deleteDraftThirdPartyModel}
        onSave={saveModelSelection}
      />

      <ConfirmationDialogHost
        container={portalContainer}
        controller={confirmationController}
      />

      {status.clientPlatform === "windows" && (
        <CodexAppPathDialogHost
          controller={codexAppPathDialog}
          isBusy={isBusy}
          busy={busy}
          container={portalContainer}
          onOpenChange={handleCodexAppPathOpenChange}
          onChooseDirectory={handleChooseCodexAppDirectory}
          onConfirm={handleConfirmCodexAppPath}
        />
      )}
    </main>
  );
  return embedded ? (
    <SettingsModalShell
      afterClose={onAfterClose}
      container={modalContainer}
      header={(
        <div className="semi-modal-header codey-settings-modal-header">
          {configHeaderContent}
        </div>
      )}
      onCancel={handleCloseSettings}
      visible={modalVisible}
    >
      {appContent}
    </SettingsModalShell>
  ) : appContent;
}
