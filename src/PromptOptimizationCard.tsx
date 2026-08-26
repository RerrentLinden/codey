import { memo, useEffect, useId, useMemo, useRef, useState } from "react";

import {
  IconEye,
  IconEyeOff,
  IconKey,
  IconPlus,
  IconPlugConnected,
  IconRobot,
  IconSparkles,
  IconWorld,
} from "@tabler/icons-react";

import type { Config, InlineResult } from "./App.types";
import { invoke } from "./api";
import { errorText, withTimeout } from "./appUtils";
import { ModelCombobox } from "./components/ModelCombobox";
import { Button, Card, Input, PasswordInput, Select, Switch } from "./components/mantine";
import { SETTINGS_OVERLAY_Z_INDEX } from "./overlay.constants";
import type { SubagentModelOption } from "./subagentModels";
import {
  inputShellClass,
  insetInputClass,
  surfaceCardPaddingClass,
} from "./uiClasses";
import { validateOutboundApiUrl } from "./urlValidation";

const TEST_TIMEOUT_MS = 65_000;
const FETCH_MODELS_TIMEOUT_MS = 20_000;
const DEFAULT_OPTIMIZER_INSTRUCTION =
  "你是提示词优化专家。用户会提供一段提示词，请在不改变其意图的前提下，把它重写为更清晰、更具体、可执行的高质量提示词。只输出优化后的提示词本身，不要添加任何解释、前言、后记或代码围栏。";

const MANUAL_PROTOCOL_OPTIONS = [
  { value: "openaiResponses", label: "OpenAI Responses" },
  { value: "openaiChatCompletions", label: "OpenAI Chat Completions" },
  { value: "anthropicMessages", label: "Anthropic Messages" },
] as const;

type PromptOptimizationCardProps = {
  config: Config;
  isBusy: boolean;
  popupContainer: HTMLElement | null;
  subagentModelOptions: SubagentModelOption[];
  onConfigChange: (config: Config) => void;
};

type TestResult = {
  httpStatus?: number;
  responsePreview?: string;
};

