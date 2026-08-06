import { memo, type CSSProperties } from "react";

import type { Config } from "./App.types";
import { Badge, Card, Switch } from "./components/semi";
import type { SubagentModelOption } from "./useModelSelection";

const GPU_LAUNCH_MODES = [
  { value: "off", label: "关闭" },
  { value: "disableGpu", label: "禁用 GPU" },
  { value: "disableGpuRasterization", label: "禁用 GPU 栅格化" },
] as const satisfies ReadonlyArray<{
  value: Config["gpuLaunchMode"];
  label: string;
}>;
const REASONING_EFFORT_LABELS: Record<string, string> = {
  low: "低",
  medium: "中",
  high: "高",
  xhigh: "极高",
  max: "最大",
  ultra: "超高",
};

type FeaturePolicyCardProps = {
  config: Config;
  isMacClient: boolean;
  busy: string | null;
  isBusy: boolean;
  subagentModelOptions: SubagentModelOption[];
  onConfigChange: (config: Config) => void;
  onSubagentOptimizationChange: (checked: boolean) => void;
};

function FeaturePolicyCardComponent({
  config,
  isMacClient,
  busy,
  isBusy,
  subagentModelOptions,
  onConfigChange,
  onSubagentOptimizationChange,
}: FeaturePolicyCardProps) {
  const configuredGpuLaunchModeIndex = GPU_LAUNCH_MODES.findIndex(
    ({ value }) => value === config.gpuLaunchMode,
  );
  const gpuLaunchModeIndex = isMacClient
    ? 0
    : Math.max(configuredGpuLaunchModeIndex, 0);
  const gpuLaunchMode = GPU_LAUNCH_MODES[gpuLaunchModeIndex];
  const gpuLaunchModeStyle = {
    "--gpu-mode-offset": `${gpuLaunchModeIndex * 100}%`,
  } as CSSProperties;
  const selectedSubagentModel = subagentModelOptions.find(
    (option) =>
      option.value.trim().toLowerCase() ===
      config.subagentModel.trim().toLowerCase(),
  );
  const subagentReasoningEfforts =
    selectedSubagentModel?.supportedReasoningEfforts ?? [];
  const subagentPolicyControlsDisabled =
    isBusy || !config.subagentOptimization;

  return (
    <section className="secondary-section" aria-labelledby="runtime-title">
      <div className="section-title compact">
        <div>
          <h2 id="runtime-title">Codex 功能策略</h2>
          <p>按需精简客户端模块和界面行为。</p>
        </div>
      </div>
      <Card className="secondary-card runtime-card">
        <div className="feature-grid">
          <div
            className={`feature-card gpu-mode-card ${!isMacClient && gpuLaunchMode.value !== "off" ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <div className="feature-card-title">
                <strong>GPU 渲染模式</strong>
                <Badge variant={isMacClient ? "secondary" : "warning"}>
                  {isMacClient ? "macOS 不可用" : "实验性"}
                </Badge>
              </div>
            </div>
            <div className="feature-card-body gpu-mode-card-body">
              <fieldset
                className="gpu-mode-fieldset"
                disabled={isMacClient}
                aria-describedby="gpu-launch-mode-description"
              >
                <legend className="sr-only">Codex GPU 启动模式</legend>
                <div className="gpu-mode-slider" style={gpuLaunchModeStyle}>
                  <span className="gpu-mode-slider-thumb" aria-hidden="true" />
                  {GPU_LAUNCH_MODES.map((mode) => (
                    <label
                      key={mode.value}
                      className={`gpu-mode-option ${gpuLaunchMode.value === mode.value ? "selected" : ""}`}
                    >
                      <input
                        type="radio"
                        name="codey-gpu-launch-mode"
                        value={mode.value}
                        checked={gpuLaunchMode.value === mode.value}
                        onChange={() =>
                          onConfigChange({
                            ...config,
                            gpuLaunchMode: mode.value,
                          })
                        }
                      />
                      <span>{mode.label}</span>
                    </label>
                  ))}
                </div>
              </fieldset>
              <small id="gpu-launch-mode-description" aria-live="polite">
                {isMacClient
                  ? "macOS 下已禁用，不会向 Codex 传递 GPU 诊断参数"
                  : gpuLaunchMode.value === "disableGpu"
                    ? "启动 Codex 时附加 --disable-gpu；可能增加 CPU 占用"
                    : gpuLaunchMode.value === "disableGpuRasterization"
                      ? "启动 Codex 时附加 --disable-gpu-rasterization；仅将栅格化移到 CPU"
                      : "保持 Codex 默认 GPU 渲染，不附加诊断参数"}
              </small>
            </div>
          </div>

          <div
            className={`feature-card subagent-policy-card ${config.subagentOptimization ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <div className="feature-card-title">
                <strong>子代理协作优化</strong>
                <Badge variant="warning">模型与深度可配置</Badge>
              </div>
              <Switch
                checked={config.subagentOptimization}
                disabled={isBusy}
                aria-busy={busy === "check-subagent-model"}
                onCheckedChange={(checked) =>
                  onSubagentOptimizationChange(checked)
                }
                aria-label="启用子代理协作优化"
              />
            </div>
            <div className="feature-card-body subagent-policy-body">
              <div className="subagent-policy-controls">
                <label>
                  <span>子代理模型</span>
                  <select
                    aria-label="选择子代理模型"
                    value={selectedSubagentModel?.value ?? ""}
                    disabled={
                      subagentPolicyControlsDisabled ||
                      subagentModelOptions.length === 0
                    }
                    onChange={(event) => {
                      const option = subagentModelOptions.find(
                        (candidate) => candidate.value === event.target.value,
                      );
                      if (!option) return;
                      const reasoningEffort =
                        option.supportedReasoningEfforts.includes(
                          config.subagentReasoningEffort,
                        )
                          ? config.subagentReasoningEffort
                          : option.defaultReasoningEffort;
                      onConfigChange({
                        ...config,
                        subagentModel: option.value,
                        subagentReasoningEffort: reasoningEffort,
                      });
                    }}
                  >
                    {!selectedSubagentModel && (
                      <option value="">
                        {subagentModelOptions.length === 0
                          ? "当前线路无兼容模型"
                          : "请选择兼容模型"}
                      </option>
                    )}
                    {subagentModelOptions.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  <span>思考深度</span>
                  <select
                    aria-label="选择子代理思考深度"
                    value={
                      subagentReasoningEfforts.includes(
                        config.subagentReasoningEffort,
                      )
                        ? config.subagentReasoningEffort
                        : ""
                    }
                    disabled={
                      subagentPolicyControlsDisabled ||
                      subagentReasoningEfforts.length === 0
                    }
                    onChange={(event) =>
                      onConfigChange({
                        ...config,
                        subagentReasoningEffort: event.target.value,
                      })
                    }
                  >
                    {subagentReasoningEfforts.map((effort) => (
                      <option key={effort} value={effort}>
                        {REASONING_EFFORT_LABELS[effort] ?? effort}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              <small>
                {busy === "check-subagent-model"
                  ? `正在校验当前线路是否支持 ${config.subagentModel}`
                  : subagentModelOptions.length === 0
                    ? "当前 Codex 版本或线路没有可用于子代理的模型"
                  : config.subagentOptimization
                    ? "保存后立即用于当前任务后续新启动的子代理，无需重启"
                    : "保持 Codex 默认子代理配置，不注入协作提示词"}
              </small>
            </div>
          </div>

          <div
            className={`feature-card ${config.slimCodexPet ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <strong>精简 Codex 宠物模块</strong>
              <Switch
                checked={config.slimCodexPet}
                onCheckedChange={(checked) =>
                  onConfigChange({ ...config, slimCodexPet: checked })
                }
                aria-label="精简 Codex 宠物模块"
              />
            </div>
            <div className="feature-card-body">
              <small>
                {config.slimCodexPet
                  ? "已停止宠物窗口和相关运行时模块"
                  : "保留 Codex 宠物的完整功能"}
              </small>
            </div>
          </div>

          <div
            className={`feature-card ${config.slimCodexVoice ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <strong>精简 Codex 语音模块</strong>
              <Switch
                checked={config.slimCodexVoice}
                onCheckedChange={(checked) =>
                  onConfigChange({ ...config, slimCodexVoice: checked })
                }
                aria-label="精简 Codex 语音模块"
              />
            </div>
            <div className="feature-card-body">
              <small>
                {config.slimCodexVoice
                  ? "已停止听写、GPT Voice、快捷键与音频初始化"
                  : "保留 Codex 语音的完整功能"}
              </small>
            </div>
          </div>

          <div
            className={`feature-card ${config.fastCodexStartup ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <strong>Codex 慢启动保护</strong>
              <Switch
                checked={config.fastCodexStartup}
                onCheckedChange={(checked) =>
                  onConfigChange({
                    ...config,
                    fastCodexStartup: checked,
                  })
                }
                aria-label="启用 Codex 慢启动保护"
              />
            </div>
            <div className="feature-card-body">
              <small>
                {config.fastCodexStartup
                  ? "远程启动配置超过 1.5 秒时快速降级；正常响应保持原流程"
                  : "完全使用 Codex 原生远程启动等待策略"}
              </small>
            </div>
          </div>

          <div
            className={`feature-card ${config.fastContextTools ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <div className="feature-card-title">
                <strong>FastCtx 上下文工具</strong>
                <Badge variant="secondary">v0.2.4</Badge>
              </div>
              <Switch
                checked={config.fastContextTools}
                onCheckedChange={(checked) =>
                  onConfigChange({ ...config, fastContextTools: checked })
                }
                aria-label="启用 FastCtx 上下文工具"
              />
            </div>
            <div className="feature-card-body">
              <small>
                {config.fastContextTools
                  ? "优先复用已配置的 FastCtx；未配置时加载 Codey 内置工具"
                  : "保持 Codex 默认文件工具，不加载额外 MCP"}
              </small>
            </div>
          </div>

          <div
            className={`feature-card ${config.disableTraceLogWrites ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <strong>Trace 日志写盘保护</strong>
              <Switch
                checked={config.disableTraceLogWrites}
                onCheckedChange={(checked) =>
                  onConfigChange({
                    ...config,
                    disableTraceLogWrites: checked,
                  })
                }
                aria-label="启用 Codex Trace 日志写盘保护"
              />
            </div>
            <div className="feature-card-body">
              <small>阻止Trace日志持续写入数据库影响硬盘寿命</small>
            </div>
          </div>

          {isMacClient && (
            <div
              className={`feature-card ${config.protectCrashpadPending ? "active" : ""}`}
            >
              <div className="feature-card-header">
                <strong>Crashpad 磁盘保护</strong>
                <Switch
                  checked={config.protectCrashpadPending}
                  onCheckedChange={(checked) =>
                    onConfigChange({
                      ...config,
                      protectCrashpadPending: checked,
                    })}
                  aria-label="启用 Codex Crashpad 磁盘保护"
                />
              </div>
              <div className="feature-card-body">
                <small>
                  {config.protectCrashpadPending
                    ? "待处理崩溃报告超过安全上限时自动收敛，并保留最近写入"
                    : "仅显示占用和提供手动清理，不执行自动容量保护"}
                </small>
              </div>
            </div>
          )}

          <div
            className={`feature-card ${config.hideFullAccessWarning ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <strong>屏蔽完全访问安全提示</strong>
              <Switch
                checked={config.hideFullAccessWarning}
                onCheckedChange={(checked) =>
                  onConfigChange({ ...config, hideFullAccessWarning: checked })
                }
                aria-label="屏蔽完全访问安全提示"
              />
            </div>
            <div className="feature-card-body">
              <small>
                {config.hideFullAccessWarning
                  ? "自动隐藏完全访问模式的原生安全提示"
                  : "保留 Codex 原生安全提示"}
              </small>
            </div>
          </div>
        </div>
      </Card>
    </section>
  );
}

export const FeaturePolicyCard = memo(FeaturePolicyCardComponent);
