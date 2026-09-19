import { useCallback, useEffect, useRef, useState } from "react";
import { Card } from "@heroui/react";
import { IconPuzzle, IconRefresh } from "@tabler/icons-react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import { Button } from "./components/ui";
import { CodeyPluginsDialog } from "./CodeyPluginsDialog";
import { parseCodeyPluginsResult, type CodeyPluginsResult } from "./codeyPlugins";
import { surfaceCardPaddingClass } from "./uiClasses";

export function CodeyPluginsSection({ container }: { container?: HTMLElement | null }) {
  const [result, setResult] = useState<CodeyPluginsResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  const listEpoch = useRef(0);
  useEffect(() => {
    let cancelled = false;
    const generation = ++listEpoch.current;
    setLoading(true);
    setError("");
    void invoke<CodeyPluginsResult>("list_codey_plugins")
      .then(data => {
        data = parseCodeyPluginsResult(data);
        if (!cancelled && generation === listEpoch.current) setResult(data);
      })
      .catch(cause => { if (!cancelled && generation === listEpoch.current) { setResult(null); setError(errorText(cause)); } })
      .finally(() => { if (!cancelled && generation === listEpoch.current) setLoading(false); });
    return () => { cancelled = true; };
  }, [revision]);
  const acceptResult = useCallback((data: CodeyPluginsResult) => {
    data = parseCodeyPluginsResult(data);
    setResult(data);
    listEpoch.current++;
    setLoading(false);
    setError("");
  }, []);
  return <section className="secondary-section" aria-labelledby="codey-plugins-title">
    <div className="section-title compact">
      <div className="section-heading">
        <span className="section-icon" aria-hidden="true"><IconPuzzle size={15} /></span>
        <div><h2 id="codey-plugins-title">Codey 插件</h2><p>按需安装独立功能模块，统一管理插件与配置。</p></div>
      </div>
    </div>
    <Card className={`secondary-card ${surfaceCardPaddingClass}`}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2 text-sm text-muted" role="status">
          {loading ? (
            <span>正在读取插件…</span>
          ) : result ? (
            <>
              <span>已安装 <strong className="font-semibold text-gray-900 dark:text-gray-100">{result.plugins.length}</strong> 个插件</span>
              <span className="text-muted/40">·</span>
              <span>已启用 <strong className="font-semibold text-gray-900 dark:text-gray-100">{result.plugins.filter(plugin => plugin.enabled).length}</strong> 个</span>
            </>
          ) : error ? (
            <span className="text-red-600 dark:text-red-400">插件状态未知，请刷新重试</span>
          ) : (
            <span>尚未读取插件</span>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={loading || open}
            onClick={() => setRevision(value => value + 1)}
            aria-label="刷新插件列表"
          >
            <IconRefresh size={14} className={loading ? "animate-spin" : ""} />
            <span>刷新</span>
          </Button>
          <Button size="sm" onClick={() => setOpen(true)}>
            管理插件
          </Button>
        </div>
      </div>
      {error && <p role="alert" className="mb-0 break-words text-sm text-red-600 dark:text-red-400">{error}</p>}
    </Card>
    <CodeyPluginsDialog open={open} onChanged={acceptResult} onClose={() => { setOpen(false); setRevision(value => value + 1); }} container={container} />
  </section>;
}
