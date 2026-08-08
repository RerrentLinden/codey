import { memo, useMemo, useRef, useState } from "react";
import {
  IconActivity as Activity,
  IconAdjustmentsHorizontal,
  IconBrandWindows,
  IconClock,
  IconCode as Code,
  IconCheck as Check,
  IconCloudCheck,
  IconCpu,
  IconDatabase,
  IconFileCheck,
  IconFolderOpen,
  IconHistory,
  IconLoader2 as LoaderCircle,
  IconPlugConnected as PlugZap,
  IconRefresh as RefreshCw,
  IconShieldCheck,
  IconShoppingBag,
  IconX as X,
  IconBolt as Zap,
} from "@tabler/icons-react";

import type { Config, PluginMarketplaceStatus, RuntimeStatus } from "./App.types";
import { Badge, Button, Card, Tooltip } from "./components/semi";

const Cpu = IconCpu;
const FolderOpen = IconFolderOpen;
const History = IconHistory;
const EMPTY_INJECTION_SCRIPTS: NonNullable<
  RuntimeStatus["injectionScripts"]
> = [];

type OperationsRuntimeStatus = Pick<
  RuntimeStatus,
  | "running"
  | "codexAppVersion"
  | "clientPlatform"
  | "restartRequired"
  | "restartInProgress"
  | "startupError"
  | "codexAppPath"
  | "maintenance"
  | "injectionScripts"
>;

type OperationsPanelProps = {
  config: Config;
  status: OperationsRuntimeStatus;
  busy: string | null;
  isBusy: boolean;
  pluginMarketplaceStatus: PluginMarketplaceStatus | null;
  onRepairPluginMarketplace: () => void;
  onRestart: () => void;
  showRestartAction?: boolean;
};

