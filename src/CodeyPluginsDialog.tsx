import { SchemaConfigForm } from "./SchemaConfigForm";
import { useEffect, useRef, useState } from "react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import { Badge, Button, Checkbox, Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, Input } from "./components/ui";
import { pluginStatusLabel, type CodeyPlugin, type CodeyPluginPreview, type CodeyPluginsResult } from "./codeyPlugins";

const panel = "rounded-xl border border-gray-200 dark:border-gray-700 p-4";
function ConfigEditor({ plugin, busy, onSave }: { plugin: CodeyPlugin; busy: boolean; onSave: (config: Record<string, unknown>) => void }) {
  return <section className={`${panel} grid gap-3`} aria-label={`${plugin.name} 配置`}>
    <h3 className="m-0 text-sm font-semibold">插件配置</h3>
    <SchemaConfigForm schema={plugin.configSchema ?? {}} value={plugin.config} disabled={busy} onSave={onSave} />
    <p className="m-0 text-xs text-muted">保存后，正在运行的实例需要停用并重新启用，或重启 Codey 后生效。</p>
  </section>;
}

export function CodeyPluginsDialog({ open, onClose, onChanged, container }: { open: boolean; onClose: () => void; onChanged?: (result: CodeyPluginsResult) => void; container?: HTMLElement | null }) {
  const [result, setResult] = useState<CodeyPluginsResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [path, setPath] = useState("");
  const [preview, setPreview] = useState<CodeyPluginPreview | null>(null);
  const [editId, setEditId] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ kind: "enable" | "uninstall"; plugin: CodeyPlugin } | null>(null);
  const [removeData, setRemoveData] = useState(false);
  const pending = useRef(false);
  const epoch = useRef(0);
  useEffect(() => { if (result) onChanged?.(result); }, [result, onChanged]);
  useEffect(() => {
    if (!open) return;
    const generation = ++epoch.current;
    setBusy(true); pending.current = true; setError(""); setPreview(null); setConfirm(null); setEditId(null); setNotice("");
    void invoke<CodeyPluginsResult>("list_codey_plugins").then(data => { if (epoch.current === generation) setResult(data); })
      .catch(cause => { if (epoch.current === generation) setError(errorText(cause)); })
      .finally(() => { if (epoch.current === generation) { setBusy(false); pending.current = false; } });
    return () => { epoch.current++; pending.current = false; };
  }, [open]);
  async function run(action: () => Promise<void>) {
    if (pending.current) return;
    pending.current = true; setBusy(true); setError(""); setNotice("");
    try { await action(); } catch (cause) { setError(errorText(cause)); }
    finally { pending.current = false; setBusy(false); }
  }
  async function toggle(plugin: CodeyPlugin, enabled: boolean) {
    setResult(await invoke<CodeyPluginsResult>("set_codey_plugin_enabled", { pluginId: plugin.id, enabled }));
    setConfirm(null); setNotice(enabled ? "插件已启用" : "插件已停用");
  }
  const upgrading = preview && result?.plugins.find(plugin => plugin.id === preview.manifest.id);
  return <Dialog open={open} onOpenChange={next => { if (!next && !pending.current) onClose(); }}>
    <DialogContent container={container} className="w-full sm:w-[760px] max-w-[calc(100vw-32px)]" onEscapeKeyDown={event => { if (busy) event.preventDefault(); }}>
      <DialogHeader><DialogTitle>Codey 插件</DialogTitle><DialogDescription>安装独立功能模块，并管理启用状态与配置。</DialogDescription></DialogHeader>
      <div className="mt-4 grid max-h-[72vh] gap-4 overflow-y-auto pr-1" aria-busy={busy}>
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span className="text-xs text-muted">{result ? `${result.plugins.length} 个插件 · ${result.platform} / ${result.arch}` : "正在读取插件…"}</span>
          <div className="flex gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => void run(async () => { setResult(await invoke("list_codey_plugins")); })}>刷新</Button>
            <Button size="sm" disabled={busy} onClick={() => void run(async () => { const selected = await invoke<CodeyPluginPreview | null>("select_codey_plugin_package"); if (selected) { setPreview(selected); setConfirm(null); } })}>导入插件</Button></div>
        </div>
        <details className="text-xs text-muted"><summary className="cursor-pointer">使用本地文件路径导入</summary><div className="mt-2 flex gap-2"><Input aria-label="插件包路径" placeholder=".codey-plugin 文件的完整路径" value={path} disabled={busy} onChange={event => { setPath(event.target.value); setPreview(null); }} /><Button variant="outline" size="sm" disabled={busy || !path.trim()} onClick={() => void run(async () => { setPreview(await invoke("inspect_codey_plugin", { path: path.trim() })); setConfirm(null); })}>检查</Button></div></details>
        {error && <p role="alert" className="m-0 break-words rounded-lg bg-red-50 p-3 text-xs text-red-700 dark:bg-red-950 dark:text-red-300">{error}</p>}
        {notice && <p role="status" className="m-0 text-xs text-green-700 dark:text-green-400">{notice}</p>}
        {busy && <p role="status" className="m-0 text-xs text-muted">正在处理，请稍候…</p>}
        {preview && <section className={`${panel} grid gap-3 border-blue-300`}>
          <div><h3 className="m-0 font-semibold">{upgrading ? "升级预览" : "安装预览"} · {preview.manifest.name}</h3><p className="mb-0 text-xs text-muted break-all">{preview.manifest.id} · {upgrading ? `${upgrading.version} → ` : ""}{preview.manifest.version}</p></div>
          {preview.manifest.description && <p className="m-0 text-sm">{preview.manifest.description}</p>}
          <div className="text-xs break-words">声明能力：{[...(preview.manifest.capabilities ?? []), ...(preview.manifest.permissions ?? [])].join("、") || "无"}
            {(preview.manifest.headerNames?.length ?? 0) > 0 && <p className="mb-0">可修改的请求头：{preview.manifest.headerNames!.join("、")}</p>}</div>
          <p className="m-0 text-xs text-amber-700 dark:text-amber-300">原生插件与 Codey 在同一进程运行，具有相同的系统权限。仅安装你信任的来源；权限声明不提供沙箱隔离。</p>
          <p className="m-0 text-xs text-muted">{upgrading ? "正在运行的插件升级后，停用并重新启用或重启 Codey 即可生效。" : "安装后默认停用，可稍后配置并启用。"}</p>
          <div className="flex gap-2"><Button size="sm" disabled={busy} onClick={() => void run(async () => { setResult(await invoke("install_codey_plugin", { path: preview.path, sha256: preview.sha256 })); setPreview(null); setNotice("安装完成，请查看插件状态"); })}>{upgrading ? "确认升级" : "确认安装"}</Button><Button variant="outline" size="sm" disabled={busy} onClick={() => setPreview(null)}>取消</Button></div>
        </section>}
        {confirm && <section className={`${panel} grid gap-3`}>
          <h3 className="m-0 text-sm font-semibold">{confirm.kind === "enable" ? "启用" : "卸载"} {confirm.plugin.name}</h3>
          {confirm.kind === "enable" ? <p className="m-0 text-xs text-amber-700 dark:text-amber-300">启用后将执行插件代码，插件具有与 Codey 相同的系统权限。请确认你信任此插件。</p>
            : <Checkbox disabled={busy} checked={removeData} onCheckedChange={next => setRemoveData(next === true)} label="同时删除插件配置、数据和日志（默认保留）" />}
          <div className="flex gap-2"><Button size="sm" variant={confirm.kind === "uninstall" ? "destructive" : "default"} disabled={busy} onClick={() => void run(async () => {
            if (confirm.kind === "enable") await toggle(confirm.plugin, true);
            else { setResult(await invoke("uninstall_codey_plugin", { pluginId: confirm.plugin.id, removeData })); setConfirm(null); setEditId(null); setNotice("插件已卸载"); }
          })}>{confirm.kind === "enable" ? "信任并启用" : "确认卸载"}</Button><Button variant="outline" size="sm" disabled={busy} onClick={() => setConfirm(null)}>取消</Button></div>
        </section>}
        {result?.plugins.length === 0 && <div className={`${panel} py-10 text-center`}><p className="m-0 font-medium">尚未安装插件</p><p className="mb-0 text-xs text-muted">导入 .codey-plugin 文件，添加你需要的功能。</p></div>}
        {result?.plugins.map(plugin => <div key={plugin.id} className="grid gap-3"><section className={`${panel} grid gap-3`}>
          <div className="flex flex-wrap justify-between gap-2"><div><h3 className="m-0 text-sm font-semibold">{plugin.name} <span className="font-normal text-muted">{plugin.version}</span></h3><p className="mb-0 text-xs text-muted break-all">{plugin.id}</p></div><Badge variant={plugin.restartRequired ? "warning" : plugin.lastError ? "destructive" : plugin.enabled ? "success" : "secondary"}>{pluginStatusLabel(plugin)}</Badge></div>
          {plugin.description && <p className="m-0 text-xs text-muted">{plugin.description}</p>}
          {plugin.pluginDir && <details className="text-xs text-muted"><summary className="cursor-pointer">插件文件位置</summary><dl className="mt-2 grid gap-1 break-all">
            <dt>插件目录</dt><dd className="ml-0 select-text">{plugin.pluginDir}</dd>
            {plugin.dataDir && <><dt>数据目录</dt><dd className="ml-0 select-text">{plugin.dataDir}</dd></>}
            {plugin.logDir && <><dt>日志目录</dt><dd className="ml-0 select-text">{plugin.logDir}</dd></>}
          </dl><p className="mb-0">升级保留数据和日志；卸载时可选择是否一并删除。</p></details>}
          {plugin.activeVersion && plugin.activeVersion !== plugin.version && <p className="m-0 text-xs text-muted">当前运行版本：{plugin.activeVersion}</p>}
          {plugin.restartRequired && <p role="status" className="m-0 text-xs text-amber-700 dark:text-amber-300">重启 Codey 或停用后重新启用以应用变更；当前实例仍使用先前的版本或配置。</p>}
          {plugin.lastError && <p role="alert" className="m-0 break-words text-xs text-red-600 dark:text-red-400">{plugin.lastError}</p>}
          <div className="flex flex-wrap gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => {
            if (plugin.enabled) void run(() => toggle(plugin, false)); else { setConfirm({ kind: "enable", plugin }); setPreview(null); }
          }}>{plugin.enabled ? "停用" : "启用"}</Button><Button size="sm" variant="outline" disabled={busy} onClick={() => setEditId(editId === plugin.id ? null : plugin.id)}>{editId === plugin.id ? "收起配置" : "配置"}</Button><Button size="sm" variant="destructive-light" disabled={busy || plugin.enabled} title={plugin.enabled ? "请先停用插件再卸载" : undefined} onClick={() => { setRemoveData(false); setConfirm({ kind: "uninstall", plugin }); setPreview(null); }}>卸载</Button></div>
        </section>{editId === plugin.id && <ConfigEditor key={`${plugin.id}:${JSON.stringify(plugin.config)}`} plugin={plugin} busy={busy} onSave={config => void run(async () => { setResult(await invoke("configure_codey_plugin", { pluginId: plugin.id, config })); setNotice("配置已保存，请查看插件状态确认是否需要重新启用"); })} />}</div>)}
      </div>
    </DialogContent>
  </Dialog>;
}
