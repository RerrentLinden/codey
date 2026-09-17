import { memo } from "react";
import { Card } from "@heroui/react";
import { IconInfoCircle, IconSparkles } from "@tabler/icons-react";

import type { Config } from "./App.types";
import { Button } from "./components/ui";
import { ModelCombobox } from "./components/ModelCombobox";
import { routeProviderId } from "./modelRoutes";
import type { SubagentModelOption } from "./subagentModels";
import { flushCardClass } from "./uiClasses";

type MiscModelCardProps = {
  config: Config;
  isBusy: boolean;
  subagentModelOptions: SubagentModelOption[];
  onConfigChange: (config: Config) => void;
};

function MiscModelCardComponent({
  config,
  isBusy,
  subagentModelOptions,
  onConfigChange,
}: MiscModelCardProps) {
  const preferredProfile =
    config.profiles.find((profile) => profile.id === config.activeProfileId) ??
    config.profiles[0];
  const preferredProviderId = preferredProfile
    ? routeProviderId(preferredProfile)
    : undefined;
  const controlsDisabled = isBusy || subagentModelOptions.length === 0;

  return (
    <section className="secondary-section misc-model-section" aria-labelledby="misc-model-title">
      <Card className={`secondary-card misc-model-card ${flushCardClass}`}>
        <div className="module-card-header">
          <div className="module-card-heading">
            <span className="module-card-icon" aria-hidden="true">
              <IconSparkles size={15} />
            </span>
            <div className="module-card-titles">
              <h2 id="misc-model-title">杂事模型</h2>
              <p>统一指定会话命名、Git 提交消息与自动复核回退使用的模型。</p>
            </div>
          </div>
          <div className="module-card-action">
            <Button
              variant="ghost"
              size="sm"
              disabled={isBusy || config.miscModel.trim() === ""}
              onClick={() => onConfigChange({ ...config, miscModel: "" })}
            >
              恢复默认
            </Button>
          </div>
        </div>
        <div className="module-card-body misc-model-body">
          <div className="misc-model-field">
            <label htmlFor="misc-model-select" className="misc-model-field-label">
              指定模型
            </label>
            <div className="misc-model-field-control">
              <ModelCombobox
                aria-label="杂事模型"
                value={config.miscModel}
                placeholder={
                  subagentModelOptions.length === 0
                    ? "所有线路均暂无模型"
                    : "请选择模型"
                }
                disabled={controlsDisabled}
                options={subagentModelOptions}
                preferredProviderId={preferredProviderId}
                onChange={(value) => {
                  if (!subagentModelOptions.some((option) => option.value === value)) {
                    return;
                  }
                  onConfigChange({ ...config, miscModel: value });
                }}
              />
            </div>
          </div>
          <div className="subagent-policy-callout">
            <IconInfoCircle size={14} className="subagent-callout-icon" aria-hidden="true" />
            <div className="subagent-callout-text">
              留空时保持 Codex 默认行为：会话命名和 Git 提交消息使用内置的 Luna 模型，自动复核在没有可用线路时直接报错。选择模型后，会话命名、Git 提交消息和自动复核回退都改用它；只要任一线路支持 codex-auto-review，自动复核仍优先使用专用模型。会话命名与 Git 提交消息的变更需要重启 Codex 生效。
              {config.localRouterEnabled
                ? ""
                : " 本地路由已关闭，自动复核回退不会生效。"}
            </div>
          </div>
        </div>
      </Card>
    </section>
  );
}

export const MiscModelCard = memo(MiscModelCardComponent);
