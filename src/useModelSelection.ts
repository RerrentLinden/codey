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

type UseModelSelectionOptions = {
  provider: CcSwitchStatus["provider"] | undefined;
  runOperation: (name: string, action: () => Promise<void>) => Promise<void>;
  setPersistedConfig: (config: Config) => void;
  setStatus: Dispatch<SetStateAction<RuntimeStatus>>;
  setNotice: Dispatch<SetStateAction<Notice>>;
  setSubagentOptimization: (enabled: boolean) => void;
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
  const [modelQuery, setModelQuery] = useState("");
  const [draftModels, setDraftModels] = useState<string[]>([]);

  const officialSlugs = useMemo(
    () => new Set(modelState.officialModelIds),
    [modelState.officialModelIds],
  );
  const draftModelSet = useMemo(() => new Set(draftModels), [draftModels]);
  const filteredUpstreamModels = useMemo(() => {
    const query = modelQuery.trim().toLowerCase();
    return query
      ? modelState.upstreamModels.filter((model) =>
          model.toLowerCase().includes(query),
        )
      : modelState.upstreamModels;
  }, [modelQuery, modelState.upstreamModels]);

  async function fetchCurrentModels() {
    if (!provider || provider.official) return;
    await runOperation("fetch-models", async () => {
      const result = await withTimeout(
        invoke<{ modelState: ModelState; restartRequired?: boolean }>(
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
      setDraftModels(result.modelState.thirdPartyModels);
      setModelQuery("");
      setModelPickerVisible(true);
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
          restartRequired?: boolean;
        };
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

  async function saveModelSelection() {
    await runOperation("save-models", async () => {
      const result = await invoke<{
        config: Config;
        modelState: ModelState;
        restartRequired?: boolean;
      }>("save_selected_models", { models: draftModels });
      setPersistedConfig(result.config);
      setModelState(result.modelState);
      setStatus((current) => ({
        ...current,
        restartRequired: result.restartRequired ?? current.restartRequired,
      }));
      setModelPickerVisible(false);
      setNotice({
        tone: result.restartRequired ? "info" : "success",
        text: result.restartRequired
          ? `已更新模型列表，共 ${result.modelState.thirdPartyModels.length} 个三方模型；重启 Codex 后生效`
          : `已更新模型列表，共 ${result.modelState.thirdPartyModels.length} 个三方模型`,
      });
    });
  }

  async function setDefaultModel(model: string) {
    await runOperation("save-default-model", async () => {
      const result = await invoke<{
        config: Config;
        modelState: ModelState;
        restartRequired?: boolean;
      }>("save_default_model", { model });
      setPersistedConfig(result.config);
      setModelState(result.modelState);
      setStatus((current) => ({
        ...current,
        restartRequired: result.restartRequired ?? current.restartRequired,
      }));
      setNotice({
        tone: result.restartRequired ? "info" : "success",
        text: result.restartRequired
          ? `已将 ${result.modelState.defaultModel} 设为默认模型；重启 Codex 后新对话生效`
          : `已将 ${result.modelState.defaultModel} 设为默认模型`,
      });
    });
  }

  return {
    subagentModel: SUBAGENT_MODEL,
    modelState,
    setModelState,
    modelPickerVisible,
    setModelPickerVisible,
    modelQuery,
    setModelQuery,
    setDraftModels,
    officialSlugs,
    draftModelSet,
    filteredUpstreamModels,
    fetchCurrentModels,
    updateSubagentOptimization,
    toggleDraftModel,
    saveModelSelection,
    setDefaultModel,
  };
}
