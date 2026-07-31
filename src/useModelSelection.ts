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

const SUBAGENT_MODEL = "gpt-5.6-luna";

const supportsModel = (models: string[], expected: string) =>
  models.some((model) => model.trim().toLowerCase() === expected);

const modelKey = (model: string) => model.trim().toLowerCase();

const pickerSelection = (state: ModelState) => [
  ...state.officialModels
    .filter((model) => model.supported)
    .map((model) => model.slug),
  ...state.thirdPartyModels,
];

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
    upstreamModels: [],
    defaultModel: "",
  });
  const [modelPickerVisible, setModelPickerVisible] = useState(false);
  const [draftModels, setDraftModels] = useState<string[]>([]);
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
        !models.includes(normalized)
      ) {
        models.push(normalized);
      }
      return models;
    }, []),
    [
      draftModels,
      modelState.thirdPartyModels,
      modelState.upstreamModels,
      officialSlugKeys,
    ],
  );

  function openModelPicker(state: ModelState, warning = "") {
    setDraftModels(pickerSelection(state));
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

  async function updateSubagentOptimization(checked: boolean) {
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
    await runOperation("check-subagent-model", async () => {
      let supported = false;
      if (provider.official) {
        supported = modelState.officialModels.some(
          (model) => model.slug === SUBAGENT_MODEL && model.supported,
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
            `无法确认当前第三方 API 是否支持 ${SUBAGENT_MODEL}：${errorText(error)}`,
          );
        }
        setModelState(result.modelState);
        if (typeof result.restartRequired === "boolean") {
          setStatus((current) => ({
            ...current,
            restartRequired: result.restartRequired,
          }));
        }
        supported = supportsModel(result.models, SUBAGENT_MODEL);
      }

      if (!supported) {
        setNotice({
          tone: "error",
          text: `当前${provider.official ? "官方账号" : "第三方 API"}不支持 ${SUBAGENT_MODEL}，无法开启子代理协作优化`,
        });
        return;
      }
      setSubagentOptimization(true);
      setNotice({
        tone: "success",
        text: `已确认当前线路支持 ${SUBAGENT_MODEL}，保存并重启 Codex 后生效`,
      });
    });
  }

  function toggleDraftModel(model: string, checked: boolean) {
    setDraftModels((current) =>
      checked
        ? current.includes(model)
          ? current
          : [...current, model]
        : current.filter((item) => item !== model),
    );
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
    setDraftModels((current) =>
      current.includes(model) ? current : [...current, model],
    );
    setCustomModelInput("");
    setModelInputError("");
  }

  async function saveModelSelection() {
    await runOperation("save-models", async () => {
      const officialModels = draftModels.filter((model) =>
        officialSlugs.has(model)
      );
      const thirdPartyModels = draftModels.filter((model) =>
        !officialSlugs.has(model)
      );
      const result = await invoke<{
        config: Config;
        modelState: ModelState;
      } & ModelRuntimeUpdate>("save_selected_models", {
        officialModels,
        thirdPartyModels,
      });
      setPersistedConfig(result.config);
      setModelState(result.modelState);
      setStatus((current) => ({
        ...current,
        restartRequired: result.restartRequired ?? current.restartRequired,
      }));
      setModelPickerVisible(false);
      const summary =
        `已更新模型支持情况：${officialModels.length} 个官方模型、` +
        `${thirdPartyModels.length} 个其他模型`;
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
    subagentModel: SUBAGENT_MODEL,
    modelState,
    setModelState,
    modelPickerVisible,
    setModelPickerVisible,
    customModelInput,
    modelInputError,
    modelSyncWarning,
    draftModelSet,
    thirdPartyModelOptions,
    fetchCurrentModels,
    updateSubagentOptimization,
    toggleDraftModel,
    updateCustomModelInput,
    addCustomModel,
    saveModelSelection,
    setDefaultModel,
  };
}
