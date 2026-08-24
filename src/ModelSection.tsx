import { memo, useMemo, useState } from "react";
import {
  IconCheck as Check,
  IconEdit as Edit,
  IconPlus as Plus,
  IconRefresh as RefreshCw,
  IconServer as Server,
  IconTrash as Trash,
} from "@tabler/icons-react";

import type { Config, ModelState, Profile } from "./App.types";
import {
  Badge,
  Button,
  Card,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Select,
  Switch,
} from "./components/mantine";
import { modelIdsEqual, modelKey } from "./modelIds";
import { SETTINGS_OVERLAY_Z_INDEX } from "./overlay.constants";
import { flushCardClass } from "./uiClasses";

type ModelSectionProps = {
  config: Config;
  officialAccountAvailable: boolean;
  popupContainer: HTMLElement | null;
  modelState: ModelState;
  dirty: boolean;
  isBusy: boolean;
  busy: string | null;
  showAccountUsageInHeader: boolean;
  onSyncCurrentProvider: () => void;
  onSaveRoute: (route: Profile) => Promise<boolean>;
  onDeleteRoute: (routeId: string) => void;
  onFetchRouteModels: (route: Profile) => void;
  onSaveOfficialRouteSettings: (
    routeId: string,
    models: string[],
    showAccountUsageInHeader: boolean,
  ) => Promise<boolean>;
  onSetDefaultModel: (routeId: string, model: string) => void;
};

type RouteModelGroup = {
  profile: Profile;
  providerId: string;
  models: string[];
  defaultModel: string;
  official: boolean;
};

function routeProviderId(profile: Profile) {
  return profile.ccSwitchProviderId || profile.id;
}

