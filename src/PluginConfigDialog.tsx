import { useEffect, useRef, useState } from "react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import { Button, Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./components/ui";
import { parseCodeyPluginsResult, type CodeyPlugin, type CodeyPluginConfigFile, type CodeyPluginsResult } from "./codeyPlugins";
import { parsePluginConfigDocument, serializePluginConfigDocument, validatePluginConfigValue, type PluginConfigDocument, type PluginConfigEntry } from "./pluginConfigDocument";

type Props = { plugin: CodeyPlugin; onClose: () => void; onChanged: (result: CodeyPluginsResult) => void; container?: HTMLElement | null };

export function PluginConfigDialog(props: Props) {
  return <ConfigFileEditor key={JSON.stringify([props.plugin.id, props.plugin.version])} {...props} />;
}

function ConfigFileEditor({ plugin, onClose, onChanged, container }: Props) {
  const [file, setFile] = useState<CodeyPluginConfigFile | null>(null);
  const [document, setDocument] = useState<PluginConfigDocument | null>(null);
  const [edits, setEdits] = useState<ReadonlyMap<string, string>>(new Map());
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const pending = useRef(false);
  const alive = useRef(false);
  const epoch = useRef(0);
  const draft = useRef<ReadonlyMap<string, string>>(new Map());

  async function load() {
    if (pending.current) return;
    const generation = ++epoch.current;
    setLoading(true); setError(""); setDiscard(null); setFile(null); setDocument(null);
    draft.current = new Map(); setEdits(draft.current);
    try {
      const next = await invoke<CodeyPluginConfigFile>("get_codey_plugin_config_file", { pluginId: plugin.id });
      if (!alive.current || generation !== epoch.current) return;
      if (!next || next.pluginId !== plugin.id || next.version !== plugin.version || typeof next.path !== "string" || typeof next.content !== "string" || !/^[a-f0-9]{64}$/i.test(next.sha256)) throw new Error("配置文件响应无效或插件版本已变化，请重新打开配置。");
      setFile(next);
      try { setDocument(parsePluginConfigDocument(next.content)); }
      catch (cause) { setError(`${errorText(cause)} 请修正配置文件后重新加载。`); }
    } catch (cause) { if (alive.current && generation === epoch.current) setError(errorText(cause)); }
    finally { if (alive.current && generation === epoch.current) setLoading(false); }
  }
  useEffect(() => {
    alive.current = true; void load();
    return () => { alive.current = false; epoch.current++; };
  }, []);

  function request(action: "close" | "reload") {
    if (pending.current) return;
    if (draft.current.size) setDiscard(action);
    else if (action === "close") onClose();
    else void load();
  }
  function edit(entry: PluginConfigEntry, value: string) {
    if (pending.current) return;
    const next = new Map(draft.current);
    if (value === entry.valueText) next.delete(entry.id); else next.set(entry.id, value);
    draft.current = next; setEdits(next);
  }
  async function save() {
    if (pending.current || loading || !file || !document || !draft.current.size) return;
    let content: string;
    try { content = serializePluginConfigDocument(document, draft.current); }
    catch (cause) { setError(errorText(cause)); return; }
    pending.current = true; setSaving(true); setError(""); setDiscard(null);
    try {
      const result = parseCodeyPluginsResult(await invoke("save_codey_plugin_config_file", { pluginId: plugin.id, content, expectedSha256: file.sha256 }));
      if (alive.current) { onChanged(result); onClose(); }
    } catch (cause) { if (alive.current) setError(errorText(cause)); }
    finally { pending.current = false; if (alive.current) setSaving(false); }
  }
  const invalid = (entries: PluginConfigEntry[]): boolean => entries.some(entry => entry.children.length ? invalid(entry.children) : edits.has(entry.id) && !!validatePluginConfigValue(entry, edits.get(entry.id)!));
  const inputClass = "min-w-0 w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-sm text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60";
  function renderEntry(entry: PluginConfigEntry) {
    const value = edits.get(entry.id) ?? entry.valueText;
    const validation = edits.has(entry.id) ? validatePluginConfigValue(entry, value) : undefined;
    const fieldId = `plugin-config-value-${encodeURIComponent(entry.id)}`;
    const commentId = `${fieldId}-comment`, errorId = `${fieldId}-error`;
    const composite = entry.kind === "object" || entry.kind === "array";
    const common = { id: fieldId, "aria-label": entry.path.map(String).join("."), "aria-describedby": [entry.comment ? commentId : "", validation ? errorId : ""].filter(Boolean).join(" ") || undefined, "aria-invalid": !!validation, value, disabled: saving, className: inputClass };
    return <div key={entry.id} className="min-w-0 space-y-2">
      {entry.comment && <p id={commentId} className="m-0 whitespace-pre-wrap break-words text-sm leading-6 text-muted-foreground" style={{ userSelect: "none", WebkitUserSelect: "none" }}>{entry.comment}</p>}
      {composite ? <>
        <div className="flex items-baseline gap-2"><code className="break-all text-sm font-medium">{entry.key}</code><span className="text-xs text-muted-foreground">{entry.kind === "array" ? "数组" : "对象"}</span></div>
        <div className="min-w-0 space-y-5 border-l border-border pl-3 sm:pl-4">{entry.children.length ? entry.children.map(renderEntry) : <p className="m-0 text-sm text-muted-foreground">无可编辑值</p>}</div>
      </> : <>
        <label htmlFor={fieldId} className="block break-all font-mono text-sm font-medium">{entry.key}</label>
        {entry.kind === "boolean" ? <select {...common} onChange={event => edit(entry, event.target.value)}><option value="true">true · 开启</option><option value="false">false · 关闭</option></select>
          : entry.kind === "string" && entry.valueText.includes("\n") ? <textarea {...common} rows={Math.min(8, Math.max(3, value.split("\n").length))} spellCheck={false} onChange={event => edit(entry, event.target.value)} />
          : <input {...common} type="text" inputMode={entry.kind === "number" ? "decimal" : "text"} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={event => edit(entry, event.target.value)} />}
        {validation && <p id={errorId} role="alert" className="m-0 break-words text-sm text-red-600 dark:text-red-400">{validation}</p>}
      </>}
    </div>;
  }
  return <Dialog open onOpenChange={open => { if (!open) request("close"); }}>
    <DialogContent container={container} className="flex max-h-[calc(100dvh-32px)] w-full max-w-[calc(100vw-32px)] flex-col sm:w-[720px]" onEscapeKeyDown={event => { if (pending.current) event.preventDefault(); }}>
      <div className="contents" onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") { event.preventDefault(); void save(); } }}>
      <DialogHeader className="shrink-0"><DialogTitle>{plugin.name} · 配置</DialogTitle><DialogDescription>仅可修改配置项的值，字段名和说明只读。保存后重新启用插件生效。</DialogDescription></DialogHeader>
      {error && <p role="alert" className="mt-3 mb-0 shrink-0 break-words text-sm text-red-600 dark:text-red-400">{error}</p>}
      {discard && <section role="alert" className="mt-3 grid shrink-0 gap-2 rounded-lg border border-amber-300 p-3 text-sm"><p className="m-0">{discard === "close" ? "配置尚未保存，放弃修改并返回插件管理？" : "重新加载将丢弃尚未保存的修改，是否继续？"}</p><div className="flex gap-2"><Button size="sm" variant="destructive" disabled={saving} onClick={() => { if (pending.current) return; if (discard === "close") onClose(); else void load(); }}>放弃修改</Button><Button size="sm" variant="outline" disabled={saving} onClick={() => setDiscard(null)}>继续编辑</Button></div></section>}
      <div className="mt-3 min-h-0 space-y-5 overflow-y-auto pr-1" aria-busy={loading || saving}>
        <div className="space-y-1"><p className="m-0 text-sm font-medium">config.json</p><p className="m-0 select-text break-all font-mono text-xs text-muted-foreground">{file?.path ?? plugin.configPath}</p></div>
        {loading ? <p role="status" className="m-0 text-sm text-muted-foreground">正在读取配置文件…</p> : document && <div className="space-y-6 py-1">{document.entries.length ? document.entries.map(renderEntry) : <p className="m-0 text-sm text-muted-foreground">无可编辑值</p>}</div>}
      </div>
      <div className="mt-4 flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-border pt-4"><Button size="sm" variant="outline" disabled={saving} onClick={() => request("close")}>返回插件管理</Button><div className="flex gap-2"><Button size="sm" variant="outline" disabled={saving || loading} onClick={() => request("reload")}>{file ? "重新加载" : "重试读取"}</Button><Button size="sm" disabled={saving || loading || !document || !edits.size || invalid(document.entries)} onClick={() => void save()}>{saving ? "保存中…" : "保存配置"}</Button></div></div>
      </div>
    </DialogContent>
  </Dialog>;
}
