import { memo } from "react";
import { IconLoader2 as LoaderCircle, IconRefresh as RefreshCw } from "@tabler/icons-react";

import type { Config, RuntimeStatus } from "./App.types";
import { Button, Card, Switch } from "./components/semi";

const EXPERIMENTAL_FEATURES = [
  {
    configKey: "unifiedExec",
    flag: "unified_exec",
    label: "统一命令执行",
    description: "统一命令执行、后台进程管理与流式输出。",
  },
  {
    configKey: "shellSnapshot",
    flag: "shell_snapshot",
    label: "Shell 环境快照",
    description: "复用 Shell 环境快照，减少重复初始化开销。",
  },
  {
    configKey: "responsesWebsocketsV2",
    flag: "responses_websockets_v2",
    label: "Responses WebSocket v2",
    description: "使用 WebSocket v2，降低连接与流式输出延迟。",
  },
  {
    configKey: "toolSearchAlwaysDeferMcpTools",
    flag: "tool_search_always_defer_mcp_tools",
    label: "MCP 工具按需加载",
    description: "减少初始工具上下文；首次调用相关工具可能稍慢。",
  },
  {
    configKey: "standaloneWebSearch",
    flag: "standalone_web_search",
    label: "独立网页搜索",
    description: "使用独立网页搜索链路，仅影响网页搜索任务。",
  },
  {
    configKey: "enableRequestCompression",
    flag: "enable_request_compression",
    label: "请求压缩",
    description: "压缩发送给服务端的请求，减少网络传输量。",
  },
  {
    configKey: "remoteCompactionV2",
    flag: "remote_compaction_v2",
    label: "远程上下文压缩 v2",
    description: "在长会话中远程压缩上下文，降低上下文开销。",
  },
  {
    configKey: "applyPatchStreamingEvents",
    flag: "apply_patch_streaming_events",
    label: "补丁流式事件",
    description: "流式展示补丁应用进度和结果。",
  },
  {
    configKey: "concurrentReasoningSummaries",
    flag: "concurrent_reasoning_summaries",
    label: "并发推理摘要",
    description: "并发生成推理摘要，减少结果等待时间。",
  },
] as const satisfies ReadonlyArray<{
  configKey: keyof Config["experimentalFeatures"];
  flag: string;
  label: string;
  description: string;
}>;
type ExperimentalFeaturesCardProps = {
  config: Config;
  status: RuntimeStatus;
  busy: string | null;
  isBusy: boolean;
  onConfigChange: (config: Config) => void;
  onSyncOfficial: () => void;
};

function ExperimentalFeaturesCardComponent({
  config,
  status,
  busy,
  isBusy,
  onConfigChange,
  onSyncOfficial,
}: ExperimentalFeaturesCardProps) {
  const runtimeCheck = status.experimentalFeatureRuntime;
  const effectiveFeatures = runtimeCheck?.effectiveFeatures;
  const mismatchedFeatures = effectiveFeatures
    ? EXPERIMENTAL_FEATURES.filter(
        (feature) =>
          effectiveFeatures[feature.configKey] !==
          config.experimentalFeatures[feature.configKey],
      )
    : [];
  const runtimeCheckTone = !status.running
    ? "idle"
    : runtimeCheck?.status === "error"
      ? "error"
      : effectiveFeatures
        ? mismatchedFeatures.length > 0
          ? "warning"
          : "success"
        : "idle";
  const checkedAt = runtimeCheck?.updatedAt
    ? new Date(runtimeCheck.updatedAt).toLocaleTimeString("zh-CN", {
        hour12: false,
      })
    : "";
  const runtimeCheckText = !status.running
    ? "Codex 未运行，无法读取运行态"
    : runtimeCheckTone === "success"
      ? `已生效：运行态与当前页面配置一致${checkedAt ? `（${checkedAt}）` : ""}`
      : runtimeCheckTone === "warning"
        ? `未一致：${mismatchedFeatures
            .slice(0, 3)
            .map((feature) => feature.label)
            .join(
              "、",
            )}${mismatchedFeatures.length > 3 ? " 等" : ""} 与当前页面配置不同；保存并重启 Codex 后再打开自检`
        : runtimeCheck?.detail || "打开配置面板时读取一次运行态，不做后台轮询";
  return (
    <section
      className="secondary-section"
      aria-labelledby="experimental-features-title"
    >
      <div className="section-title compact">
        <div>
          <h2 id="experimental-features-title">试验性功能</h2>
          <p>由用户配置固定控制；保存并重启 Codex 后生效。</p>
        </div>
        <Button
          variant="secondary"
          size="sm"
          disabled={isBusy}
          aria-busy={busy === "sync-experimental-features"}
          onClick={onSyncOfficial}
        >
          {busy === "sync-experimental-features" ? (
            <LoaderCircle className="spinner" aria-hidden="true" />
          ) : (
            <RefreshCw aria-hidden="true" />
          )}
          {busy === "sync-experimental-features" ? "同步中" : "同步官方配置"}
        </Button>
      </div>
      <Card className="secondary-card experimental-features-card">
        <div
          className={`experimental-runtime-check ${runtimeCheckTone}`}
          aria-live="polite"
        >
          <span className="experimental-runtime-check-label">自检</span>
          <span className="experimental-runtime-check-text">
            {runtimeCheckText}
          </span>
        </div>
        <div className="feature-grid">
          {EXPERIMENTAL_FEATURES.map((feature) => {
            const enabled = config.experimentalFeatures[feature.configKey];
            return (
              <div
                className={`feature-card ${enabled ? "active" : ""}`}
                key={feature.flag}
              >
                <div className="feature-card-header">
                  <div className="experimental-feature-heading">
                    <strong>{feature.label}</strong>
                    <code>{feature.flag}</code>
                  </div>
                  <Switch
                    checked={enabled}
                    disabled={isBusy}
                    onCheckedChange={(checked) =>
                      onConfigChange({
                        ...config,
                        experimentalFeatures: {
                          ...config.experimentalFeatures,
                          [feature.configKey]: checked,
                        },
                      })
                    }
                    aria-label={`${enabled ? "关闭" : "开启"}${feature.label}`}
                  />
                </div>
                <div className="feature-card-body">
                  <small>{feature.description}</small>
                </div>
              </div>
            );
          })}
        </div>
        <p className="experimental-features-note">
          “同步官方配置”只更新当前草稿；不点击同步时会一直使用已保存的用户配置。
        </p>
      </Card>
    </section>
  );
}

export const ExperimentalFeaturesCard = memo(ExperimentalFeaturesCardComponent);
