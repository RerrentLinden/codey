import { memo, useId, useMemo, useRef, useState } from "react";

import {
  IconEye,
  IconEyeOff,
  IconKey,
  IconPlus,
  IconPlugConnected,
  IconRefresh,
  IconRobot,
  IconSparkles,
  IconWorld,
} from "@tabler/icons-react";

import type { CcSwitchStatus, Config, InlineResult } from "./App.types";
import { invoke } from "./api";
import { errorText, withTimeout } from "./appUtils";
import { Button, Card, Input, Select, Switch } from "./components/semi";
import { SETTINGS_OVERLAY_Z_INDEX } from "./overlay.constants";

const TEST_TIMEOUT_MS = 65_000;
const FETCH_MODELS_TIMEOUT_MS = 20_000;
const SAVED_API_KEY_MASK = "****************";
const DEFAULT_OPTIMIZER_INSTRUCTION =
  "你是提示词优化专家。用户会提供一段提示词，请在不改变其意图的前提下，把它重写为更清晰、更具体、可执行的高质量提示词。只输出优化后的提示词本身，不要添加任何解释、前言、后记或代码围栏。";

type PromptOptimizationCardProps = {
  config: Config;
  provider: CcSwitchStatus["provider"];
  isBusy: boolean;
  busy: string | null;
  popupContainer: HTMLElement | null;
  onConfigChange: (config: Config) => void;
  onSyncCurrentProvider: () => Promise<boolean>;
};

type TestResult = {
  httpStatus?: number;
  responsePreview?: string;
};

