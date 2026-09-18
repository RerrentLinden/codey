import { useCallback, useEffect, useState } from "react";
import { Card } from "@heroui/react";
import { IconPuzzle } from "@tabler/icons-react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import { Badge, Button } from "./components/ui";
import { CodeyPluginsDialog } from "./CodeyPluginsDialog";
import { pluginStatusLabel, type CodeyPluginsResult } from "./codeyPlugins";
import { surfaceCardPaddingClass } from "./uiClasses";

export function CodeyPluginsSection({ container }: { container?: HTMLElement | null }) {
  const [result, setResult] = useState<CodeyPluginsResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError("");
    void invoke<CodeyPluginsResult>("list_codey_plugins")
      .then(data => {
        if (!data || !Array.isArray(data.plugins)) throw new Error("插件列表响应无效，请刷新或更新 Codey 后重试。");
        if (!cancelled) setResult(data);
      })
      .catch(cause => { if (!cancelled) setError(errorText(cause)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [revision]);
  const acceptResult = useCallback((data: CodeyPluginsResult) => {
    if (!data || !Array.isArray(data.plugins)) {
      setError("插件列表响应无效，请刷新或更新 Codey 后重试。");
      return;
    }
    setResult(data);
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
        <p className="m-0 text-sm text-muted" role="status">{loading ? "正在读取插件…" : error ? "插件列表读取失败" : result ? `已安装 ${result.plugins.length} 个插件 · 已启用 ${result.plugins.filter(plugin => plugin.enabled).length} 个` : "尚未读取插件"}</p>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" disabled={loading} onClick={() => setRevision(value => value + 1)}>刷新</Button>
          <Button size="sm" onClick={() => setOpen(true)}>管理插件</Button>
        </div>
      </div>
      {error && <p role="alert" className="mb-0 break-words text-sm text-red-600 dark:text-red-400">{error}</p>}
      {!loading && !error && result?.plugins.length === 0 && <div className="mt-4 rounded-xl border border-dashed border-gray-200 px-4 py-7 text-center dark:border-gray-700">
        <p className="m-0 text-sm font-medium">尚未安装插件</p>
        <p className="mb-0 mt-2 text-xs text-muted">打开管理插件，导入 .codey-plugin 文件以添加功能。</p>
      </div>}
      {!!result?.plugins.length && <ul className="m-0 mt-4 grid list-none gap-3 p-0 sm:grid-cols-2">
        {result.plugins.map(plugin => {
          const failed = Boolean(plugin.lastError || plugin.status === "failed" || plugin.status === "error");
          return <li key={plugin.id} className="min-w-0 rounded-xl border border-gray-200 p-4 dark:border-gray-700">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="min-w-0 break-words text-sm font-semibold">{plugin.name} <span className="font-normal text-muted">{plugin.version}</span></div>
              <Badge variant={failed ? "destructive" : plugin.restartRequired ? "warning" : plugin.enabled ? "success" : "secondary"}>{pluginStatusLabel(plugin)}</Badge>
            </div>
            {plugin.description && <p className="mb-0 mt-2 break-words text-xs text-muted">{plugin.description}</p>}
            {plugin.lastError && <p role="alert" className="mb-0 mt-2 break-words text-xs text-red-600 dark:text-red-400">{plugin.lastError}</p>}
          </li>;
        })}
      </ul>}
    </Card>
    <CodeyPluginsDialog open={open} onChanged={acceptResult} onClose={() => { setOpen(false); setRevision(value => value + 1); }} container={container} />
  </section>;
}