function PromptOptimizationCardComponent({
  config,
  isBusy,
  popupContainer,
  subagentModelOptions,
  onConfigChange,
}: PromptOptimizationCardProps) {
  const optimization = config.promptOptimization;
  const controlId = useId();
  const requestSequenceRef = useRef(0);
  const activeOperationRef = useRef<"models" | "test" | null>(null);
  const [apiKeyVisible, setApiKeyVisible] = useState(false);
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
  const apiKeyInputId = controlId + "-api-key";
  const baseUrlInputId = controlId + "-base-url";
  const modelInputId = controlId + "-model";
  const usesCodeyRoute = optimization.mode === "codeyRoute";
  const hasApiKey = Boolean(
    optimization.apiKey.trim() ||
      (optimization.apiKeyConfigured && !optimization.clearApiKey),
  );
  const baseUrlError =
    !usesCodeyRoute && (optimization.enabled || optimization.baseUrl.trim())
      ? validateOutboundApiUrl(optimization.baseUrl, "API 地址")
      : "";
  const apiKeyError =
    !usesCodeyRoute && optimization.enabled && !hasApiKey
      ? "请输入 API Key"
      : "";
  const modelError =
    optimization.enabled && !optimization.model.trim()
      ? usesCodeyRoute
        ? "请选择 Codey 路由模型"
        : "请选择或填写模型"
      : "";
  const connectionDraftValid = usesCodeyRoute || (!baseUrlError && !apiKeyError);
  const testDraftValid = connectionDraftValid && !modelError;
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

  useEffect(() => {
    setApiKeyVisible(false);
  }, [config.settingsRevision]);

  const clearModelSuggestions = () => {
    setCloudModels([]);
    setModelsResult({ tone: "idle", text: "" });
  };

  const changeMode = (mode: "codeyRoute" | "manual") => {
    if (optimization.mode === mode) return;
    clearModelSuggestions();
    setTestResult({ tone: "idle", text: "" });
    updateOptimization({ mode });
  };

  const handleApiKeyChange = (value: string) => {
    if (value === "") {
      updateOptimization({
        apiKey: "",
        clearApiKey: false,
      });
      return;
    }
    updateOptimization({
      apiKey: value,
      clearApiKey: false,
    });
  };

  const runFetchModels = async () => {
    if (usesCodeyRoute || activeOperationRef.current || !connectionDraftValid) return;
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
          ? { tone: "success", text: "已获取 " + models.length + " 个模型" }
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
    if (activeOperationRef.current || !testDraftValid) return;
    activeOperationRef.current = "test";
    const requestId = requestSequenceRef.current + 1;
    requestSequenceRef.current = requestId;
    setTesting(true);
    setTestResult({ tone: "pending", text: "正在测试 API 连通性…" });
    try {
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
            ? "连接失败（HTTP " + httpStatus + "）：" + responsePreview
            : "连接失败（HTTP " + httpStatus + "）",
        });
        return;
      }
      setTestResult({
        tone: "success",
        text:
          typeof httpStatus === "number"
            ? "连接成功（HTTP " + httpStatus + "）"
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
      className="secondary-section prompt-optimization-section"
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
        <Switch
          checked={optimization.enabled}
          disabled={isBusy}
          aria-label="启用提示词优化"
          onCheckedChange={(checked) =>
            updateOptimization({ enabled: checked })
          }
        />
      </div>
      <Card className={"secondary-card prompt-optimization-card " + surfaceCardPaddingClass}>
        {optimization.enabled ? (
          <div className="prompt-optimization-content">
            <div className="prompt-optimization-toolbar">
              <div className="prompt-optimization-mode-tabs" role="tablist" aria-label="提示词优化配置方式">
                <button
                  type="button"
                  role="tab"
                  aria-selected={usesCodeyRoute}
                  className={
                    "prompt-optimization-mode-tab" +
                    (usesCodeyRoute ? " active" : "")
                  }
                  disabled={isBusy}
                  onClick={() => changeMode("codeyRoute")}
                >
                  使用 Codey 路由
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={!usesCodeyRoute}
                  className={
                    "prompt-optimization-mode-tab" +
                    (!usesCodeyRoute ? " active" : "")
                  }
                  disabled={isBusy}
                  onClick={() => changeMode("manual")}
                >
                  手动配置
                </button>
              </div>

              <div className="prompt-optimization-toolbar-actions">
                {testResult.text ? (
                  <span className={"inline-result " + testResult.tone}>
                    {testResult.text}
                  </span>
                ) : null}
                <Button
                  variant="light"
                  size="xs"
                  className="prompt-test-btn"
                  disabled={isBusy || testing || fetchingModels || !testDraftValid}
                  onClick={() => void runTest()}
                >
                  <IconPlugConnected size={13} aria-hidden="true" />
                  <span>
                    {testing
                      ? "测试中…"
                      : usesCodeyRoute
                        ? "测试路由连通性"
                        : "测试 API 连通性"}
                  </span>
                </Button>
              </div>
            </div>

            <div className="prompt-optimization-form-fields">
              {usesCodeyRoute ? (
                <div className="prompt-form-group">
                  <div className="field prompt-optimization-model-field">
                    <label htmlFor={modelInputId} className="field-label">模型</label>
                    <div className="field-control">
                      <ModelCombobox
                        aria-label="提示词优化 Codey 路由模型"
                        value={optimization.model}
                        placeholder={
                          subagentModelOptions.length === 0
                            ? "所有线路均暂无模型"
                            : "请选择模型"
                        }
                        disabled={isBusy || subagentModelOptions.length === 0}
                        options={subagentModelOptions}
                        getPopupContainer={() => popupContainer ?? document.body}
                        zIndex={SETTINGS_OVERLAY_Z_INDEX}
                        onChange={(model) => updateOptimization({ model })}
                      />
                      {modelError ? (
                        <small id={modelInputId + "-error"} className="field-error" role="alert">
                          {modelError}
                        </small>
                      ) : null}
                      <small className="field-hint">
                        {subagentModelOptions.length === 0
                          ? "请先在模型管理中为任一可用线路启用模型。"
                          : "可搜索并选择模型管理中已启用的任意线路模型。"}
                      </small>
                    </div>
                  </div>

                  <div className="field prompt-optimization-instruction-field">
                    <div className="field-label-wrap">
                      <label htmlFor={controlId + "-instruction"} className="field-label">优化指令</label>
                      {optimization.instruction && optimization.instruction !== DEFAULT_OPTIMIZER_INSTRUCTION ? (
                        <button
                          type="button"
                          className="reset-instruction-btn"
                          onClick={() => updateOptimization({ instruction: DEFAULT_OPTIMIZER_INSTRUCTION })}
                        >
                          恢复默认
                        </button>
                      ) : null}
                    </div>
                    <div className="field-control">
                      <textarea
                        id={controlId + "-instruction"}
                        className="prompt-optimization-instruction"
                        value={optimization.instruction || DEFAULT_OPTIMIZER_INSTRUCTION}
                        disabled={isBusy}
                        onChange={(event) =>
                          updateOptimization({ instruction: event.target.value })
                        }
                        placeholder="自定义优化指令…"
                        spellCheck={false}
                      />
                    </div>
                  </div>
                </div>
              ) : (
                <div className="prompt-form-group">
                  <div className="field prompt-optimization-protocol-field">
                    <label htmlFor={controlId + "-protocol"} className="field-label">上游协议</label>
                    <div className="field-control">
                      <Select
                        id={controlId + "-protocol"}
                        className="w-full min-w-0"
                        value={optimization.upstreamProtocol}
                        disabled={isBusy}
                        aria-label="提示词优化上游协议"
                        optionList={[...MANUAL_PROTOCOL_OPTIONS]}
                        showClear={false}
                        filter={false}
                        dropdownClassName="rounded-[10px]"
                        getPopupContainer={() => popupContainer ?? document.body}
                        zIndex={SETTINGS_OVERLAY_Z_INDEX}
                        onChange={(value) => {
                          clearModelSuggestions();
                          updateOptimization({
                            upstreamProtocol: String(value ?? "openaiResponses") as Config["promptOptimization"]["upstreamProtocol"],
                          });
                        }}
                      />
                    </div>
                  </div>

                  <div className="field prompt-optimization-address-field">
                    <label htmlFor={baseUrlInputId} className="field-label">API 地址</label>
                    <div className="field-control">
                      <div className={inputShellClass}>
                        <IconWorld size={15} aria-hidden="true" />
                        <Input
                          id={baseUrlInputId}
                          className={insetInputClass}
                          value={optimization.baseUrl}
                          disabled={isBusy}
                          aria-invalid={Boolean(baseUrlError)}
                          aria-describedby={baseUrlError ? baseUrlInputId + "-error" : undefined}
                          onChange={(event) => {
                            clearModelSuggestions();
                            updateOptimization({ baseUrl: event.target.value });
                          }}
                          placeholder="https://api.openai.com/v1"
                          spellCheck={false}
                        />
                      </div>
                      {baseUrlError ? (
                        <small id={baseUrlInputId + "-error"} className="field-error" role="alert">
                          {baseUrlError}
                        </small>
                      ) : null}
                    </div>
                  </div>

                  <div className="field prompt-optimization-key-field">
                    <label htmlFor={apiKeyInputId} className="field-label">API Key</label>
                    <div className="field-control">
                      <div className={inputShellClass}>
                        <IconKey size={15} aria-hidden="true" />
                        <PasswordInput
                          id={apiKeyInputId}
                          variant="unstyled"
                          className="min-w-0 flex-1"
                          classNames={{
                            input: insetInputClass + " pr-11!",
                            visibilityToggle:
                              "h-7! w-7! min-w-7! rounded-[7px]! text-[#6e6e73]! hover:bg-black/6! hover:text-[#1d1d1f]!",
                          }}
                          visible={apiKeyVisible}
                          onVisibilityChange={() => setApiKeyVisible((visible) => !visible)}
                          value={optimization.apiKey}
                          disabled={isBusy}
                          aria-invalid={Boolean(apiKeyError)}
                          aria-describedby={apiKeyError ? apiKeyInputId + "-error" : undefined}
                          onChange={(event) => {
                            clearModelSuggestions();
                            handleApiKeyChange(event.target.value);
                          }}
                          placeholder={
                            optimization.apiKeyConfigured &&
                            optimization.apiKey.trim() === ""
                              ? "已保存（点击眼睛查看，或输入新 Key 替换）"
                              : "sk-…"
                          }
                          autoComplete="new-password"
                          spellCheck={false}
                          visibilityToggleIcon={({ reveal }) =>
                            reveal ? (
                              <IconEyeOff size={15} aria-hidden="true" />
                            ) : (
                              <IconEye size={15} aria-hidden="true" />
                            )
                          }
                          visibilityToggleButtonProps={{
                            disabled: isBusy || !optimization.apiKey.trim(),
                            title: apiKeyVisible ? "隐藏 API Key" : "显示 API Key",
                            "aria-label": apiKeyVisible
                              ? "隐藏 API Key"
                              : "显示 API Key",
                          }}
                        />
                      </div>
                      {apiKeyError ? (
                        <small id={apiKeyInputId + "-error"} className="field-error" role="alert">
                          {apiKeyError}
                        </small>
                      ) : optimization.apiKeyConfigured &&
                        !optimization.clearApiKey &&
                        !optimization.apiKey.trim() ? (
                        <small className="field-hint">
                          Key 已保存；点击眼睛可查看，直接输入可替换。
                        </small>
                      ) : null}
                    </div>
                  </div>

                  <div className="field prompt-optimization-model-field">
                    <label htmlFor={modelInputId} className="field-label">模型</label>
                    <div className="field-control">
                      <div className="flex min-w-0 items-center gap-2 max-[680px]:flex-col max-[680px]:items-stretch">
                        <div className="relative min-w-0 flex-1 max-[680px]:w-full">
                          <div className={inputShellClass + " w-full flex-1"}>
                            <IconRobot size={15} aria-hidden="true" />
                            <Select
                              id={modelInputId}
                              className="min-w-0 flex-1"
                              inputClassName={insetInputClass + " font-medium"}
                              optionClassName="min-w-0 truncate"
                              sectionClassName="w-6 text-[#1d1d1f]"
                              value={optimization.model || undefined}
                              disabled={isBusy || fetchingModels}
                              aria-label="提示词优化模型"
                              aria-invalid={Boolean(modelError)}
                              aria-describedby={modelError ? modelInputId + "-error" : undefined}
                              optionList={modelSelectOptions}
                              placeholder="gpt-4o-mini"
                              dropdownClassName="rounded-[10px]"
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
                                    className={
                                      "flex min-h-[34px] w-full items-center gap-[7px] rounded-md px-3 py-[7px] text-[13px] leading-5 text-[#1d1d1f] " +
                                      (focused ? "bg-blue-500/8" : "")
                                    }
                                    style={style}
                                  >
                                    <IconPlus size={14} aria-hidden="true" />
                                    <span className="shrink-0 text-[#6e6e73]">
                                      使用
                                    </span>
                                    <span className="min-w-0 truncate font-semibold text-[#1d1d1f]">
                                      {String(inputValue)}
                                    </span>
                                  </div>
                                ) : null
                              }
                              onChange={(value) =>
                                updateOptimization({ model: String(value ?? "") })
                              }
                              onCreate={(option) =>
                                updateOptimization({
                                  model: String(option.value ?? ""),
                                })
                              }
                            />
                          </div>
                        </div>
                        <Button
                          className="h-[38px]! min-w-[76px] shrink-0 max-[680px]:w-full!"
                          variant="light"
                          size="xs"
                          disabled={
                            isBusy ||
                            fetchingModels ||
                            testing ||
                            !connectionDraftValid
                          }
                          onClick={() => void runFetchModels()}
                        >
                          {fetchingModels ? "获取中…" : "获取列表"}
                        </Button>
                      </div>
                      {modelsResult.text ? (
                        <span className={"inline-result " + modelsResult.tone}>
                          {modelsResult.text}
                        </span>
                      ) : null}
                      {modelError ? (
                        <small id={modelInputId + "-error"} className="field-error" role="alert">
                          {modelError}
                        </small>
                      ) : null}
                    </div>
                  </div>

                  <div className="field prompt-optimization-instruction-field">
                    <div className="field-label-wrap">
                      <label htmlFor={controlId + "-instruction"} className="field-label">优化指令</label>
                      {optimization.instruction && optimization.instruction !== DEFAULT_OPTIMIZER_INSTRUCTION ? (
                        <button
                          type="button"
                          className="reset-instruction-btn"
                          onClick={() => updateOptimization({ instruction: DEFAULT_OPTIMIZER_INSTRUCTION })}
                        >
                          恢复默认
                        </button>
                      ) : null}
                    </div>
                    <div className="field-control">
                      <textarea
                        id={controlId + "-instruction"}
                        className="prompt-optimization-instruction"
                        value={optimization.instruction || DEFAULT_OPTIMIZER_INSTRUCTION}
                        disabled={isBusy}
                        onChange={(event) =>
                          updateOptimization({ instruction: event.target.value })
                        }
                        placeholder="自定义优化指令…"
                        spellCheck={false}
                      />
                    </div>
                  </div>
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="feature-disabled-placeholder">
            <div className="feature-disabled-icon">
              <IconSparkles size={22} aria-hidden="true" />
            </div>
            <div className="feature-disabled-text">
              <strong>提示词优化已关闭</strong>
              <p>开启后，在 Codex 输入框旁可通过快捷按钮一键将自然语言重写为高质量提示词。</p>
            </div>
          </div>
        )}
      </Card>
    </section>
  );
}

export const PromptOptimizationCard = memo(PromptOptimizationCardComponent);
