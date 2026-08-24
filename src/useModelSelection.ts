import {
  useCallback,
  useMemo,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";

import { invoke } from "./api";
import type {
  CcSwitchStatus,
  Config,
  ModelState,
  Notice,
  RuntimeStatus,
} from "./App.types";
import { errorText } from "./appUtils";
import {
  includesModelId,
  modelKey,
  partitionModelIdsByKey,
  uniqueModelIds,
  withoutModelId,
} from "./modelIds";

const THIRD_PARTY_REASONING_EFFORTS = ["low", "medium", "high", "xhigh"];
const MAX_MODEL_ID_BYTES = 512;
const MAX_MODEL_COUNT = 10_000;
const modelIdEncoder = new TextEncoder();

const pickerSelection = (state: ModelState) => [
  ...state.officialModels
    .filter((model) => model.supported)
    .map((model) => model.slug),
  ...state.thirdPartyModels,
];

export type SubagentModelOption = {
  value: string;
  label: string;
  supportedReasoningEfforts: string[];
  defaultReasoningEffort: string;
};

type UseModelSelectionOptions = {
  provider: CcSwitchStatus["provider"] | undefined;
  runOperation: (name: string, action: () => Promise<void>) => Promise<void>;
  setPersistedConfig: (config: Config) => void;
  setStatus: Dispatch<SetStateAction<RuntimeStatus>>;
  setNotice: Dispatch<SetStateAction<Notice>>;
};

type ModelRuntimeUpdate = {
  restartRequired?: boolean;
  modelHotReloaded?: boolean;
  modelHotReloadError?: string;
  subagentConfigHotReloaded?: boolean;
  subagentConfigRepaired?: boolean;
  subagentConfigHotReloadError?: string;
  modelCatalogFallback?: boolean;
};

export function useModelSelection({
  provider,
  runOperation,
  setPersistedConfig,
  setStatus,
  setNotice,
}: UseModelSelectionOptions) {
  const [modelState, setModelState] = useState<ModelState>({
    officialModels: [],
    officialModelIds: [],
    thirdPartyModels: [],
    manualThirdPartyModels: [],
    upstreamModels: [],
    defaultModel: "",
  });
  const [modelPickerVisible, setModelPickerVisible] = useState(false);
  const [modelPickerRouteId, setModelPickerRouteId] = useState<string | null>(null);
  const [modelPickerState, setModelPickerState] = useState<ModelState | null>(null);
  const [draftModels, setDraftModels] = useState<string[]>([]);
  const [draftManualThirdPartyModels, setDraftManualThirdPartyModels] = useState<string[]>([]);
  const [deletedThirdPartyModels, setDeletedThirdPartyModels] = useState<string[]>([]);
  const [customModelInput, setCustomModelInput] = useState("");
  const [modelInputError, setModelInputError] = useState("");
  const [modelSyncWarning, setModelSyncWarning] = useState("");

  const modelEditorState = modelPickerState ?? modelState;
  const officialSlugKeys = useMemo(
    () =>
      new Set(
        modelPickerRouteId ? [] : modelEditorState.officialModelIds.map(modelKey),
      ),
    [modelEditorState.officialModelIds, modelPickerRouteId],
  );
  const draftModelSet = useMemo(
    () => new Set(draftModels.map(modelKey)),
    [draftModels],
  );
  const draftManualThirdPartyModelKeys = useMemo(
    () => new Set(draftManualThirdPartyModels.map(modelKey)),
    [draftManualThirdPartyModels],
  );
  const manualThirdPartyModelKeys = useMemo(
    () => new Set(modelEditorState.manualThirdPartyModels.map(modelKey)),
    [modelEditorState.manualThirdPartyModels],
  );
  const deletedThirdPartyModelKeys = useMemo(
    () => new Set(deletedThirdPartyModels.map(modelKey)),
    [deletedThirdPartyModels],
  );
  const thirdPartyModelOptions = useMemo(
    () => {
      const seenKeys = new Set<string>();
      return [
        ...modelEditorState.upstreamModels,
        ...modelEditorState.thirdPartyModels,
        ...draftModels,
      ].reduce<string[]>((models, model) => {
        const normalized = model.trim();
        const key = modelKey(normalized);
        if (
          normalized &&
          !officialSlugKeys.has(key) &&
          !deletedThirdPartyModelKeys.has(key) &&
          !seenKeys.has(key)
        ) {
          seenKeys.add(key);
          models.push(normalized);
        }
        return models;
      }, []);
    },
    [
      draftModels,
      deletedThirdPartyModelKeys,
      modelEditorState.thirdPartyModels,
      modelEditorState.upstreamModels,
      officialSlugKeys,
    ],
  );
  const subagentModelOptions = useMemo<SubagentModelOption[]>(
    () => [
      ...modelState.officialModels
        .filter((model) => model.supported)
        .map((model) => ({
          value: model.slug,
          label: model.displayName,
          supportedReasoningEfforts:
            model.supportedReasoningEfforts.length > 0
              ? model.supportedReasoningEfforts
              : ["low"],
          defaultReasoningEffort: model.defaultReasoningEffort || "low",
        })),
      ...modelState.thirdPartyModels
        .map((model) => ({
          value: model,
          label: model,
          supportedReasoningEfforts: THIRD_PARTY_REASONING_EFFORTS,
          defaultReasoningEffort: "low",
        })),
    ],
    [modelState.officialModels, modelState.thirdPartyModels],
  );

  const openModelPicker = useCallback((
    state: ModelState,
    warning = "",
    routeId: string | null = null,
  ) => {
    setDraftModels(pickerSelection(state));
    setDraftManualThirdPartyModels(state.manualThirdPartyModels);
    setDeletedThirdPartyModels([]);
    setCustomModelInput("");
    setModelInputError("");
    setModelSyncWarning(warning);
    setModelPickerRouteId(routeId);
    setModelPickerState(state);
    setModelPickerVisible(true);
  }, []);

  const fetchCurrentModels = useCallback(async () => {
    if (!provider || provider.official) return;
    await runOperation("fetch-models", async () => {
      try {
        const result = await invoke<
          { modelState: ModelState } & ModelRuntimeUpdate
        >(
          "fetch_current_provider_models",
        );
        setModelState(result.modelState);
        if (typeof result.restartRequired === "boolean") {
          setStatus((current) => ({
            ...current,
            restartRequired: result.restartRequired,
          }));
        }
        openModelPicker(result.modelState);
      } catch (error) {
        const warning =
          `自动同步失败：${errorText(error)}。当前线路可能不支持 /v1/models 或 /models 接口，` +
          "请手动输入当前线路支持的模型 ID。";
        openModelPicker(modelState, warning);
        setNotice({
          tone: "error",
          text: "第三方模型同步失败，当前线路可能不支持 /v1/models 或 /models 接口，已打开手动配置。",
        });
      }
    });
  }, [
    modelState,
    openModelPicker,
    provider,
    runOperation,
    setNotice,
    setStatus,
  ]);

  const toggleDraftModel = useCallback((model: string, checked: boolean) => {
    if (checked) {
      setDeletedThirdPartyModels((current) =>
        withoutModelId(current, model),
      );
    }
    setDraftModels((current) =>
      checked
        ? includesModelId(current, model)
          ? current
          : [...current, model]
        : withoutModelId(current, model),
    );
    if (!checked) {
      setDraftManualThirdPartyModels((current) =>
        withoutModelId(current, model),
      );
    }
  }, []);

  const updateCustomModelInput = useCallback((value: string) => {
    setCustomModelInput(value);
    if (modelInputError) setModelInputError("");
  }, [modelInputError]);

  const addCustomModel = useCallback(() => {
    const model = customModelInput.trim();
    if (!model) {
      setModelInputError("请输入要添加的模型 ID");
      return;
    }
    if (modelIdEncoder.encode(model).byteLength > MAX_MODEL_ID_BYTES) {
      setModelInputError(`模型 ID 不能超过 ${MAX_MODEL_ID_BYTES} 字节`);
      return;
    }
    if (
      draftModels.length >= MAX_MODEL_COUNT &&
      !draftModels.some((item) => modelKey(item) === modelKey(model))
    ) {
      setModelInputError(`模型数量不能超过 ${MAX_MODEL_COUNT} 个`);
      return;
    }
    const officialModel = modelPickerRouteId
      ? undefined
      : modelEditorState.officialModelIds.find(
          (official) => modelKey(official) === modelKey(model),
        );
    if (officialModel) {
      setModelInputError(
        `${officialModel} 已在上方官方模型列表中，请直接勾选，不可重复输入`,
      );
      return;
    }
    const existingUpstreamModel = modelEditorState.upstreamModels.find(
      (upstream) => modelKey(upstream) === modelKey(model),
    );
    setDraftModels((current) =>
      includesModelId(current, model) ? current : [...current, model],
    );
    if (!existingUpstreamModel || manualThirdPartyModelKeys.has(modelKey(model))) {
      setDraftManualThirdPartyModels((current) =>
        current.some((item) => modelKey(item) === modelKey(model))
          ? current
          : [...current, model],
      );
    }
    setDeletedThirdPartyModels((current) =>
      withoutModelId(current, model),
    );
    setCustomModelInput("");
    setModelInputError("");
  }, [
    customModelInput,
    draftModels,
    manualThirdPartyModelKeys,
    modelEditorState.officialModelIds,
    modelEditorState.upstreamModels,
    modelPickerRouteId,
  ]);

  const deleteDraftThirdPartyModel = useCallback((model: string) => {
    const normalized = model.trim();
    if (!normalized) return;
    const normalizedKey = modelKey(normalized);
    const wasManual = draftManualThirdPartyModelKeys.has(normalizedKey);
    if (!wasManual) return;
    setDraftModels((current) =>
      withoutModelId(current, normalized),
    );
    setDraftManualThirdPartyModels((current) =>
      withoutModelId(current, normalized),
    );
    setDeletedThirdPartyModels((current) =>
      !manualThirdPartyModelKeys.has(normalizedKey) ||
      current.some((item) => modelKey(item) === normalizedKey)
        ? current
        : [...current, normalized],
    );
    setModelInputError("");
  }, [
    draftManualThirdPartyModelKeys,
    manualThirdPartyModelKeys,
  ]);

  const applyModelSelection = useCallback(async (
    officialModels: string[],
    thirdPartyModels: string[],
    manualThirdPartyModels: string[],
    deletedModels: string[],
    summary: string,
    closePicker: boolean,
  ) => {
    const result = await invoke<{
      config: Config;
      modelState: ModelState;
    } & ModelRuntimeUpdate>("save_selected_models", {
      officialModels,
      thirdPartyModels,
      manualThirdPartyModels,
      deletedThirdPartyModels: deletedModels,
      routeId: modelPickerRouteId,
    });
    setPersistedConfig(result.config);
    setModelState(result.modelState);
    setStatus((current) => ({
      ...current,
      restartRequired: result.restartRequired ?? current.restartRequired,
    }));
    if (closePicker) {
      setModelPickerVisible(false);
      setModelPickerRouteId(null);
      setModelPickerState(null);
    }
    setDeletedThirdPartyModels([]);
    const hotReloadFailed = Boolean(
      result.modelHotReloadError || result.subagentConfigHotReloadError,
    );
    const subagentSuffix = result.subagentConfigRepaired
      ? "；已校验并修复受影响的子代理运行配置"
      : result.subagentConfigHotReloaded
        ? "；受影响的子代理角色也已同步"
      : "";
    setNotice({
      tone:
        hotReloadFailed || result.restartRequired ? "info" : "success",
      text: result.modelHotReloaded
        ? result.restartRequired
          ? `${summary}；Codex 模型列表已立即更新，其他设置仍需重启`
          : `${summary}；Codex 模型列表已立即更新${subagentSuffix}`
        : hotReloadFailed || result.restartRequired
          ? `${summary}；当前 Codex 模型列表暂未能刷新，重启 Codex 后生效`
          : summary,
    });
  }, [
    setNotice,
    setPersistedConfig,
    setStatus,
    modelPickerRouteId,
  ]);

  const saveModelSelection = useCallback(async () => {
    await runOperation("save-models", async () => {
      const normalizedDraftModels = uniqueModelIds(draftModels);
      const {
        matching: officialModels,
        remaining: thirdPartyModels,
      } = partitionModelIdsByKey(normalizedDraftModels, officialSlugKeys);
      const thirdPartyModelKeys = new Set(thirdPartyModels.map(modelKey));
      const manualThirdPartyModels = draftManualThirdPartyModels.filter((model) =>
        thirdPartyModelKeys.has(modelKey(model))
      );
      await applyModelSelection(
        officialModels,
        thirdPartyModels,
        manualThirdPartyModels,
        deletedThirdPartyModels,
        `已更新模型声明：${thirdPartyModels.length} 个线路模型`,
        true,
      );
    });
  }, [
    applyModelSelection,
    deletedThirdPartyModels,
    draftManualThirdPartyModels,
    draftModels,
    officialSlugKeys,
    runOperation,
  ]);

  const deleteThirdPartyModel = useCallback(async (model: string) => {
    const normalized = model.trim();
    if (!normalized) return;
    const deletedKey = modelKey(normalized);
    if (!manualThirdPartyModelKeys.has(deletedKey)) {
      setNotice({
        tone: "error",
        text: `${normalized} 不是手动添加的其他模型，不能删除`,
      });
      return;
    }
    await runOperation("delete-model", async () => {
      const officialModels = modelState.officialModels
        .filter((candidate) => candidate.supported)
        .map((candidate) => candidate.slug);
      const thirdPartyModels = withoutModelId(
        modelState.thirdPartyModels,
        normalized,
      );
      const manualThirdPartyModels = withoutModelId(
        modelState.manualThirdPartyModels,
        normalized,
      );
      await applyModelSelection(
        officialModels,
        thirdPartyModels,
        manualThirdPartyModels,
        [normalized],
        `已删除其他模型 ${normalized}`,
        false,
      );
      setDraftModels((current) =>
        withoutModelId(current, normalized),
      );
      setDraftManualThirdPartyModels((current) =>
        withoutModelId(current, normalized),
      );
    });
  }, [
    applyModelSelection,
    manualThirdPartyModelKeys,
    modelState.manualThirdPartyModels,
    modelState.officialModels,
    modelState.thirdPartyModels,
    runOperation,
    setNotice,
  ]);

  const setDefaultModel = useCallback(async (model: string) => {
    await runOperation("save-default-model", async () => {
      const result = await invoke<{
        config: Config;
        modelState: ModelState;
      } & ModelRuntimeUpdate>("save_default_model", { model });
      setPersistedConfig(result.config);
      setModelState(result.modelState);
      setStatus((current) => ({
        ...current,
        restartRequired: result.restartRequired ?? current.restartRequired,
      }));
      const summary = `已将 ${result.modelState.defaultModel} 设为默认模型`;
      const hotReloadFailed = Boolean(result.modelHotReloadError);
      setNotice({
        tone:
          hotReloadFailed || result.restartRequired ? "info" : "success",
        text: result.modelHotReloaded
          ? result.restartRequired
            ? `${summary}；默认模型已立即更新，其他设置仍需重启`
            : `${summary}；Codex 模型选择器已立即更新，新对话将使用该模型`
          : hotReloadFailed || result.restartRequired
            ? `${summary}；当前 Codex 暂未能热更新，重启后新对话生效`
            : summary,
      });
    });
  }, [
    runOperation,
    setNotice,
    setPersistedConfig,
    setStatus,
  ]);

  return {
    subagentModelOptions,
    modelState,
    modelEditorState,
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
    openModelPicker,
    fetchCurrentModels,
    toggleDraftModel,
    deleteDraftThirdPartyModel,
    updateCustomModelInput,
    addCustomModel,
    saveModelSelection,
    deleteThirdPartyModel,
    setDefaultModel,
  };
}
