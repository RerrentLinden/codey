import { memo, useRef, useState } from "react";

import { IconKey, IconRobot, IconSparkles, IconWorld } from "@tabler/icons-react";

import type { Config, InlineResult } from "./App.types";
import { invoke } from "./api";
import { errorText, withTimeout } from "./appUtils";
import { AutoComplete, Button, Card, Input, Switch } from "./components/semi";

const TEST_TIMEOUT_MS = 65_000;
const FETCH_MODELS_TIMEOUT_MS = 20_000;

type PromptOptimizationCardProps = {
  config: Config;
  isBusy: boolean;
  busy: string | null;
  /** Popup container inside the overlay shadow DOM; dropdowns rendered into
   * `document.body` would be hidden behind the overlay host. */
  container?: HTMLElement | null;
  onConfigChange: (config: Config) => void;
};

type TestResult = {
  httpStatus?: number;
  responsePreview?: string;
};

function PromptOptimizationCardComponent({
  config,
  isBusy,
  busy,
  container,
  onConfigChange,
}: PromptOptimizationCardProps) {
  const optimization = config.promptOptimization;
  const requestSequenceRef = useRef(0);
  const activeOperationRef = useRef<"models" | "test" | null>(null);
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

  const runFetchModels = async () => {
    if (busy || activeOperationRef.current) return;
    activeOperationRef.current = "models";
    const requestId = requestSequenceRef.current + 1;
    requestSequenceRef.current = requestId;
    setFetchingModels(true);
    setModelsResult({ tone: "pending", text: "正在获取云端模型…" });
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
        <div>
          <h2 id="prompt-optimization-title">提示词优化</h2>
          <p>在 Codex 输入框旁一键优化提示词，使用任意 OpenAI 兼容接口。</p>
        </div>
      </div>
      <Card className="secondary-card prompt-optimization-card">
        <div className="feature-card prompt-optimization-toggle">
          <div className="feature-card-header">
            <div className="feature-card-title">
              <IconSparkles size={15} aria-hidden="true" />
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
          <div className="feature-card-body">
            <small>
              启用后，Codex 输入框旁会出现「优化」按钮；输入内容后点击即可
              调用 API 重写提示词并直接替换输入框内容。配置变更即时生效，
              无需重启 Codex。
            </small>
          </div>
        </div>
        <div className="prompt-optimization-fields">
          <label className="field">
            <span>API 地址</span>
            <div className="input-shell">
              <IconWorld size={15} aria-hidden="true" />
              <Input
                value={optimization.baseUrl}
                disabled={isBusy}
                onChange={(event) =>
                  updateOptimization({ baseUrl: event.target.value })
                }
                placeholder="https://api.openai.com/v1（也可直接填完整的 /chat/completions 地址）"
                spellCheck={false}
              />
            </div>
          </label>
          <label className="field">
            <span>模型</span>
            <div className="input-shell prompt-optimization-model-row">
              <IconRobot size={15} aria-hidden="true" />
              <AutoComplete
                value={optimization.model}
                disabled={isBusy || fetchingModels}
                data={cloudModels}
                getPopupContainer={container ? () => container : undefined}
                onChange={(value) =>
                  updateOptimization({ model: String(value ?? "") })
                }
                onSelect={(value) =>
                  updateOptimization({ model: String(value ?? "") })
                }
                placeholder="gpt-4o-mini（可输入，也可从云端获取后选择）"
              />
              <Button
                variant="secondary"
                size="xs"
                disabled={isBusy || fetchingModels || testing}
                onClick={() => void runFetchModels()}
              >
                {fetchingModels ? "获取中…" : "获取列表"}
              </Button>
            </div>
            <span className={`inline-result ${modelsResult.tone}`}>
              {modelsResult.text}
            </span>
          </label>
          <label className="field">
            <span>API Key</span>
            <div className="input-shell">
              <IconKey size={15} aria-hidden="true" />
              <Input
                type="password"
                value={optimization.apiKey}
                disabled={isBusy}
                onChange={(event) =>
                  updateOptimization({
                    apiKey: event.target.value,
                    clearApiKey: false,
                  })
                }
                placeholder={
                  optimization.apiKeyConfigured
                    ? "已保存；输入新 Key 可替换"
                    : "sk-…"
                }
                autoComplete="new-password"
                spellCheck={false}
              />
            </div>
          </label>
          {optimization.apiKeyConfigured ? (
            <div className="notification-secret-action">
              <Button
                variant="ghost"
                size="xs"
                disabled={isBusy}
                onClick={() =>
                  updateOptimization({
                    apiKey: "",
                    apiKeyConfigured: false,
                    clearApiKey: true,
                  })
                }
              >
                清除已保存的 API Key
              </Button>
            </div>
          ) : null}
          <label className="field">
            <span>自定义优化指令（可选）</span>
            <textarea
              className="prompt-optimization-instruction"
              value={optimization.instruction}
              disabled={isBusy}
              onChange={(event) =>
                updateOptimization({ instruction: event.target.value })
              }
              placeholder="留空使用内置指令：只输出优化后的提示词本身，不添加解释、前言或代码围栏"
              rows={3}
              spellCheck={false}
            />
          </label>
          <div className="prompt-optimization-test-row">
            <Button
              variant="secondary"
              size="xs"
              disabled={isBusy || testing || fetchingModels}
              onClick={() => void runTest()}
            >
              {testing ? "测试中…" : "测试 API 连通性"}
            </Button>
            <span className={`inline-result ${testResult.tone}`}>
              {testResult.text}
            </span>
          </div>
        </div>
      </Card>
    </section>
  );
}

export const PromptOptimizationCard = memo(PromptOptimizationCardComponent);
