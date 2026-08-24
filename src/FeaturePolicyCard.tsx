import { memo, useMemo, type CSSProperties } from "react";
import { IconAdjustmentsHorizontal, IconInfoCircle, IconUsersGroup } from "@tabler/icons-react";

import type {
  Config,
  FastContextToolsStatus,
  SubagentRoleId,
} from "./App.types";
import {
  ActionIcon,
  Badge,
  Card,
  Select,
  Switch,
  Table,
  Tooltip,
} from "./components/mantine";
import { compactSelectInputClass, surfaceCardPaddingClass } from "./uiClasses";
import { SETTINGS_OVERLAY_Z_INDEX } from "./overlay.constants";
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
const SUBAGENT_TASK_TYPES = [
  {
    id: "codey_quick_scan",
    name: "快速定位",
    description: "默认只读；用于精确位置、重复性检查、低风险事实查找和小范围快速检索。",
  },
  {
    id: "codey_deep_research",
    name: "深度检索",
    description: "默认只读；用于跨文件、日志、代码和文档的宽范围检索、归纳与架构探索。",
  },
  {
    id: "codey_visual_analysis",
    name: "视觉分析",
    description: "默认只读；仅用于必须读取截图、页面、GUI、PDF 或渲染结果的视觉证据分析。",
  },
  {
    id: "codey_worker",
    name: "代码实施",
    description: "默认可写；用于边界清晰、可回滚、可测试的低到中等复杂度非视觉实现。",
  },
  {
    id: "codey_visual_worker",
    name: "视觉实施",
    description: "默认可写；用于页面、GUI、PDF 或其他依赖视觉证据和渲染验证的实现。",
  },
] as const satisfies ReadonlyArray<{
  id: SubagentRoleId;
  name: string;
  description: string;
}>;

export type SubagentPolicyCardProps = {
  config: Config;
  popupContainer: HTMLElement | null;
  tooltipContainer: HTMLElement | null;
  isBusy: boolean;
  subagentModelOptions: SubagentModelOption[];
  onConfigChange: (config: Config) => void;
  onSubagentOptimizationChange: (checked: boolean) => void;
};

