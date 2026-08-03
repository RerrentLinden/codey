import { memo } from "react";
import {
  IconCheck as Check,
  IconGitBranch as GitBranch,
  IconLoader2 as LoaderCircle,
  IconPlugConnected as PlugZap,
  IconRefresh as RefreshCw,
  IconServer as Server,
  IconTrash as Trash,
  IconX as X,
} from "@tabler/icons-react";

import type { CcSwitchStatus, ModelState } from "./App.types";
import { Badge, Button, Card, Switch } from "./components/semi";

type ModelSectionProps = {
  ccSwitchStatus: CcSwitchStatus;
  provider: CcSwitchStatus["provider"];
  modelState: ModelState;
  dirty: boolean;
  isBusy: boolean;
  busy: string | null;
  showAccountUsageInHeader: boolean;
  manualThirdPartyModelKeys: Set<string>;
  onSyncCurrentProvider: () => void;
  onFetchCurrentModels: () => void;
  onSetDefaultModel: (model: string) => void;
  onDeleteThirdPartyModel: (model: string) => void;
  onShowAccountUsageInHeaderChange: (checked: boolean) => void;
};

function ModelSectionComponent({
  ccSwitchStatus,
  provider,
  modelState,
  dirty,
  isBusy,
  busy,
  showAccountUsageInHeader,
  manualThirdPartyModelKeys,
  onSyncCurrentProvider,
  onFetchCurrentModels,
  onSetDefaultModel,
  onDeleteThirdPartyModel,
  onShowAccountUsageInHeaderChange,
}: ModelSectionProps) {
  const defaultModel = modelState.defaultModel;
  return (
    <section className="route-section" aria-labelledby="route-title">
      <div className="section-title">
        <div>
          <h2 id="route-title">线路与模型</h2>
          <p>
            {ccSwitchStatus.available
              ? "cc-switch 当前配置"
              : "本地 Codex 直登配置"}
          </p>
        </div>
        <div className="route-heading-actions">
          {provider.official && (
            <div
              className={`header-usage-toggle${showAccountUsageInHeader ? " active" : ""}`}
            >
              <span>显示额度浮窗</span>
              <Switch
                checked={showAccountUsageInHeader}
                disabled={isBusy}
                onCheckedChange={onShowAccountUsageInHeaderChange}
                aria-label="显示官方账号额度浮窗"
              />
            </div>
          )}
          <Button
            variant="outline"
            size="sm"
            disabled={dirty || isBusy}
            onClick={onSyncCurrentProvider}
          >
            <RefreshCw
              className={busy === "sync-provider" ? "spinner" : ""}
              aria-hidden="true"
            />
            同步当前线路
          </Button>
        </div>
      </div>

      <Card className="route-card">
        <div className="provider-summary">
          <div className="provider-identity">
            <span className="column-icon">
              <Server size={16} />
            </span>
            <div className="provider-name-box">
              <strong>{provider.name}</strong>
            </div>
          </div>
          <div className="provider-meta">
            <div>
              <span>类型</span>
              <strong>
                {provider.official ? "OpenAI 官方" : "第三方 API"}
              </strong>
            </div>
            <div className="provider-endpoint">
              <span>地址</span>
              <strong>
                {provider.official ? "ChatGPT 登录" : provider.baseUrl}
              </strong>
            </div>
            <div>
              <span>默认模型</span>
              <strong>{defaultModel || "未配置"}</strong>
            </div>
          </div>
        </div>

        <div className="catalog-workspace">
          <div className="catalog-heading">
            <div className="column-heading">
              <span className="column-icon">
                <GitBranch size={16} />
              </span>
              <div>
                <strong>模型列表</strong>
                <small>
                  {provider.official
                    ? "官方目录"
                    : `已确认 ${
                        modelState.officialModels.filter((model) => model.supported).length +
                        modelState.thirdPartyModels.length
                      } 个可用模型`}
                </small>
              </div>
            </div>
            {!provider.official && (
              <Button
                variant="secondary"
                size="sm"
                disabled={isBusy}
                onClick={onFetchCurrentModels}
              >
                <RefreshCw
                  className={busy === "fetch-models" ? "spinner" : ""}
                  aria-hidden="true"
                />
                同步模型
              </Button>
            )}
          </div>

          <div className="model-groups">
            <section className="model-group">
              <div className="model-group-title">
                <div>
                  <strong>官方模型</strong>
                  <small>
                    {
                      modelState.officialModels.filter(
                        (model) => model.supported,
                      ).length
                    }{" "}
                    / {modelState.officialModels.length}
                  </small>
                </div>
                <Badge variant="info">OpenAI</Badge>
              </div>
              <div className="catalog-model-list">
                {modelState.officialModels.map((model) => {
                  const isDefault = defaultModel === model.slug;
                  return (
                    <div
                      className={`catalog-model-row${model.supported ? "" : " unsupported"}${isDefault ? " default-model" : ""}`}
                      key={model.slug}
                      aria-disabled={!model.supported}
                    >
                      <span className="model-availability">
                        {model.supported ? (
                          <Check size={12} />
                        ) : (
                          <X size={12} />
                        )}
                      </span>
                      <div>
                        <strong>{model.displayName}</strong>
                        <small>{model.slug}</small>
                      </div>
                      <div className="catalog-model-actions">
                        {isDefault && <Badge variant="brand">默认</Badge>}
                        {model.supported && !isDefault && (
                          <Button
                            variant="ghost"
                            size="xs"
                            disabled={isBusy}
                            onClick={() => onSetDefaultModel(model.slug)}
                          >
                            设为默认
                          </Button>
                        )}
                      </div>
                    </div>
                  );
                })}
                {modelState.officialModels.length === 0 && (
                  <div className="empty-state">暂未读取到官方模型</div>
                )}
              </div>
            </section>

            {!provider.official && (
              <section className="model-group">
                <div className="model-group-title">
                  <div>
                    <strong>其他模型</strong>
                    <small>{modelState.thirdPartyModels.length}</small>
                  </div>
                  <Badge variant="brand">API</Badge>
                </div>
                <div
                  className={
                    modelState.thirdPartyModels.length === 0
                      ? "catalog-model-list catalog-model-list-empty"
                      : "catalog-model-list"
                  }
                >
                  {modelState.thirdPartyModels.map((model) => {
                    const isDefault = defaultModel === model;
                    return (
                      <div
                        className={`catalog-model-row third-party${isDefault ? " default-model" : ""}`}
                        key={model}
                      >
                        <span className="model-availability">
                          <Check size={12} />
                        </span>
                        <div>
                          <strong>{model}</strong>
                          <small>当前线路模型</small>
                        </div>
                        <div className="catalog-model-actions">
                          {isDefault && <Badge variant="brand">默认</Badge>}
                          {!isDefault && (
                            <Button
                              variant="ghost"
                              size="xs"
                              disabled={isBusy}
                              onClick={() => onSetDefaultModel(model)}
                            >
                              设为默认
                            </Button>
                          )}
                          {manualThirdPartyModelKeys.has(model.toLowerCase()) && (
                            <Button
                              variant="ghost"
                              size="xs"
                              className="catalog-model-delete"
                              disabled={isBusy}
                              onClick={() => onDeleteThirdPartyModel(model)}
                              aria-label={`删除其他模型 ${model}`}
                            >
                              <Trash aria-hidden="true" />
                              删除
                            </Button>
                          )}
                        </div>
                      </div>
                    );
                  })}
                  {modelState.thirdPartyModels.length === 0 && (
                    <div
                      className="catalog-empty-state"
                      role="region"
                      aria-labelledby="third-party-empty-title"
                      aria-busy={busy === "fetch-models"}
                    >
                      <span className="catalog-empty-icon" aria-hidden="true">
                        <PlugZap size={22} />
                      </span>
                      <div className="catalog-empty-copy">
                        <strong id="third-party-empty-title">
                          尚未添加其他模型
                        </strong>
                        <p>可同步发现，也可手动输入模型 ID 添加。</p>
                      </div>
                      <Button
                        variant="secondary"
                        size="sm"
                        disabled={isBusy}
                        onClick={onFetchCurrentModels}
                      >
                        {busy === "fetch-models" ? (
                          <LoaderCircle
                            className="spinner"
                            aria-hidden="true"
                          />
                        ) : (
                          <RefreshCw aria-hidden="true" />
                        )}
                        {busy === "fetch-models" ? "同步中" : "同步或手动配置"}
                      </Button>
                    </div>
                  )}
                </div>
              </section>
            )}
          </div>
        </div>

        <div className="readonly-note">
          <Server size={14} />
          <span>
            {ccSwitchStatus.available
              ? "线路配置由 cc-switch 管理，Codey 仅在启动时读取当前线路"
              : "未安装 cc-switch，Codey 读取本地 Codex 登录与 API 配置"}
          </span>
          <Badge variant="secondary">只读</Badge>
        </div>
      </Card>
    </section>
  );
}

export const ModelSection = memo(ModelSectionComponent);