function PromptOptimizationCardComponent({
  config,
  provider,
  isBusy,
  busy,
  popupContainer,
  onConfigChange,
  onSyncCurrentProvider,
}: PromptOptimizationCardProps) {
  const optimization = config.promptOptimization;
  const controlId = useId();
  const requestSequenceRef = useRef(0);
  const activeOperationRef = useRef<"sync" | "models" | "test" | null>(null);
  const [apiKeyVisible, setApiKeyVisible] = useState(false);
  const [revealedApiKey, setRevealedApiKey] = useState<string | null>(null);
  const [revealingApiKey, setRevealingApiKey] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [syncResult, setSyncResult] = useState<InlineResult>({
    tone: "idle",
    text: "",
  });
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<InlineResult>({
    tone: "idle",
    text: "",
  });
  const [cloudModels, setCloudModels] = useState<string[]>([]);
  const [fetchingModels, setFetchingModels] = useState(false);
  const [modelsResult, setModelsResult] = useState<InlineResult>({
    tone: "idle",
    text: "",
  });

  const updateOptimization = (patch: Partial<Config["promptOptimization"]>) => {
    onConfigChange({
      ...config,
      promptOptimization: { ...optimization, ...patch },
    });
  };
  const showingSavedApiKey =
    optimization.apiKeyConfigured &&
    optimization.apiKey.trim() === "" &&
    revealedApiKey === null;
  const apiKeyValue =
    revealedApiKey ??
    (showingSavedApiKey ? SAVED_API_KEY_MASK : optimization.apiKey);
  const apiKeyTextVisible = apiKeyVisible && !showingSavedApiKey;
  const apiKeyInputId = `${controlId}-api-key`;
  const modelInputId = `${controlId}-model`;
  const modelSelectOptions = useMemo(
    () => [
      ...(optimization.model.trim() !== "" &&
      !cloudModels.includes(optimization.model)
        ? [{ label: optimization.model, value: optimization.model }]
        : []),
      ...cloudModels.map((model) => ({ label: model, value: model })),
    ],
    [cloudModels, optimization.model],
  );
  // Semi Select retains stale options when a controlled, creatable Select gets
  // a new optionList. Remount only when the fetched list actually changes.
  const modelSelectKey = useMemo(
    () => JSON.stringify(cloudModels),
    [cloudModels],
  );

  const handleApiKeyChange = (value: string) => {
    setRevealedApiKey(null);
    if (value === "") {
      updateOptimization({
        apiKey: "",
        apiKeyConfigured: false,
        clearApiKey: optimization.apiKeyConfigured,
      });
      return;
    }
    if (showingSavedApiKey) {
      if (/^\*+$/.test(value)) {
        updateOptimization({
          apiKey: "",
          apiKeyConfigured: true,
          clearApiKey: false,
        });
        return;
      }
    }
    const nextValue = showingSavedApiKey
      ? value.replace(SAVED_API_KEY_MASK, "")
      : value;
    updateOptimization({
      apiKey: nextValue,
      apiKeyConfigured: nextValue.trim() !== "",
      clearApiKey: false,
    });
  };

  const toggleApiKeyVisibility = async () => {
    if (apiKeyTextVisible) {
      setApiKeyVisible(false);
      return;
    }
    if (showingSavedApiKey) {
      setRevealingApiKey(true);
      setSyncResult({ tone: "idle", text: "" });
      try {
        const result = await invoke<{ apiKey?: string }>(
          "reveal_prompt_optimization_api_key",
        );
        setRevealedApiKey(result.apiKey ?? "");
      } catch (error) {
        setSyncResult({
          tone: "error",
          text: `无法回显 API Key：${errorText(error)}`,
        });
        return;
      } finally {
        setRevealingApiKey(false);
      }
    }
    setApiKeyVisible(true);
  };

  const clearModelSuggestions = () => {
    setCloudModels([]);
    setModelsResult({ tone: "idle", text: "" });
  };

  const updateModel = (model: string) => {
    updateOptimization({ model });
  };

  const runSyncCurrentProvider = async () => {
    if (busy || provider.official || activeOperationRef.current) return;
    activeOperationRef.current = "sync";
    setSyncing(true);
    setSyncResult({ tone: "pending", text: "正在同步当前线路配置…" });
    setTestResult({ tone: "idle", text: "" });
    try {
      const synced = await onSyncCurrentProvider();
      if (!synced) return;
      setRevealedApiKey(null);
      setApiKeyVisible(false);
      setCloudModels([]);
      setModelsResult({ tone: "idle", text: "" });
      setSyncResult({
        tone: "success",
        text: `已同步「${provider.name}」的地址、密钥、Responses API 和默认模型`,
      });
    } catch (error) {
      setSyncResult({ tone: "error", text: errorText(error) });
    } finally {
      activeOperationRef.current = null;
      setSyncing(false);
    }
  };

  const runFetchModels = async () => {
    if (busy || activeOperationRef.current) return;
    activeOperationRef.current = "models";
    const requestId = requestSequenceRef.current + 1;
    requestSequenceRef.current = requestId;
    setFetchingModels(true);
    setModelsResult({
      tone: "pending",
      text: "正在获取模型列表…",
    });
    try {
      const result = await withTimeout(
        invoke<{ models?: string[] }>("fetch_prompt_optimization_models", {
          config: optimization,
        }),
        FETCH_MODELS_TIMEOUT_MS,
        "获取模型列表超时，请检查 API 地址与网络",
      );
      if (requestSequenceRef.current !== requestId) return;
      const models = result?.models ?? [];
      setCloudModels(models);
      setModelsResult(
        models.length > 0
          ? { tone: "success", text: `已获取 ${models.length} 个模型` }
          : { tone: "error", text: "服务端没有返回可用模型" },
      );
    } catch (error) {
      if (requestSequenceRef.current === requestId) {
        setModelsResult({ tone: "error", text: errorText(error) });
      }
    } finally {
      if (requestSequenceRef.current === requestId) {
        activeOperationRef.current = null;
        setFetchingModels(false);
      }
    }
  };

  const runTest = async () => {
    if (busy || activeOperationRef.current) return;
    activeOperationRef.current = "test";
    const requestId = requestSequenceRef.current + 1;
    requestSequenceRef.current = requestId;
    setTesting(true);
    setSyncResult({ tone: "idle", text: "" });
    setTestResult({ tone: "pending", text: "正在测试 API 连通性…" });
    try {
      // 测试直接使用当前编辑的草稿，无需先保存；已保存的 API Key
      // 会由后端在草稿基础上自动回填。
      const result = await withTimeout(
        invoke<{ result?: TestResult }>("test_prompt_optimization", {
          config: optimization,
        }),
        TEST_TIMEOUT_MS,
        "测试超时，请检查 API 地址与网络",
      );
      if (requestSequenceRef.current !== requestId) return;
      const httpStatus = result?.result?.httpStatus;
      const responsePreview = result?.result?.responsePreview?.trim();
      if (typeof httpStatus === "number" && httpStatus >= 400) {
        setTestResult({
          tone: "error",
          text: responsePreview
            ? `连接失败（HTTP ${httpStatus}）：${responsePreview}`
            : `连接失败（HTTP ${httpStatus}）`,
        });
        return;
      }
      setTestResult({
        tone: "success",
        text:
          typeof httpStatus === "number"
            ? `连接成功（HTTP ${httpStatus}）`
            : "连接成功",
      });
    } catch (error) {
      if (requestSequenceRef.current === requestId) {
        setTestResult({ tone: "error", text: errorText(error) });
      }
    } finally {
      if (requestSequenceRef.current === requestId) {
        activeOperationRef.current = null;
        setTesting(false);
      }
    }
  };

  return (
    <section
      className="secondary-section"
      aria-labelledby="prompt-optimization-title"
    >
      <div className="section-title compact">
        <div className="section-heading">
          <span className="section-icon" aria-hidden="true">
            <IconSparkles size={15} />
          </span>
          <div>
            <h2 id="prompt-optimization-title">提示词优化</h2>
            <p>在 Codex 输入框旁一键重写与优化提示词。</p>
          </div>
        </div>
      </div>
      <Card className="secondary-card prompt-optimization-card">
        <div
          className={`feature-card prompt-optimization-toggle${optimization.enabled ? " active" : ""}`}
        >
          <div className="feature-card-header">
            <div className="feature-card-title">
              <IconSparkles size={16} aria-hidden="true" />
              <strong>启用提示词优化</strong>
            </div>
            <Switch
              checked={optimization.enabled}
              disabled={isBusy}
              aria-label="启用提示词优化"
              onCheckedChange={(checked) =>
                updateOptimization({ enabled: checked })
              }
            />
          </div>
        </div>

        {optimization.enabled ? (
          <div className="prompt-optimization-fields">
            <div className="prompt-optimization-actions-row">
              <div className="prompt-optimization-action-result">
                {syncResult.text ? (
                  <span className={`inline-result ${syncResult.tone}`}>
                    {syncResult.text}
                  </span>
                ) : testResult.text ? (
                  <span className={`inline-result ${testResult.tone}`}>
                    {testResult.text}
                  </span>
                ) : null}
              </div>
              <div className="prompt-optimization-action-buttons">
                {!provider.official ? (
                  <Button
                    variant="secondary"
                    size="xs"
                    disabled={isBusy || testing || fetchingModels}
                    onClick={() => void runSyncCurrentProvider()}
                  >
                    <IconRefresh
                      className={
                        syncing || busy === "sync-prompt-provider"
                          ? "spinner"
                          : ""
                      }
                      aria-hidden="true"
                    />
                    {syncing ? "同步中…" : "同步当前线路配置"}
                  </Button>
                ) : null}
                <Button
                  variant="secondary"
                  size="xs"
                  disabled={isBusy || testing || fetchingModels}
                  onClick={() => void runTest()}
                >
                  <IconPlugConnected aria-hidden="true" />
                  {testing ? "测试中…" : "测试 API 连通性"}
                </Button>
              </div>
            </div>

            <div className="prompt-optimization-config-grid">
              <label className="field prompt-optimization-address-field">
                <span>API 地址</span>
                <div className="input-shell">
                  <IconWorld size={15} aria-hidden="true" />
                  <Input
                    value={optimization.baseUrl}
                    disabled={isBusy}
                    onChange={(event) => {
                      clearModelSuggestions();
                      updateOptimization({ baseUrl: event.target.value });
                    }}
                    placeholder="https://api.openai.com/v1"
                    spellCheck={false}
                  />
                </div>
              </label>

              <div className="field prompt-optimization-key-field">
                <label htmlFor={apiKeyInputId}>API Key</label>
                <div className="input-shell">
                  <IconKey size={15} aria-hidden="true" />
                  <input
                    id={apiKeyInputId}
                    type={apiKeyTextVisible ? "text" : "password"}
                    className="prompt-optimization-secret-input"
                    value={apiKeyValue}
                    disabled={isBusy}
                    onChange={(event) => {
                      clearModelSuggestions();
                      handleApiKeyChange(event.target.value);
                    }}
                    onFocus={(event) => {
                      if (showingSavedApiKey) event.currentTarget.select();
                    }}
                    placeholder={
                      optimization.apiKeyConfigured
                        ? "已保存（输入新 Key 可替换）"
                        : "sk-…"
                    }
                    autoComplete="new-password"
                    spellCheck={false}
                  />
                  <Button
                    variant="ghost"
                    size="icon"
                    className="prompt-optimization-icon-button"
                    disabled={isBusy || revealingApiKey}
                    aria-label={
                      apiKeyTextVisible ? "隐藏 API Key" : "显示 API Key"
                    }
                    title={
                      revealingApiKey
                        ? "正在读取 API Key"
                        : apiKeyTextVisible
                          ? "隐藏 API Key"
                          : "显示 API Key"
                    }
                    onClick={() => void toggleApiKeyVisibility()}
                  >
                    {apiKeyTextVisible ? (
                      <IconEyeOff size={15} aria-hidden="true" />
                    ) : (
                      <IconEye size={15} aria-hidden="true" />
                    )}
                  </Button>
                </div>
              </div>

              <div className="field prompt-optimization-model-field">
                <label htmlFor={modelInputId}>模型</label>
                <div className="prompt-optimization-model-wrapper">
                  <div className="prompt-optimization-model-control">
                    <div className="prompt-optimization-model-picker">
                      <div className="input-shell prompt-optimization-model-row">
                        <IconRobot size={15} aria-hidden="true" />
                        <Select
                          key={modelSelectKey}
                          id={modelInputId}
                          className="prompt-optimization-model-select"
                          value={optimization.model || undefined}
                          disabled={isBusy || fetchingModels}
                          aria-label="提示词优化模型"
                          optionList={modelSelectOptions}
                          placeholder="gpt-4o-mini"
                          dropdownClassName="prompt-optimization-model-dropdown"
                          emptyContent={
                            cloudModels.length > 0
                              ? "没有匹配模型"
                              : "暂无模型列表，可输入后回车创建"
                          }
                          showClear={false}
                          filter
                          allowCreate
                          searchPosition="trigger"
                          getPopupContainer={() => popupContainer ?? document.body}
                          zIndex={SETTINGS_OVERLAY_Z_INDEX}
                          renderCreateItem={(inputValue, focused, style) =>
                            inputValue ? (
                              <div
                                className={`prompt-optimization-model-create-option${focused ? " focused" : ""}`}
                                style={style}
                              >
                                <IconPlus size={14} aria-hidden="true" />
                                <span className="prompt-optimization-model-create-label">
                                  使用
                                </span>
                                <span className="prompt-optimization-model-create-value">
                                  {String(inputValue)}
                                </span>
                              </div>
                            ) : null
                          }
                          onChange={(value) => updateModel(String(value ?? ""))}
                          onCreate={(option) =>
                            updateModel(String(option.value ?? ""))
                          }
                        />
                      </div>
                    </div>
                    <Button
                      variant="secondary"
                      size="xs"
                      disabled={isBusy || fetchingModels || testing}
                      onClick={() => void runFetchModels()}
                    >
                      {fetchingModels ? "获取中…" : "获取列表"}
                    </Button>
                  </div>
                  {modelsResult.text ? (
                    <span className={`inline-result ${modelsResult.tone}`}>
                      {modelsResult.text}
                    </span>
                  ) : null}
                </div>
              </div>

              <label className="field prompt-optimization-instruction-field">
                <span>优化指令</span>
                <textarea
                  className="prompt-optimization-instruction"
                  value={
                    optimization.instruction || DEFAULT_OPTIMIZER_INSTRUCTION
                  }
                  disabled={isBusy}
                  onChange={(event) =>
                    updateOptimization({ instruction: event.target.value })
                  }
                  rows={3}
                  placeholder="自定义优化指令…"
                  spellCheck={false}
                />
              </label>
            </div>
          </div>
        ) : null}
      </Card>
    </section>
  );
}

export const PromptOptimizationCard = memo(PromptOptimizationCardComponent);