function uniqueModels(models: string[]) {
  const seen = new Set<string>();
  return models.filter((model) => {
    const key = modelKey(model);
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function newRouteName(profiles: Profile[]) {
  let index = profiles.length + 1;
  const names = new Set(profiles.map((profile) => profile.name));
  while (names.has(`新线路 ${index}`)) index += 1;
  return `新线路 ${index}`;
}

function createRoute(profiles: Profile[]): Profile {
  const id = `route-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  return {
    id,
    name: newRouteName(profiles),
    baseUrl: "",
    apiKey: "",
    upstreamProtocol: "openaiResponses",
    authMode: "apiKey",
    apiKeyConfigured: false,
    clearApiKey: false,
    ccSwitchReadOnly: false,
    supportsRemoteCompaction: false,
  };
}

const routeProtocolOptions: Array<{
  label: string;
  value: Profile["upstreamProtocol"];
}> = [
  { label: "OpenAI Responses", value: "openaiResponses" },
  { label: "第三方 Responses 兼容", value: "openaiCompatible" },
];

function ModelSectionComponent({
  config,
  officialAccountAvailable,
  popupContainer,
  modelState,
  dirty,
  isBusy,
  busy,
  showAccountUsageInHeader,
  onSyncCurrentProvider,
  onSaveRoute,
  onDeleteRoute,
  onFetchRouteModels,
  onSaveOfficialRouteSettings,
  onSetDefaultModel,
}: ModelSectionProps) {
  const [routeDialogOpen, setRouteDialogOpen] = useState(false);
  const [routeDraft, setRouteDraft] = useState<Profile | null>(null);
  const [officialModelDraft, setOfficialModelDraft] = useState<string[]>([]);
  const [usageDraft, setUsageDraft] = useState(showAccountUsageInHeader);
  const visibleProfiles = useMemo(
    () =>
      config.profiles.filter(
        (profile) =>
          profile.authMode !== "officialAccount" || officialAccountAvailable,
      ),
    [config.profiles, officialAccountAvailable],
  );
  const officialDisplayNames = useMemo(
    () =>
      new Map(
        modelState.officialModels.map((model) => [
          modelKey(model.slug),
          model.displayName,
        ]),
      ),
    [modelState.officialModels],
  );
  const officialCatalog = useMemo(
    () =>
      uniqueModels([
        ...modelState.officialModelIds,
        ...modelState.officialModels.map((model) => model.slug),
      ]),
    [modelState.officialModelIds, modelState.officialModels],
  );
  const officialModelDraftKeys = useMemo(
    () => new Set(officialModelDraft.map(modelKey)),
    [officialModelDraft],
  );
  const modelGroups = useMemo<RouteModelGroup[]>(
    () =>
      visibleProfiles.map((profile) => {
        const providerId = routeProviderId(profile);
        const official = profile.authMode === "officialAccount";
        const configuredModels = config.selectedModelsByProvider[providerId] || [];
        const models = official
          ? configuredModels.length > 0
            ? configuredModels
            : uniqueModels([
                ...modelState.officialModelIds,
                ...modelState.officialModels.map((model) => model.slug),
              ])
          : uniqueModels([
              ...configuredModels,
              ...(config.declaredOfficialModelsByProvider[providerId] || []),
            ]);
        return {
          profile,
          providerId,
          models,
          defaultModel:
            config.defaultModelByProvider[providerId] || models[0] || "",
          official,
        };
      }),
    [config, modelState.officialModelIds, modelState.officialModels, visibleProfiles],
  );

  const openNewRouteDialog = () => {
    setRouteDraft(createRoute(config.profiles));
    setOfficialModelDraft([]);
    setRouteDialogOpen(true);
  };
  const openEditRouteDialog = (profile: Profile) => {
    const official = profile.authMode === "officialAccount";
    setRouteDraft(
      official
        ? { ...profile }
        : { ...profile, apiKey: "", clearApiKey: false },
    );
    if (official) {
      const providerId = routeProviderId(profile);
      const configuredModels = config.selectedModelsByProvider[providerId] || [];
      setOfficialModelDraft(
        configuredModels.length > 0
          ? configuredModels
          : uniqueModels([
              ...modelState.officialModelIds,
              ...modelState.officialModels.map((model) => model.slug),
            ]),
      );
      setUsageDraft(showAccountUsageInHeader);
    }
    setRouteDialogOpen(true);
  };
  const updateRouteDraft = (patch: Partial<Profile>) => {
    setRouteDraft((current) => current ? { ...current, ...patch } : current);
  };
  const saveRouteDraft = async () => {
    if (!routeDraft) return;
    const saved = routeDraft.authMode === "officialAccount"
      ? await onSaveOfficialRouteSettings(
          routeDraft.id,
          officialModelDraft,
          usageDraft,
        )
      : await onSaveRoute(routeDraft);
    if (saved) {
      setRouteDialogOpen(false);
      setRouteDraft(null);
    }
  };

  return (
    <section className="route-section" aria-labelledby="route-title">
      <div className="section-title">
        <div className="section-heading">
          <span className="section-icon" aria-hidden="true">
            <Server size={15} />
          </span>
          <div>
            <h2 id="route-title">线路与模型</h2>
            <p>统一管理供应商线路与模型目录</p>
          </div>
        </div>
        <div className="route-heading-actions">
          <Button
            variant="outline"
            size="sm"
            disabled={dirty || isBusy}
            onClick={onSyncCurrentProvider}
          >
            <RefreshCw
              className={busy === "sync-provider" ? "animate-spin" : ""}
              aria-hidden="true"
            />
            重新读取 Codex 配置
          </Button>
        </div>
      </div>

      <Card className={`route-card ${flushCardClass}`}>
        <div className="route-manager route-manager-balanced">
          <aside className="route-list-pane" aria-label="线路列表">
            <div className="route-list-heading">
              <div>
                <strong>供应商线路</strong>
                <small>第三方线路同时接入统一路由</small>
              </div>
              <Button
                size="xs"
                variant="secondary"
                disabled={isBusy || dirty}
                onClick={openNewRouteDialog}
              >
                <Plus aria-hidden="true" />
                新增线路
              </Button>
            </div>
            <div className="route-list">
              {visibleProfiles.map((profile) => {
                const providerId = routeProviderId(profile);
                const group = modelGroups.find(
                  (candidate) => candidate.providerId === providerId,
                );
                return (
                  <div
                    className="route-list-item"
                    key={profile.id}
                  >
                    <div className="route-list-summary">
                      <span>
                        <strong>{profile.name || "未命名线路"}</strong>
                        <small>
                          {profile.authMode === "officialAccount"
                            ? "官方账号登录"
                            : profile.baseUrl || "待填写 URL"}
                        </small>
                      </span>
                      <span className="route-list-meta">
                        <Badge variant="secondary">
                          {group?.models.length || 0} 模型
                        </Badge>
                        {profile.authMode !== "officialAccount" && (
                          <Badge variant={group?.models.length ? "brand" : "secondary"}>
                            {group?.models.length ? "已接入路由" : "待配置模型"}
                          </Badge>
                        )}
                        {profile.authMode === "officialAccount" &&
                          showAccountUsageInHeader && (
                            <Badge variant="info">额度已开启</Badge>
                          )}
                      </span>
                    </div>
                    <Button
                      className="route-edit-button"
                      variant="ghost"
                      size="xs"
                      disabled={isBusy || dirty}
                      onClick={() => openEditRouteDialog(profile)}
                      aria-label={`编辑线路 ${profile.name}`}
                    >
                      <Edit aria-hidden="true" />
                      编辑
                    </Button>
                  </div>
                );
              })}
            </div>
          </aside>

          <div className="route-catalog-pane">
            <div className="catalog-aggregate-heading">
              <div>
                <strong>统一模型目录</strong>
                <small>选择模型时，本地路由会自动分发到所属供应商</small>
              </div>
              <Badge variant="secondary">
                {modelGroups.reduce((count, group) => count + group.models.length, 0)} 个
              </Badge>
            </div>

            <div className="provider-model-groups">
              {modelGroups.map((group) => (
                <section
                  className="provider-model-group"
                  key={group.providerId}
                  aria-labelledby={`provider-model-${group.profile.id}`}
                >
                  <div className="provider-model-group-heading">
                    <div>
                      <strong id={`provider-model-${group.profile.id}`}>
                        {group.profile.name}
                      </strong>
                      <small>
                        {group.official ? "官方账号" : "第三方 API Key"}
                      </small>
                    </div>
                    <div className="provider-model-group-actions">
                      <Badge variant={group.official ? "info" : "brand"}>
                        {group.models.length} 模型
                      </Badge>
                      {!group.official && (
                        <Button
                          variant="ghost"
                          size="xs"
                          disabled={isBusy || dirty}
                          onClick={() => onFetchRouteModels(group.profile)}
                        >
                          <RefreshCw
                            className={
                              busy === "fetch-route-models" &&
                              group.profile.id === config.activeProfileId
                                ? "animate-spin"
                                : ""
                            }
                            aria-hidden="true"
                          />
                          同步模型
                        </Button>
                      )}
                    </div>
                  </div>

                  <div className="provider-model-list">
                    {group.models.map((model) => {
                      const isDefault = modelIdsEqual(group.defaultModel, model);
                      return (
                        <div
                          className={`provider-model-row${isDefault ? " default-model" : ""}`}
                          key={`${group.providerId}:${model}`}
                        >
                          <button
                            type="button"
                            className="provider-model-select"
                            disabled={isBusy || dirty || isDefault}
                            onClick={() =>
                              onSetDefaultModel(group.profile.id, model)}
                          >
                            <span>
                              <strong>
                                {group.official
                                  ? officialDisplayNames.get(modelKey(model)) || model
                                  : model}
                              </strong>
                              {group.official && <small>{model}</small>}
                            </span>
                            {isDefault ? (
                              <Badge variant="brand">默认</Badge>
                            ) : (
                              <span className="set-default-label">设为默认</span>
                            )}
                          </button>
                        </div>
                      );
                    })}
                    {group.models.length === 0 && (
                      <div className="provider-model-empty">
                        <span>尚未配置模型</span>
                        {!group.official && (
                          <Button
                            variant="outline"
                            size="xs"
                            disabled={isBusy || dirty}
                            onClick={() => onFetchRouteModels(group.profile)}
                          >
                            <RefreshCw aria-hidden="true" />
                            同步或手动添加
                          </Button>
                        )}
                      </div>
                    )}
                  </div>
                </section>
              ))}
            </div>
          </div>
        </div>

        <div className="readonly-note">
          <Server size={14} />
          <span className="readonly-note-text">
            所有第三方线路通过 Codey 本地路由同时生效，线路列表仅用于管理配置
          </span>
          <Badge variant="brand" className="readonly-note-tag">
            一次性
          </Badge>
        </div>
      </Card>

      <Dialog
        open={routeDialogOpen}
        onOpenChange={(open) => {
          if (!isBusy) {
            setRouteDialogOpen(open);
            if (!open) setRouteDraft(null);
          }
        }}
      >
        {routeDialogOpen && routeDraft && (
          <DialogContent
            className="route-editor-dialog"
            container={popupContainer ?? undefined}
            zIndex={SETTINGS_OVERLAY_Z_INDEX}
            onEscapeKeyDown={(event) => {
              if (isBusy) event.preventDefault();
            }}
            onPointerDownOutside={(event) => {
              if (isBusy) event.preventDefault();
            }}
          >
            <DialogHeader>
              <DialogTitle>
                {routeDraft.authMode === "officialAccount"
                  ? "编辑官方账号"
                  : config.profiles.some((profile) => profile.id === routeDraft.id)
                    ? "编辑线路"
                    : "新增线路"}
              </DialogTitle>
              <DialogDescription>
                {routeDraft.authMode === "officialAccount"
                  ? "选择允许在 Codex 中使用的官方模型，并配置账户额度展示。"
                  : "配置第三方服务的接入信息。保存后可在模型目录中同步模型。"}
              </DialogDescription>
            </DialogHeader>

            {routeDraft.authMode === "officialAccount" ? (
              <div className="official-route-editor">
                <div className="official-route-summary">
                  <span>
                    <strong>{routeDraft.name}</strong>
                    <small>使用当前 Codex 官方账号登录状态</small>
                  </span>
                  <Badge variant="info">官方账号</Badge>
                </div>

                <div className="official-usage-setting">
                  <span>
                    <strong>在账户区域显示额度</strong>
                    <small>开启后，Codex 账户卡片会展示额度与重置时间。</small>
                  </span>
                  <Switch
                    checked={usageDraft}
                    disabled={isBusy}
                    onCheckedChange={setUsageDraft}
                    aria-label="在账户区域显示额度"
                  />
                </div>

                <div className="official-model-editor">
                  <div className="official-model-editor-heading">
                    <span>
                      <strong>支持的模型</strong>
                      <small>已启用 {officialModelDraft.length} 个，至少保留一个。</small>
                    </span>
                    <Badge variant="secondary">
                      {officialModelDraft.length} / {officialCatalog.length}
                    </Badge>
                  </div>
                  <div className="official-model-options">
                    {officialCatalog.map((model) => {
                      const checked = officialModelDraftKeys.has(modelKey(model));
                      return (
                        <label className="official-model-option" key={model}>
                          <Checkbox
                            checked={checked}
                            disabled={isBusy || (checked && officialModelDraft.length <= 1)}
                            onCheckedChange={(nextChecked) => {
                              setOfficialModelDraft((current) =>
                                nextChecked === true
                                  ? uniqueModels([...current, model])
                                  : current.filter(
                                      (candidate) => !modelIdsEqual(candidate, model),
                                    ),
                              );
                            }}
                            aria-label={`${checked ? "停用" : "启用"}官方模型 ${model}`}
                          />
                          <span>
                            <strong>
                              {officialDisplayNames.get(modelKey(model)) || model}
                            </strong>
                            <small>{model}</small>
                          </span>
                        </label>
                      );
                    })}
                  </div>
                </div>
              </div>
            ) : (
              <div className="route-editor-form">
                <label className="route-field">
                  <span>线路名</span>
                  <Input
                    aria-label="线路名"
                    value={routeDraft.name}
                    disabled={isBusy}
                    onChange={(event) => updateRouteDraft({ name: event.target.value })}
                  />
                </label>
                <div className="route-field">
                  <span id="route-protocol-label">上游协议</span>
                  <Select
                    aria-label="上游协议"
                    aria-labelledby="route-protocol-label"
                    value={routeDraft.upstreamProtocol}
                    disabled={isBusy}
                    getPopupContainer={() => popupContainer ?? document.body}
                    zIndex={SETTINGS_OVERLAY_Z_INDEX}
                    onChange={(value) => value != null &&
                      updateRouteDraft({
                        upstreamProtocol: value as Profile["upstreamProtocol"],
                      })}
                    optionList={routeProtocolOptions}
                  />
                  <small className="route-field-hint">
                    请选择上游服务实际支持的 Responses 协议。
                  </small>
                </div>
                <label className="route-field">
                  <span>URL</span>
                  <Input
                    aria-label="URL"
                    value={routeDraft.baseUrl}
                    disabled={isBusy}
                    placeholder="https://api.example.com/v1"
                    onChange={(event) => updateRouteDraft({ baseUrl: event.target.value })}
                  />
                </label>
                <label className="route-field">
                  <span>Key</span>
                  <Input
                    aria-label="Key"
                    type="password"
                    autoComplete="off"
                    value={routeDraft.apiKey}
                    disabled={isBusy}
                    placeholder={
                      routeDraft.apiKeyConfigured
                        ? "已保存（输入新 Key 可替换）"
                        : "sk-..."
                    }
                    onChange={(event) => {
                      const value = event.target.value;
                      updateRouteDraft({
                        apiKey: value,
                        clearApiKey: value.trim() === "" ? routeDraft.clearApiKey : false,
                      });
                    }}
                  />
                  {routeDraft.apiKeyConfigured && !routeDraft.clearApiKey && (
                    <Button
                      className="route-clear-key"
                      type="button"
                      variant="ghost"
                      size="xs"
                      disabled={isBusy}
                      onClick={() =>
                        updateRouteDraft({
                          apiKey: "",
                          apiKeyConfigured: false,
                          clearApiKey: true,
                        })}
                    >
                      清除已保存 Key
                    </Button>
                  )}
                </label>
              </div>
            )}

            <DialogFooter className="route-editor-footer">
              {routeDraft.authMode !== "officialAccount" &&
                config.profiles.some((profile) => profile.id === routeDraft.id) && (
                <Button
                  className="catalog-model-delete route-editor-delete"
                  variant="ghost"
                  disabled={isBusy || config.profiles.length <= 1}
                  onClick={() => {
                    setRouteDialogOpen(false);
                    setRouteDraft(null);
                    onDeleteRoute(routeDraft.id);
                  }}
                >
                  <Trash aria-hidden="true" />
                  删除线路
                </Button>
                )}
              <Button
                variant="outline"
                disabled={isBusy}
                onClick={() => {
                  setRouteDialogOpen(false);
                  setRouteDraft(null);
                }}
              >
                取消
              </Button>
              <Button
                disabled={isBusy || (
                  routeDraft.authMode === "officialAccount" &&
                  officialModelDraft.length === 0
                )}
                onClick={() => void saveRouteDraft()}
              >
                <Check aria-hidden="true" />
                {routeDraft.authMode === "officialAccount" ? "保存设置" : "保存线路"}
              </Button>
            </DialogFooter>
          </DialogContent>
        )}
      </Dialog>
    </section>
  );
}

export const ModelSection = memo(ModelSectionComponent);
