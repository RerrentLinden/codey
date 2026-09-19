import { useCallback, useEffect, useRef, useState } from "react";
import { Card } from "@heroui/react";
import {
  IconAlertCircle,
  IconAlertTriangle,
  IconInfoCircle,
  IconPuzzle,
  IconRefresh,
  IconSettings,
  IconTrash,
} from "@tabler/icons-react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  Switch,
  Tooltip,
} from "./components/ui";
import { PluginConfigDialog } from "./PluginConfigDialog";
import { CodeyPluginsDialog } from "./CodeyPluginsDialog";
import { pluginStatusLabel, type CodeyPlugin, type CodeyPluginsResult } from "./codeyPlugins";
import { surfaceCardPaddingClass } from "./uiClasses";

export function CodeyPluginsSection({ container }: { container?: HTMLElement | null }) {
  const [result, setResult] = useState<CodeyPluginsResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  const [editId, setEditId] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ kind: "enable" | "uninstall"; plugin: CodeyPlugin } | null>(null);
  const [removeData, setRemoveData] = useState(false);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const listEpoch = useRef(0);
  useEffect(() => {
    let cancelled = false;
    const generation = ++listEpoch.current;
    setLoading(true);
    setError("");
    void invoke<CodeyPluginsResult>("list_codey_plugins")
      .then(data => {
        if (!data || !Array.isArray(data.plugins)) throw new Error("插件列表响应无效，请刷新或更新 Codey 后重试。");
        if (!cancelled && generation === listEpoch.current) setResult(data);
      })
      .catch(cause => { if (!cancelled && generation === listEpoch.current) setError(errorText(cause)); })
      .finally(() => { if (!cancelled && generation === listEpoch.current) setLoading(false); });
    return () => { cancelled = true; };
  }, [revision]);
  const acceptResult = useCallback((data: CodeyPluginsResult) => {
    if (!data || !Array.isArray(data.plugins)) {
      setError("插件列表响应无效，请刷新或更新 Codey 后重试。");
      return;
    }
    setResult(data);
    listEpoch.current++;
    setLoading(false);
    setError("");
  }, []);
  async function toggle(plugin: CodeyPlugin, enabled: boolean) {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError("");
    try {
      acceptResult(await invoke<CodeyPluginsResult>("set_codey_plugin_enabled", { pluginId: plugin.id, enabled }));
      setConfirm(null);
    } catch (cause) { setError(errorText(cause)); }
    finally { pending.current = false; setBusy(false); }
  }
  async function uninstall(plugin: CodeyPlugin, shouldRemoveData: boolean) {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError("");
    try {
      acceptResult(await invoke<CodeyPluginsResult>("uninstall_codey_plugin", { pluginId: plugin.id, removeData: shouldRemoveData }));
      setConfirm(null);
      if (editId === plugin.id) setEditId(null);
    } catch (cause) { setError(errorText(cause)); }
    finally { pending.current = false; setBusy(false); }
  }
  const editing = result?.plugins.find(plugin => plugin.id === editId);
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
            <span className="text-red-600 dark:text-red-400">插件列表读取失败</span>
          ) : (
            <span>尚未读取插件</span>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={loading || busy}
            onClick={() => setRevision(value => value + 1)}
            aria-label="刷新插件列表"
          >
            <IconRefresh size={14} className={loading ? "animate-spin" : ""} />
            <span>刷新</span>
          </Button>
          <Button size="sm" disabled={busy} onClick={() => setOpen(true)}>
            管理插件
          </Button>
        </div>
      </div>
      {error && <p role="alert" className="mb-0 break-words text-sm text-red-600 dark:text-red-400">{error}</p>}
      {!loading && !error && result?.plugins.length === 0 && (
        <div className="mt-4 flex flex-col items-center justify-center rounded-2xl border border-dashed border-gray-200/90 py-10 px-4 text-center dark:border-gray-800">
          <div className="size-11 rounded-2xl bg-black/[0.03] dark:bg-white/[0.04] flex items-center justify-center text-muted mb-2.5">
            <IconPuzzle size={22} stroke={1.5} />
          </div>
          <p className="m-0 text-sm font-medium text-gray-900 dark:text-gray-100">尚未安装任何插件</p>
          <p className="mb-0 mt-1 text-xs text-muted max-w-sm">
            点击上方「管理插件」导入 <code className="font-mono text-[11px] px-1.5 py-0.5 rounded bg-black/[0.04] dark:bg-white/[0.06]">.codey-plugin</code> 文件即可扩展功能。
          </p>
        </div>
      )}
      {!!result?.plugins.length && (
        <ul className="m-0 mt-4 grid list-none gap-3.5 p-0 sm:grid-cols-2">
          {result.plugins.map(plugin => {
            const failed = Boolean(plugin.lastError || plugin.status === "failed" || plugin.status === "error");
            return (
              <li
                key={plugin.id}
                className={`group relative flex flex-col justify-between rounded-2xl border p-4 transition-all duration-200 ${
                  failed
                    ? "border-red-500/35 bg-red-50/30 dark:border-red-400/30 dark:bg-red-950/20"
                    : plugin.restartRequired
                    ? "border-amber-500/35 bg-amber-50/30 dark:border-amber-400/30 dark:bg-amber-950/20"
                    : plugin.enabled
                    ? "border-blue-500/30 bg-blue-50/25 dark:border-blue-400/30 dark:bg-blue-950/20 shadow-xs"
                    : "border-gray-200/90 bg-white dark:border-gray-700/70 dark:bg-[#252529]/60 hover:border-gray-300 dark:hover:border-gray-600 hover:shadow-xs"
                }`}
              >
                <div>
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex items-start gap-3 min-w-0 flex-1">
                      <div
                        className={`size-10 rounded-xl flex items-center justify-center shrink-0 transition-colors ${
                          failed
                            ? "bg-red-100 text-red-600 dark:bg-red-950/60 dark:text-red-400 ring-1 ring-red-500/20"
                            : plugin.restartRequired
                            ? "bg-amber-100 text-amber-600 dark:bg-amber-950/60 dark:text-amber-400 ring-1 ring-amber-500/20"
                            : plugin.enabled
                            ? "bg-blue-50 text-blue-600 dark:bg-blue-950/60 dark:text-blue-400 ring-1 ring-blue-500/20"
                            : "bg-gray-100 text-gray-400 dark:bg-gray-800 dark:text-gray-500 ring-1 ring-black/[0.04] dark:ring-white/[0.06]"
                        }`}
                      >
                        <IconPuzzle size={20} stroke={1.75} aria-hidden="true" />
                      </div>

                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <span className="font-semibold text-sm text-gray-900 dark:text-gray-100 truncate tracking-tight">
                            {plugin.name}
                          </span>
                          <span className="inline-flex items-center px-1.5 py-0.5 rounded text-[10.5px] font-mono font-medium bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400 border border-black/[0.04] dark:border-white/[0.06] shrink-0">
                            v{plugin.version}
                          </span>
                          <Tooltip
                            content={
                              <div className="grid max-w-[min(360px,calc(100vw-48px))] gap-1.5 break-all py-1 text-xs">
                                <div className="font-medium text-gray-900 dark:text-gray-100 mb-0.5">{plugin.name}</div>
                                {plugin.pluginDir || plugin.dataDir || plugin.logDir ? (
                                  <>
                                    {plugin.pluginDir && <div><span className="text-muted">插件目录：</span><span className="select-text font-mono text-[11px]">{plugin.pluginDir}</span></div>}
                                    {plugin.dataDir && <div><span className="text-muted">数据目录：</span><span className="select-text font-mono text-[11px]">{plugin.dataDir}</span></div>}
                                    {plugin.logDir && <div><span className="text-muted">日志目录：</span><span className="select-text font-mono text-[11px]">{plugin.logDir}</span></div>}
                                  </>
                                ) : (
                                  <div>插件文件位置未提供</div>
                                )}
                              </div>
                            }
                          >
                            <Button
                              variant="ghost"
                              size="icon-sm"
                              className="size-5 min-w-5 p-0 text-muted hover:text-foreground shrink-0 rounded-md"
                              aria-label={`${plugin.name} 文件位置`}
                            >
                              <IconInfoCircle size={14} aria-hidden="true" />
                            </Button>
                          </Tooltip>
                        </div>

                        <div className="mt-0.5 flex items-center gap-1.5 text-[11px] text-muted">
                          <span className="truncate font-mono opacity-70" title={plugin.id}>
                            {plugin.id}
                          </span>
                          {plugin.activeVersion && plugin.activeVersion !== plugin.version && (
                            <span className="text-[10px] text-amber-600 dark:text-amber-400 shrink-0 font-medium">
                              (运行中: v{plugin.activeVersion})
                            </span>
                          )}
                        </div>
                      </div>
                    </div>

                    <div className="shrink-0 pt-0.5">
                      <Switch
                        checked={plugin.enabled}
                        disabled={busy || loading}
                        aria-label={`启用 ${plugin.name}`}
                        onCheckedChange={enabled => {
                          if (enabled) {
                            setError("");
                            setConfirm({ kind: "enable", plugin });
                          } else {
                            void toggle(plugin, false);
                          }
                        }}
                      />
                    </div>
                  </div>

                  <div className="mt-3 min-h-[34px] flex flex-col justify-center">
                    {plugin.description ? (
                      <p className="m-0 break-words text-xs text-muted leading-relaxed line-clamp-2">
                        {plugin.description}
                      </p>
                    ) : plugin.capabilities && plugin.capabilities.length > 0 ? (
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="text-[10.5px] text-muted/70">权限声明:</span>
                        {plugin.capabilities.map(cap => (
                          <span
                            key={cap}
                            className="inline-flex items-center rounded bg-black/[0.04] px-1.5 py-0.5 font-mono text-[10.5px] text-muted dark:bg-white/[0.06]"
                          >
                            {cap}
                          </span>
                        ))}
                      </div>
                    ) : (
                      <p className="m-0 text-[11.5px] italic text-muted/60">
                        独立功能扩展模块
                      </p>
                    )}
                  </div>

                  {plugin.restartRequired && (
                    <div className="mt-2.5 flex items-center gap-1.5 rounded-lg bg-amber-500/10 border border-amber-500/20 px-2.5 py-1.5 text-xs text-amber-800 dark:text-amber-300">
                      <IconAlertTriangle size={14} className="shrink-0" />
                      <span>需停用并重新启用以应用配置或版本变更</span>
                    </div>
                  )}
                  {plugin.lastError && (
                    <div
                      role="alert"
                      className="mt-2.5 flex items-start gap-1.5 rounded-lg bg-red-500/10 border border-red-500/20 px-2.5 py-1.5 text-xs text-red-600 dark:text-red-400 break-words"
                    >
                      <IconAlertCircle size={14} className="shrink-0 mt-0.5" />
                      <span className="line-clamp-2">{plugin.lastError}</span>
                    </div>
                  )}
                </div>

                <div className="mt-3.5 pt-2.5 border-t border-gray-100 dark:border-gray-800/80 flex items-center justify-between gap-2">
                  <Badge
                    variant={failed ? "destructive" : plugin.restartRequired ? "warning" : plugin.enabled ? "success" : "secondary"}
                    className="inline-flex items-center gap-1.5 text-[11px] font-medium py-0.5"
                  >
                    <span
                      className={`size-1.5 rounded-full shrink-0 ${
                        failed
                          ? "bg-red-500"
                          : plugin.restartRequired
                          ? "bg-amber-500 animate-pulse"
                          : plugin.enabled
                          ? "bg-emerald-500"
                          : "bg-gray-400 dark:bg-gray-500"
                      }`}
                    />
                    {pluginStatusLabel(plugin)}
                  </Badge>

                  <div className="flex items-center gap-1">
                    <Button
                      size="icon-sm"
                      variant="ghost"
                      className="size-7 min-w-7 rounded-lg text-muted hover:text-foreground hover:bg-black/[0.05] dark:hover:bg-white/[0.08]"
                      disabled={busy || loading}
                      title={`配置 ${plugin.name}`}
                      aria-label={`配置 ${plugin.name}`}
                      onClick={() => setEditId(plugin.id)}
                    >
                      <IconSettings size={15} aria-hidden="true" />
                    </Button>
                    <Tooltip content={plugin.enabled ? "请先停用插件再卸载" : `卸载 ${plugin.name}`}>
                      <span className="inline-flex">
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          className="size-7 min-w-7 rounded-lg text-muted hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-950/40 dark:hover:text-red-400 disabled:opacity-40 disabled:pointer-events-none"
                          disabled={busy || loading || plugin.enabled}
                          aria-label={`卸载 ${plugin.name}`}
                          onClick={() => {
                            setRemoveData(false);
                            setConfirm({ kind: "uninstall", plugin });
                          }}
                        >
                          <IconTrash size={15} aria-hidden="true" />
                        </Button>
                      </span>
                    </Tooltip>
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
    <CodeyPluginsDialog open={open} onChanged={acceptResult} onClose={() => { setOpen(false); setRevision(value => value + 1); }} container={container} />
    {editing && <PluginConfigDialog key={editing.id} plugin={editing} onClose={() => setEditId(null)} onChanged={acceptResult} container={container} />}
    <Dialog open={!!confirm} onOpenChange={next => { if (!next && !pending.current) setConfirm(null); }}>
      <DialogContent container={container} onEscapeKeyDown={event => { if (pending.current) event.preventDefault(); }}>
        <DialogHeader>
          <DialogTitle>{confirm?.kind === "enable" ? `启用 ${confirm.plugin.name}` : `卸载 ${confirm?.plugin.name}`}</DialogTitle>
          <DialogDescription>
            {confirm?.kind === "enable"
              ? "启用后将执行插件代码，插件具有与 Codey 相同的系统权限。请确认你信任此插件。"
              : "卸载后将移除该插件文件。你可以选择是否一并清理其配置、数据和日志。"}
          </DialogDescription>
        </DialogHeader>
        {error && <p role="alert" className="break-words text-sm text-red-600 dark:text-red-400">{error}</p>}
        {confirm?.kind === "uninstall" && (
          <div className="mt-2">
            <Checkbox
              disabled={busy}
              checked={removeData}
              onCheckedChange={next => setRemoveData(next === true)}
              label="同时删除插件配置、数据和日志（默认保留）"
            />
          </div>
        )}
        <div className="mt-4 flex flex-wrap gap-2">
          <Button
            variant={confirm?.kind === "uninstall" ? "destructive" : "default"}
            disabled={busy}
            onClick={() => {
              if (!confirm) return;
              if (confirm.kind === "enable") void toggle(confirm.plugin, true);
              else void uninstall(confirm.plugin, removeData);
            }}
          >
            {confirm?.kind === "enable" ? "信任并启用" : "确认卸载"}
          </Button>
          <Button variant="outline" disabled={busy} onClick={() => setConfirm(null)}>
            取消
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  </section>;
}