function OperationsPanelComponent({
  config,
  status,
  busy,
  isBusy,
  pluginMarketplaceStatus,
  onRepairPluginMarketplace,
  onRestart,
  showRestartAction = true,
}: OperationsPanelProps) {
  const operationsHubRef = useRef<HTMLElement>(null);
  const [activeCardTitle, setActiveCardTitle] = useState<string | null>(null);

  const getTooltipContainer = () =>
    operationsHubRef.current?.closest<HTMLElement>(".app-shell") ??
    document.body;

  const toggleCard = (title: string) => {
    setActiveCardTitle((prev) => (prev === title ? null : title));
  };

  const maintenance = status.maintenance;
  const sessionOk = maintenance?.sessionStatus === "ready";
  const pluginOk = pluginMarketplaceStatus?.status === "ready";
  const pluginStatusError = pluginMarketplaceStatus?.status === "error";
  const pluginRepairing = busy === "repair-plugin-marketplace";
  const pluginStatusKnown = Boolean(
    pluginMarketplaceStatus && !pluginStatusError,
  );
  const officialMarketplaceReady =
    pluginMarketplaceStatus?.officialMarketplace === true &&
    pluginMarketplaceStatus.officialRegistered === true;
  const remoteMarketplaceCached =
    pluginMarketplaceStatus?.remoteMarketplace === true;
  const remoteMarketplaceReady =
    remoteMarketplaceCached &&
    pluginMarketplaceStatus?.remoteRegistered === true;
  const performanceError =
    maintenance?.performanceStatus === "error" ||
    maintenance?.performanceStatus === "degraded";
  const injectionScripts = status.injectionScripts ?? EMPTY_INJECTION_SCRIPTS;
  const {
    effectiveScriptCount,
    failedInjectionScriptCount,
    unverifiedInjectionScriptCount,
  } = useMemo(() => {
    let effective = 0;
    let failed = 0;
    let unverified = 0;
    for (const script of injectionScripts) {
      if (script.status === "effective") effective += 1;
      else if (script.status === "executed") unverified += 1;
      else if (script.status === "failed" || script.status === "unknown") {
        failed += 1;
      }
    }
    return {
      effectiveScriptCount: effective,
      failedInjectionScriptCount: failed,
      unverifiedInjectionScriptCount: unverified,
    };
  }, [injectionScripts]);
  const injectionStatusPending = injectionScripts.length === 0;
  const injectionError = failedInjectionScriptCount > 0;
  const isWindowsClient = status.clientPlatform === "windows";
  const windowsPatchReady = maintenance?.performanceStatus === "ready";
  const windowsPatchFailed = performanceError || Boolean(status.startupError);
  const windowsPatchTone = windowsPatchReady
    ? "success"
    : windowsPatchFailed
      ? "destructive"
      : "warning";
  const windowsPatchLabel = windowsPatchReady
    ? "已启用"
    : windowsPatchFailed
      ? "未生效"
      : "待检测";
  const windowsPatchDetail = windowsPatchReady
    ? maintenance?.performanceDetail ||
      "WMI 周期采样、临时 WebView 残留与执行环境泄漏修复已生效。"
    : windowsPatchFailed
      ? maintenance?.performanceDetail ||
        status.startupError ||
        "Windows 优化补丁加载异常。"
      : status.running
        ? "正在确认 Windows 优化补丁状态。"
        : "将在 Codex 启动时自动安装并校验 Windows 优化补丁。";
  const resolvedCodexPath = status.codexAppPath || "/Applications/ChatGPT.app";
  const restartPending = Boolean(status.restartRequired);
  const codexVersion = status.codexAppVersion?.trim();
  const codexVersionLabel = codexVersion
    ? `Codex v${codexVersion}`
    : "Codex 版本未知";

  type MetricItem = {
    id: string;
    icon: typeof Activity;
    tooltip: string;
    tone?: "success" | "warning" | "destructive" | "info";
  };

  const sessionMetrics = useMemo<MetricItem[]>(
    () => [
      {
        id: "session-files",
        icon: IconFileCheck,
        tooltip: `会话文件：已修复 ${maintenance?.sessionFilesFixed ?? 0} 个会话文件`,
        tone: sessionOk ? "success" : "warning",
      },
      {
        id: "session-db",
        icon: IconDatabase,
        tooltip: `数据库索引：已更新 ${maintenance?.sqliteRowsUpdated ?? 0} 行数据库索引`,
        tone: sessionOk ? "success" : "warning",
      },
      {
        id: "session-ghost",
        icon: IconShieldCheck,
        tooltip: `幽灵任务：已清理 ${maintenance?.ghostTasksPruned ?? 0} 条幽灵任务`,
        tone: sessionOk ? "success" : "warning",
      },
    ],
    [
      maintenance?.ghostTasksPruned,
      maintenance?.sessionFilesFixed,
      maintenance?.sqliteRowsUpdated,
      sessionOk,
    ],
  );

  // System Optimization Metrics
  const optimizationMetrics = useMemo<MetricItem[]>(
    () => [
      {
        id: "opt-fastctx",
        icon: Zap,
        tooltip: config.fastContextTools
          ? "FastCtx 上下文加速：已按当前配置启用"
          : "FastCtx 上下文加速：未启用",
        tone: config.fastContextTools ? "success" : "info",
      },
      {
        id: "opt-slim",
        icon: IconAdjustmentsHorizontal,
        tooltip: config.slimCodexPet
          ? "客户端精简：已开启宠物精简"
          : "客户端精简：保留完整宠物功能",
        tone: config.slimCodexPet ? "success" : "info",
      },
      {
        id: "opt-patch",
        icon: isWindowsClient ? IconBrandWindows : IconCpu,
        tooltip: windowsPatchReady
          ? "性能策略已生效：采样与泄漏修复"
          : "性能策略：运行确认中",
        tone: windowsPatchReady ? "success" : "warning",
      },
      {
        id: "opt-injection",
        icon: Code,
        tooltip: injectionError
          ? `脚本注入：${failedInjectionScriptCount} 个异常`
          : unverifiedInjectionScriptCount > 0
            ? `脚本注入：${unverifiedInjectionScriptCount} 个未验证生效`
            : injectionScripts.length > 0
              ? `脚本注入：${effectiveScriptCount}/${injectionScripts.length} 已生效`
              : "脚本注入：等待 Codex 启动后检测",
        tone: injectionError
          ? "destructive"
          : unverifiedInjectionScriptCount > 0
            ? "warning"
            : injectionScripts.length > 0
              ? "success"
              : "warning",
      },
    ],
    [
      config.fastContextTools,
      config.slimCodexPet,
      effectiveScriptCount,
      failedInjectionScriptCount,
      injectionError,
      injectionScripts.length,
      isWindowsClient,
      unverifiedInjectionScriptCount,
      windowsPatchReady,
    ],
  );

  // Plugin Marketplace Metrics
  const pluginMetrics = useMemo<MetricItem[]>(
    () => [
      {
        id: "plugin-official",
        icon: IconShoppingBag,
        tooltip: !pluginStatusKnown
          ? "官方市场：正在检查"
          : officialMarketplaceReady
            ? "官方市场：快照与注册完整"
            : pluginMarketplaceStatus?.officialMarketplace !== true
              ? "官方市场：快照缺失"
              : "官方市场：快照存在但尚未注册",
        tone: !pluginStatusKnown
          ? "info"
          : officialMarketplaceReady
            ? "success"
            : "warning",
      },
      {
        id: "plugin-remote",
        icon: IconCloudCheck,
        tooltip: !pluginStatusKnown
          ? "远程市场：正在检查本地缓存"
          : !remoteMarketplaceCached
            ? "远程市场：未缓存本地快照，无需修复"
            : remoteMarketplaceReady
              ? "远程市场：缓存与注册完整"
              : "远程市场：已缓存但尚未注册",
        tone:
          !pluginStatusKnown || !remoteMarketplaceCached
            ? "info"
            : remoteMarketplaceReady
              ? "success"
              : "warning",
      },
      {
        id: "plugin-host",
        icon: PlugZap,
        tooltip: pluginOk
          ? "插件托管：插件服务正常且链路已就绪"
          : "插件托管：正在检查或等待修复",
        tone: pluginOk ? "success" : "warning",
      },
    ],
    [
      officialMarketplaceReady,
      pluginMarketplaceStatus?.officialMarketplace,
      pluginOk,
      pluginStatusKnown,
      remoteMarketplaceCached,
      remoteMarketplaceReady,
    ],
  );

  const statusCards: Array<{
    title: string;
    description: string;
    metrics: MetricItem[];
    label: string;
    tone: "success" | "warning" | "destructive" | "info";
    icon: typeof Activity;
    action?: {
      label: string;
      disabled: boolean;
      loading: boolean;
      onClick: () => void;
    };
    showInjectionScripts?: boolean;
  }> = [
    {
      title: "会话恢复",
      description: sessionOk
        ? "索引与恢复链路运行正常，上下文恢复就绪。"
        : "正在确认会话索引与恢复链路。",
      metrics: sessionMetrics,
      label: sessionOk ? "正常" : maintenance ? "需检查" : "检查中",
      tone: sessionOk ? "success" : maintenance ? "destructive" : "warning",
      icon: History,
    },
    {
      title: "系统优化",
      description: injectionError
        ? `${failedInjectionScriptCount} 个脚本注入异常，可展开查看错误。`
        : unverifiedInjectionScriptCount > 0
          ? `${unverifiedInjectionScriptCount} 个脚本已执行，但未验证实际效果。`
          : injectionStatusPending
            ? status.running
              ? "正在读取最近一次脚本注入结果。"
              : "Codex 启动后将记录每个脚本的注入结果。"
            : !performanceError
              ? "精简策略、性能补丁与脚本生效自检均已通过。"
              : "部分精简策略尚未启用，保留完整功能。",
      metrics: optimizationMetrics,
      label: injectionError
        ? `${failedInjectionScriptCount} 个异常`
        : unverifiedInjectionScriptCount > 0
          ? `${unverifiedInjectionScriptCount} 个未验证`
          : injectionStatusPending
            ? status.running
              ? "检测中"
              : "待启动"
            : performanceError
              ? "异常"
              : "已优化",
      tone:
        injectionError || performanceError
          ? "destructive"
          : injectionStatusPending || unverifiedInjectionScriptCount > 0
            ? "warning"
            : "success",
      icon: Cpu,
      showInjectionScripts: true,
    },
    {
      title: "插件市场",
      description: pluginOk
        ? "配置状态完整，可正常发现与管理插件。"
        : "仅检查当前状态，不会在打开配置页时自动修复。",
      metrics: pluginMetrics,
      label: pluginRepairing
        ? "修复中"
        : pluginOk
          ? "正常"
          : pluginStatusError
            ? "读取失败"
            : pluginMarketplaceStatus
              ? "需修复"
              : "检查中",
      tone: pluginOk
        ? "success"
        : pluginStatusError
          ? "destructive"
          : "warning",
      icon: PlugZap,
      action: {
        label: pluginOk ? "重新检查并修复" : "手动修复",
        disabled: isBusy,
        loading: pluginRepairing,
        onClick: onRepairPluginMarketplace,
      },
    },
  ];

  return (
    <section
      ref={operationsHubRef}
      className={`operations-hub${restartPending ? " pending" : status.running ? " running" : ""}`}
      aria-labelledby="operations-title"
    >
      <Card className="operations-panel">
        <div className="operations-header">
          <div className="operations-heading">
            <span className="operations-heading-icon">
              <Activity size={18} aria-hidden="true" />
            </span>
            <div className="operations-heading-copy">
              <div className="operations-title-row">
                <h1 id="operations-title">Codex 运行状态</h1>
                <span className="codex-version-tag">{codexVersionLabel}</span>
              </div>
              <div
                className="path-display header-path-display"
                aria-label="Codex 应用路径"
              >
                <FolderOpen size={14} aria-hidden="true" />
                <code>{config.codexAppPath || resolvedCodexPath}</code>
              </div>
            </div>
          </div>

          <div className="operations-actions">
            <div
              className="operations-status-icons"
              role="list"
              aria-label="核心服务状态"
            >
              {statusCards.map((item) => {
                const StatusIcon = item.icon;
                const isExpanded = activeCardTitle === item.title;
                return (
                  <Tooltip
                    key={item.title}
                    content={
                      isExpanded
                        ? `收起“${item.title}”`
                        : `点击展开“${item.title}”详情`
                    }
                    getPopupContainer={getTooltipContainer}
                    position="top"
                  >
                    <button
                      type="button"
                      className={`operations-icon-badge tone-${item.tone}${isExpanded ? " active" : ""}`}
                      onClick={() => toggleCard(item.title)}
                      aria-expanded={isExpanded}
                      aria-label={`${item.title}（${item.label}），点击${isExpanded ? "收起" : "展开"}`}
                    >
                      <StatusIcon size={16} aria-hidden="true" />
                      <span
                        className="operations-icon-dot"
                        aria-hidden="true"
                      />
                    </button>
                  </Tooltip>
                );
              })}
            </div>

            <Badge
              variant={
                restartPending
                  ? "warning"
                  : status.running
                    ? "success"
                    : "secondary"
              }
            >
              <span className="operations-status-dot" aria-hidden="true" />
              {restartPending
                ? "等待重启"
                : status.running
                  ? "运行中"
                  : "未启动"}
            </Badge>
            {showRestartAction && (
              <Button
                variant="warning"
                size="sm"
                disabled={isBusy || status.restartInProgress || !status.running}
                onClick={onRestart}
              >
                {busy === "restart" || status.restartInProgress ? (
                  <LoaderCircle className="spinner" aria-hidden="true" />
                ) : (
                  <RefreshCw aria-hidden="true" />
                )}
                {status.running ? "重启 Codex" : "未运行"}
              </Button>
            )}
          </div>
        </div>

        {activeCardTitle && (
          <div
            className="operations-expanded-grid"
            role="region"
            aria-label="展开的系统详情"
          >
            {statusCards
              .filter((item) => item.title === activeCardTitle)
              .map((item) => {
                const StatusIcon = item.icon;
                return (
                  <article
                    key={item.title}
                    className={`operations-expanded-card tone-${item.tone}`}
                  >
                    <div className="expanded-card-header">
                      <div className="expanded-card-title">
                        <span
                          className={`expanded-card-icon tone-${item.tone}`}
                        >
                          <StatusIcon size={18} aria-hidden="true" />
                        </span>
                        <div>
                          <h3>{item.title}</h3>
                          <p>{item.description}</p>
                        </div>
                      </div>
                      <div className="expanded-card-actions">
                        <Badge variant={item.tone}>{item.label}</Badge>
                      </div>
                    </div>

                    <div className="expanded-card-body">
                      <div className="expanded-card-metrics">
                        {item.metrics.map((metric) => {
                          const MetricIcon = metric.icon;
                          return (
                            <div
                              key={metric.id}
                              className="expanded-metric-item"
                            >
                              <span
                                className={`expanded-metric-icon tone-${metric.tone || "info"}`}
                              >
                                <MetricIcon size={14} aria-hidden="true" />
                              </span>
                              <span className="expanded-metric-text">
                                {metric.tooltip}
                              </span>
                            </div>
                          );
                        })}
                      </div>

                      {item.showInjectionScripts && (
                        <section
                          className="injection-status-section"
                          aria-labelledby="injection-status-title"
                        >
                          <div className="injection-status-header">
                            <h4 id="injection-status-title">脚本生效状态</h4>
                            <span className="injection-status-summary">
                              {injectionScripts.length > 0
                                ? `${effectiveScriptCount}/${injectionScripts.length} 已生效`
                                : "暂无结果"}
                            </span>
                          </div>

                          {injectionScripts.length > 0 ? (
                            <div className="injection-status-list" role="list">
                              {injectionScripts.map((script) => {
                                const scriptEffective =
                                  script.status === "effective";
                                const scriptUnverified =
                                  script.status === "executed";
                                const scriptFailed =
                                  script.status === "failed";
                                const scriptUnknown =
                                  script.status === "unknown";
                                const scriptProblem =
                                  scriptFailed || scriptUnknown;
                                const ScriptStatusIcon = scriptEffective
                                  ? Check
                                  : scriptUnverified
                                    ? IconClock
                                    : X;
                                const stateClass = scriptProblem
                                  ? " failed"
                                  : scriptUnverified
                                    ? " unverified"
                                    : "";
                                return (
                                  <div
                                    key={script.id}
                                    className={`injection-status-row${stateClass}`}
                                    role="listitem"
                                  >
                                    <span
                                      className="injection-script-icon"
                                      aria-hidden="true"
                                    >
                                      <Code size={15} />
                                    </span>
                                    <div className="injection-script-copy">
                                      <div className="injection-script-title">
                                        <span>{script.name}</span>
                                        <span className="injection-script-source">
                                          {script.source === "user"
                                            ? "用户脚本"
                                            : "内置"}
                                        </span>
                                      </div>
                                      {script.error && (
                                        <code className="injection-script-error">
                                          {script.error}
                                        </code>
                                      )}
                                      {!script.error && script.detail && (
                                        <span className="injection-script-detail">
                                          {script.detail}
                                        </span>
                                      )}
                                    </div>
                                    <span
                                      className={`injection-script-state${stateClass}`}
                                      title={
                                        scriptEffective
                                          ? "生效探针通过"
                                          : scriptUnverified
                                            ? "脚本已执行，但没有生效证据"
                                            : scriptFailed
                                              ? "脚本未生效"
                                              : "脚本状态异常"
                                      }
                                    >
                                      <ScriptStatusIcon
                                        size={14}
                                        aria-hidden="true"
                                      />
                                      {scriptEffective
                                        ? "已生效"
                                        : scriptUnverified
                                          ? "未验证"
                                          : scriptFailed
                                            ? "失败"
                                            : "异常"}
                                    </span>
                                  </div>
                                );
                              })}
                            </div>
                          ) : (
                            <div className="injection-status-empty">
                              {status.running
                                ? "正在等待最近一次脚本注入结果"
                                : "Codex 启动后将在这里显示每个脚本的注入状态"}
                            </div>
                          )}
                        </section>
                      )}

                      {item.action && (
                        <div className="expanded-card-footer">
                          <Button
                            variant="outline"
                            size="xs"
                            disabled={item.action.disabled}
                            onClick={item.action.onClick}
                          >
                            {item.action.loading ? (
                              <LoaderCircle
                                className="spinner"
                                aria-hidden="true"
                              />
                            ) : (
                              <RefreshCw aria-hidden="true" />
                            )}
                            {item.action.label}
                          </Button>
                        </div>
                      )}
                    </div>
                  </article>
                );
              })}
          </div>
        )}

        {isWindowsClient && (
          <div
            className={`windows-patch-status windows-patch-status-${windowsPatchTone}`}
            role="status"
            aria-live="polite"
          >
            <span className="windows-patch-icon">
              <IconBrandWindows size={18} aria-hidden="true" />
            </span>
            <div className="windows-patch-copy">
              <div className="windows-patch-heading">
                <strong>Windows 优化补丁</strong>
                <Badge variant={windowsPatchTone}>{windowsPatchLabel}</Badge>
              </div>
              <p>{windowsPatchDetail}</p>
            </div>
          </div>
        )}
      </Card>
    </section>
  );
}

export const OperationsPanel = memo(OperationsPanelComponent);