export function SubagentPolicyCardComponent({
  config,
  popupContainer,
  tooltipContainer,
  isBusy,
  subagentModelOptions,
  onConfigChange,
  onSubagentOptimizationChange,
}: SubagentPolicyCardProps) {
  const subagentPolicyControlsDisabled = isBusy;
  const subagentModelSelectOptions = useMemo(
    () =>
      subagentModelOptions.map((option) => ({
        label: option.label,
        value: option.value,
      })),
    [subagentModelOptions],
  );

  return (
    <section className="secondary-section subagent-section" aria-labelledby="subagent-title">
      <div className="section-title compact">
        <div className="section-heading">
          <span className="section-icon" aria-hidden="true">
            <IconUsersGroup size={15} />
          </span>
          <div>
            <h2 id="subagent-title">Codey 子代理角色与调度增强</h2>
            <p>基于 Codex 原生子代理的多角色调度与模型配置。</p>
          </div>
        </div>
      </div>
      <Card className={`secondary-card subagent-card ${surfaceCardPaddingClass}`}>
        <div className={`feature-card subagent-toggle-card ${config.subagentOptimization ? "active" : ""}`}>
          <div className="feature-card-header">
            <div className="feature-card-title">
              <strong>启用 Codey 子代理角色与调度增强</strong>
            </div>
            <Switch
              checked={config.subagentOptimization}
              disabled={isBusy}
              onCheckedChange={(checked) =>
                onSubagentOptimizationChange(checked)
              }
              aria-label="启用 Codey 子代理角色与调度增强"
            />
          </div>
        </div>
        <div className="subagent-policy-body">
          {config.subagentOptimization ? (
            <>
              <Table.ScrollContainer minWidth={360}>
                <Table
                  verticalSpacing="xs"
                  horizontalSpacing="xs"
                  withRowBorders
                  withTableBorder
                  className="overflow-hidden rounded-[9px] border border-black/7 bg-[#f8f8fa]/72"
                >
                  <Table.Thead className="text-[#8e8e93] text-[11px] font-semibold tracking-[0.02em]">
                    <Table.Tr>
                      <Table.Th style={{ width: "28%" }}>任务类型</Table.Th>
                      <Table.Th style={{ width: "44%" }}>模型</Table.Th>
                      <Table.Th style={{ width: "28%" }}>思考深度</Table.Th>
                    </Table.Tr>
                  </Table.Thead>
                  <Table.Tbody className="[&_td]:align-middle [&_th]:align-middle">
                    {SUBAGENT_TASK_TYPES.map((task) => {
                      const selection = config.subagentRoles[task.id] ?? {
                        model: config.subagentModel,
                        reasoningEffort: config.subagentReasoningEffort,
                      };
                      const selectedModel = subagentModelOptions.find(
                        (option) =>
                          option.value.trim().toLowerCase() ===
                          selection.model.trim().toLowerCase(),
                      );
                      const reasoningEfforts =
                        selectedModel?.supportedReasoningEfforts ?? [];
                      const reasoningOptions = reasoningEfforts.map((effort) => ({
                        label: REASONING_EFFORT_LABELS[effort] ?? effort,
                        value: effort,
                      }));
                      const updateRole = (
                        model: string,
                        reasoningEffort: string,
                      ) => {
                        onConfigChange({
                          ...config,
                          subagentRoles: {
                            ...config.subagentRoles,
                            [task.id]: { model, reasoningEffort },
                          },
                        });
                      };

                      return (
                        <Table.Tr key={task.id}>
                          <Table.Td>
                            <div className="flex items-center gap-1 text-xs font-semibold text-[#3a3a3c] whitespace-nowrap">
                              <span>{task.name}</span>
                              <Tooltip
                                content={task.description}
                                getPopupContainer={() =>
                                  popupContainer ?? tooltipContainer ?? document.body
                                }
                                position="top"
                                zIndex={SETTINGS_OVERLAY_Z_INDEX}
                              >
                                <ActionIcon
                                  variant="subtle"
                                  color="gray"
                                  size="xs"
                                  radius="sm"
                                  aria-label={`${task.name}：${task.description}`}
                                >
                                  <IconInfoCircle size={15} aria-hidden="true" />
                                </ActionIcon>
                              </Tooltip>
                            </div>
                          </Table.Td>
                          <Table.Td>
                            <Select
                              className="w-full min-w-0"
                              inputClassName={compactSelectInputClass}
                              sectionClassName="text-[#6e6e73]"
                              aria-label={`${task.name}模型`}
                              value={selectedModel?.value}
                              placeholder={
                                subagentModelOptions.length === 0
                                  ? "当前线路暂无模型"
                                  : "请选择模型"
                              }
                              disabled={
                                subagentPolicyControlsDisabled ||
                                subagentModelOptions.length === 0
                              }
                              optionList={subagentModelSelectOptions}
                              dropdownClassName="rounded-[10px]"
                              showClear={false}
                              filter={false}
                              getPopupContainer={() => popupContainer ?? document.body}
                              zIndex={SETTINGS_OVERLAY_Z_INDEX}
                              onChange={(value) => {
                                const option = subagentModelOptions.find(
                                  (candidate) =>
                                    candidate.value === String(value ?? ""),
                                );
                                if (!option) return;
                                const reasoningEffort =
                                  option.supportedReasoningEfforts.includes(
                                    selection.reasoningEffort,
                                  )
                                    ? selection.reasoningEffort
                                    : option.defaultReasoningEffort;
                                updateRole(option.value, reasoningEffort);
                              }}
                            />
                          </Table.Td>
                          <Table.Td>
                            <Select
                              className="w-full min-w-0"
                              inputClassName={compactSelectInputClass}
                              sectionClassName="text-[#6e6e73]"
                              aria-label={`${task.name}思考深度`}
                              value={
                                reasoningEfforts.includes(selection.reasoningEffort)
                                  ? selection.reasoningEffort
                                  : undefined
                              }
                              placeholder="暂无可选深度"
                              disabled={
                                subagentPolicyControlsDisabled ||
                                reasoningEfforts.length === 0
                              }
                              optionList={reasoningOptions}
                              dropdownClassName="rounded-[10px]"
                              showClear={false}
                              filter={false}
                              getPopupContainer={() => popupContainer ?? document.body}
                              zIndex={SETTINGS_OVERLAY_Z_INDEX}
                              onChange={(value) =>
                                updateRole(
                                  selection.model,
                                  String(value ?? ""),
                                )
                              }
                            />
                          </Table.Td>
                        </Table.Tr>
                      );
                    })}
                  </Table.Tbody>
                </Table>
              </Table.ScrollContainer>
              <small>
                {subagentModelOptions.length === 0
                  ? "请先在模型管理中添加当前线路可用模型"
                  : "首次开启需重启；模型或思考深度保存后对下一次派生效。角色权限是默认值，实际受父任务权限模式约束"}
              </small>
            </>
          ) : (
            <small>开启后仅在宽范围、可并行或需要专门证据时选择性委派，并提供五类专用角色与汇合门禁；不会扩大父任务权限</small>
          )}
        </div>
      </Card>
    </section>
  );
}

export const SubagentPolicyCard = memo(SubagentPolicyCardComponent);

type FeaturePolicyCardProps = {
  config: Config;
  fastContextToolsStatus: FastContextToolsStatus;
  isMacClient: boolean;
  isWindowsClient: boolean;
  popupContainer: HTMLElement | null;
  tooltipContainer: HTMLElement | null;
  isBusy: boolean;
  onConfigChange: (config: Config) => void;
};

