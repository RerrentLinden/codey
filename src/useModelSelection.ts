import {
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
import { errorText, withTimeout } from "./appUtils";

const THIRD_PARTY_REASONING_EFFORTS = ["low", "medium", "high", "xhigh"];

const supportsModel = (models: string[], expected: string) =>
  models.some(
    (model) => model.trim().toLowerCase() === expected.trim().toLowerCase(),
  );

const modelKey = (model: string) => model.trim().toLowerCase();

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
  setSubagentOptimization: (enabled: boolean) => void;
};

type ModelRuntimeUpdate = {
  restartRequired?: boolean;
  modelHotReloaded?: boolean;
  modelHotReloadError?: string;
};

export function useModelSelection({
  provider,
  runOperation,
  setPersistedConfig,
  setStatus,
  setNotice,
  setSubagentOptimization,
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
  const [draftModels, setDraftModels] = useState<string[]>([]);
  const [draftManualThirdPartyModels, setDraftManualThirdPartyModels] = useState<string[]>([]);
  const [deletedThirdPartyModels, setDeletedThirdPartyModels] = useState<string[]>([]);
  const [customModelInput, setCustomModelInput] = useState("");
  const [modelInputError, setModelInputError] = useState("");
  const [modelSyncWarning, setModelSyncWarning] = useState("");

  const officialSlugs = useMemo(
    () => new Set(modelState.officialModelIds),
    [modelState.officialModelIds],
  );
  const officialSlugKeys = useMemo(
    () => new Set(modelState.officialModelIds.map(modelKey)),
    [modelState.officialModelIds],
  );
  const draftModelSet = useMemo(() => new Set(draftModels), [draftModels]);
  const draftManualThirdPartyModelKeys = useMemo(
    () => new Set(draftManualThirdPartyModels.map(modelKey)),
    [draftManualThirdPartyModels],
  );
  const manualThirdPartyModelKeys = useMemo(
    () => new Set(modelState.manualThirdPartyModels.map(modelKey)),
    [modelState.manualThirdPartyModels],
  );
  const deletedThirdPartyModelKeys = useMemo(
    () => new Set(deletedThirdPartyModels.map(modelKey)),
    [deletedThirdPartyModels],
  );
  const thirdPartyModelOptions = useMemo(
    () => [
      ...modelState.upstreamModels,
      ...modelState.thirdPartyModels,
      ...draftModels,
    ].reduce<string[]>((models, model) => {
      const normalized = model.trim();
      if (
        normalized &&
        !officialSlugKeys.has(modelKey(normalized)) &&
        !deletedThirdPartyModelKeys.has(modelKey(normalized)) &&
        !models.includes(normalized)
      ) {
        models.push(normalized);
      }
      return models;
    }, []),
    [
      draftModels,
      deletedThirdPartyModelKeys,
      modelState.thirdPartyModels,
      modelState.upstreamModels,
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
      ...modelState.thirdPartyModels.map((model) => ({
        value: model,
        label: model,
        supportedReasoningEfforts: THIRD_PARTY_REASONING_EFFORTS,
        defaultReasoningEffort: "low",
      })),
    ],
    [modelState.officialModels, modelState.thirdPartyModels],
  );

  function openModelPicker(state: ModelState, warning = "") {
    setDraftModels(pickerSelection(state));
    setDraftManualThirdPartyModels(state.manualThirdPartyModels);
    setDeletedThirdPartyModels([]);
    setCustomModelInput("");
    setModelInputError("");
    setModelSyncWarning(warning);
    setModelPickerVisible(true);
  }

  async function fetchCurrentModels() {
    if (!provider || provider.official) return;
    await runOperation("fetch-models", async () => {
      try {
        const result = await withTimeout(
          invoke<{ modelState: ModelState } & ModelRuntimeUpdate>(
            "fetch_current_provider_models",
          ),
          15_000,
          "获取上游模型超时，请检查当前线路",
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
          "请手动确认支持的官方模型，或输入其他模型 ID。";
        openModelPicker(modelState, warning);
        setNotice({
          tone: "error",
          text: "第三方模型同步失败，当前线路可能不支持 /v1/models 或 /models 接口，已打开手动配置。",
        });
      }
    });
  }

  async function updateSubagentOptimization(
    checked: boolean,
    selectedModel: string,
  ) {
    if (!checked) {
      setSubagentOptimization(false);
      return;
    }
    if (!provider) {
      setNotice({
        tone: "error",
        text: "当前线路尚未就绪，无法校验子代理模型",
      });
      return;
    }
    const subagentModel = selectedModel.trim();
    if (!subagentModel || !subagentModelOptions.some((option) => option.value === subagentModel)) {
      setNotice({
        tone: "error",
        text: "请先从当前已选模型列表中选择子代理模型",
      });
      return;
    }
    await runOperation("check-subagent-model", async () => {
      let supported = false;
      if (provider.official) {
        supported = modelState.officialModels.some(
          (model) => model.slug === subagentModel && model.supported,
        );
      } else {
        let result: {
          models: string[];
          modelState: ModelState;
        } & ModelRuntimeUpdate;
        try {
          result = await withTimeout(
            invoke("fetch_current_provider_models"),
            15_000,
            "获取上游模型超时，请检查当前线路",
          );
        } catch (error) {
          throw new Error(
            `无法确认当前第三方 API 是否支持 ${subagentModel}：${errorText(error)}`,
          );
        }
        setModelState(result.modelState);
        if (typeof result.restartRequired === "boolean") {
          setStatus((current) => ({
            ...current,
            restartRequired: result.restartRequired,
          }));
        }
        supported = supportsModel(result.models, subagentModel);
      }

      if (!supported) {
        setNotice({
          tone: "error",
          text: `当前${provider.official ? "官方账号" : "第三方 API"}不支持 ${subagentModel}，无法开启子代理协作优化`,
        });
        return;
      }
      setSubagentOptimization(true);
      setNotice({
        tone: "success",
        text: `已确认当前线路支持 ${subagentModel}，保存并重启 Codex 后生效`,
      });
    });
  }

  function toggleDraftModel(model: string, checked: boolean) {
    if (checked) {
      setDeletedThirdPartyModels((current) =>
        current.filter((item) => modelKey(item) !== modelKey(model)),
      );
    }
    setDraftModels((current) =>
      checked
        ? current.includes(model)
          ? current
          : [...current, model]
        : current.filter((item) => item !== model),
    );
    if (!checked) {
      setDraftManualThirdPartyModels((current) =>
        current.filter((item) => modelKey(item) !== modelKey(model)),
      );
    }
  }

  function updateCustomModelInput(value: string) {
    setCustomModelInput(value);
    if (modelInputError) setModelInputError("");
  }

  function addCustomModel() {
    const model = customModelInput.trim();
    if (!model) {
      setModelInputError("请输入要添加的模型 ID");
      return;
    }
    const officialModel = modelState.officialModelIds.find(
      (official) => modelKey(official) === modelKey(model),
    );
    if (officialModel) {
      setModelInputError(
        `${officialModel} 已在上方官方模型列表中，请直接勾选，不可重复输入`,
      );
      return;
    }
    const existingUpstreamModel = modelState.upstreamModels.find(
      (upstream) => modelKey(upstream) === modelKey(model),
    );
    setDraftModels((current) =>
      current.includes(model) ? current : [...current, model],
    );
    if (!existingUpstreamModel || manualThirdPartyModelKeys.has(modelKey(model))) {
      setDraftManualThirdPartyModels((current) =>
        current.some((item) => modelKey(item) === modelKey(model))
          ? current
          : [...current, model],
      );
    }
    setDeletedThirdPartyModels((current) =>
      current.filter((item) => modelKey(item) !== modelKey(model)),
    );
    setCustomModelInput("");
    setModelInputError("");
  }

  function deleteDraftThirdPartyModel(model: string) {
    const normalized = model.trim();
    if (!normalized) return;
    const wasManual = draftManualThirdPartyModelKeys.has(modelKey(normalized));
    if (!wasManual) return;
    setDraftModels((current) =>
      current.filter((item) => modelKey(item) !== modelKey(normalized)),
    );
    setDraftManualThirdPartyModels((current) =>
      current.filter((item) => modelKey(item) !== modelKey(normalized)),
    );
    setDeletedThirdPartyModels((current) =>
      !manualThirdPartyModelKeys.has(modelKey(normalized)) ||
      current.some((item) => modelKey(item) === modelKey(normalized))
        ? current
        : [...current, normalized],
    );
    setModelInputError("");
  }

  async function applyModelSelection(
    officialModels: string[],
    thirdPartyModels: string[],
    manualThirdPartyModels: string[],
    deletedModels: string[],
    summary: string,
    closePicker: boolean,
  ) {
    const result = await invoke<{
      config: Config;
      modelState: ModelState;
    } & ModelRuntimeUpdate>("save_selected_models", {
      officialModels,
      thirdPartyModels,
      manualThirdPartyModels,
      deletedThirdPartyModels: deletedModels,
    });
    setPersistedConfig(result.config);
    setModelState(result.modelState);
    setStatus((current) => ({
      ...current,
      restartRequired: result.restartRequired ?? current.restartRequired,
    }));
    if (closePicker) {
      setModelPickerVisible(false);
    }
    setDeletedThirdPartyModels([]);
    const hotReloadFailed = Boolean(result.modelHotReloadError);
    setNotice({
      tone:
        hotReloadFailed || result.restartRequired ? "info" : "success",
      text: result.modelHotReloaded
        ? result.restartRequired
          ? `${summary}；Codex 模型列表已立即更新，其他设置仍需重启`
          : `${summary}；Codex 模型列表已立即更新`
        : hotReloadFailed || result.restartRequired
          ? `${summary}；当前 Codex 模型列表暂未能刷新，重启 Codex 后生效`
          : summary,
    });
  }

  async function saveModelSelection() {
    await runOperation("save-models", async () => {
      const officialModels = draftModels.filter((model) =>
        officialSlugs.has(model)
      );
      const thirdPartyModels = draftModels.filter((model) =>
        !officialSlugs.has(model)
      );
      const thirdPartyModelKeys = new Set(thirdPartyModels.map(modelKey));
      const manualThirdPartyModels = draftManualThirdPartyModels.filter((model) =>
        thirdPartyModelKeys.has(modelKey(model))
      );
      await applyModelSelection(
        officialModels,
        thirdPartyModels,
        manualThirdPartyModels,
        deletedThirdPartyModels,
        `已更新模型支持情况：${officialModels.length} 个官方模型、` +
          `${thirdPartyModels.length} 个其他模型`,
        true,
      );
    });
  }

  async function deleteThirdPartyModel(model: string) {
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
      const thirdPartyModels = modelState.thirdPartyModels.filter(
        (candidate) => modelKey(candidate) !== deletedKey,
      );
      const manualThirdPartyModels = modelState.manualThirdPartyModels.filter(
        (candidate) => modelKey(candidate) !== deletedKey,
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
        current.filter((item) => modelKey(item) !== deletedKey),
      );
      setDraftManualThirdPartyModels((current) =>
        current.filter((item) => modelKey(item) !== deletedKey),
      );
    });
  }

  async function setDefaultModel(model: string) {
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
  }

  return {
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
  };
}