function FeaturePolicyCardComponent({
  config,
  fastContextToolsStatus,
  isMacClient,
  isWindowsClient,
  popupContainer,
  tooltipContainer,
  isBusy,
  onConfigChange,
}: FeaturePolicyCardProps) {
  const configuredGpuLaunchModeIndex = GPU_LAUNCH_MODES.findIndex(
    ({ value }) => value === config.gpuLaunchMode,
  );
  const gpuLaunchModeIndex = Math.max(configuredGpuLaunchModeIndex, 0);
  const gpuLaunchMode = GPU_LAUNCH_MODES[gpuLaunchModeIndex];
  const gpuLaunchModeStyle = {
    "--gpu-mode-offset": `${gpuLaunchModeIndex * 100}%`,
  } as CSSProperties;
  const fastctxStatusBlocksEmbedded =
    fastContextToolsStatus.userConfigured ||
    fastContextToolsStatus.detectionFailed;
  const fastContextToolsEnabled =
    config.fastContextTools && !fastctxStatusBlocksEmbedded;
  const fastctxBlockedReason = fastContextToolsStatus.detectionFailed
    ? "暂时无法确认 Codex 配置中的 FastCtx 状态，为避免重复加载，Codey 内置 FastCtx 不可开启"
    : fastContextToolsStatus.userConfigured
      ? `已检测到 Codex 配置中的 FastCtx${
          fastContextToolsStatus.serverId
            ? `（${fastContextToolsStatus.serverId}）`
            : ""
        }，为避免重复加载，Codey 内置 FastCtx 不可开启`
      : "";
  const fastContextToolsSwitch = (
    <Switch
      checked={fastContextToolsEnabled}
      disabled={isBusy || fastctxStatusBlocksEmbedded}
      onCheckedChange={(checked) =>
        onConfigChange({ ...config, fastContextTools: checked })
      }
      aria-label="启用 FastCtx 上下文工具"
    />
  );

  return (
    <section className="secondary-section" aria-labelledby="runtime-title">
      <div className="section-title compact">
        <div className="section-heading">
          <span className="section-icon" aria-hidden="true">
            <IconAdjustmentsHorizontal size={15} />
          </span>
          <div>
            <h2 id="runtime-title">Codex 功能策略</h2>
            <p>按需精简客户端模块和界面行为。</p>
          </div>
        </div>
      </div>
      <Card className={`secondary-card runtime-card ${surfaceCardPaddingClass}`}>
        <div className="feature-grid">
          {/* GPU 渲染模式：占满整行全宽，仅 Windows 客户端展示 */}
          {isWindowsClient && (
            <div
              className={`feature-card gpu-mode-card full-width-card ${gpuLaunchMode.value !== "off" ? "active" : ""}`}
            >
              <div className="feature-card-header">
                <div className="feature-card-title">
                  <strong>GPU 渲染模式</strong>
                  <Badge variant="warning">实验性</Badge>
                </div>
              </div>
              <div className="feature-card-body gpu-mode-card-body">
                <fieldset
                  className="gpu-mode-fieldset"
                  disabled={isBusy}
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
                  {gpuLaunchMode.value === "disableGpu"
                    ? "启动 Codex 时附加 --disable-gpu；可能增加 CPU 占用"
                    : gpuLaunchMode.value === "disableGpuRasterization"
                      ? "启动 Codex 时附加 --disable-gpu-rasterization；仅将栅格化移到 CPU"
                      : "保持 Codex 默认 GPU 渲染，不附加诊断参数"}
                </small>
              </div>
            </div>
          )}

          <div
            className={`feature-card ${config.slimCodexPet ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <strong>精简 Codex 宠物模块</strong>
              <Switch
                checked={config.slimCodexPet}
                disabled={isBusy}
                onCheckedChange={(checked) =>
                  onConfigChange({ ...config, slimCodexPet: checked })
                }
                aria-label="精简 Codex 宠物模块"
              />
            </div>
            <div className="feature-card-body">
              <small>
                {config.slimCodexPet
                  ? "已收起宠物并取消隐藏窗口预热；语音功能仍按需启用"
                  : "保留 Codex 宠物的完整功能"}
              </small>
            </div>
          </div>

          <div
            className={`feature-card ${fastContextToolsEnabled ? "active" : ""}`}
          >
            <div className="feature-card-header">
              <div className="feature-card-title">
                <strong>FastCtx 上下文工具</strong>
                <Badge variant="secondary">v0.2.6</Badge>
              </div>
              {fastctxStatusBlocksEmbedded ? (
                <Tooltip
                  content={fastctxBlockedReason}
                  getPopupContainer={() =>
                    popupContainer ?? tooltipContainer ?? document.body
                  }
                  position="top"
                  zIndex={SETTINGS_OVERLAY_Z_INDEX}
                >
                  <span
                    className="fastctx-disabled-switch-tooltip"
                    tabIndex={0}
                    aria-label={fastctxBlockedReason}
                  >
                    {fastContextToolsSwitch}
                  </span>
                </Tooltip>
              ) : (
                fastContextToolsSwitch
              )}
            </div>
            <div className="feature-card-body">
              <small>
                {fastctxStatusBlocksEmbedded
                  ? fastContextToolsStatus.detectionFailed
                    ? "暂时无法确认 FastCtx 状态，内置工具保持关闭"
                    : "已检测到已配置的 FastCtx，Codey 不会重复加载内置工具"
                  : config.fastContextTools
                    ? "下次启动加载 Codey 内置 FastCtx 文件工具"
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
                disabled={isBusy}
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
                  disabled={isBusy}
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
                disabled={isBusy}
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
