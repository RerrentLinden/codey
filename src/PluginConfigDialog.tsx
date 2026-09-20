import { useEffect, useRef, useState } from "react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import { Button, Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./components/ui";
import { parseCodeyPluginsResult, validatePluginConfigText, type CodeyPlugin, type CodeyPluginConfigFile, type CodeyPluginsResult } from "./codeyPlugins";

type Props = { plugin: CodeyPlugin; onClose: () => void; onChanged: (result: CodeyPluginsResult) => void; container?: HTMLElement | null };

export function PluginConfigDialog(props: Props) {
  return <ConfigFileEditor key={JSON.stringify([props.plugin.id, props.plugin.version])} {...props} />;
}

function ConfigFileEditor({ plugin, onClose, onChanged, container }: Props) {
  const [file, setFile] = useState<CodeyPluginConfigFile | null>(null);
  const [content, setContent] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [discard, setDiscard] = useState<"close" | "reload" | null>(null);
  const pending = useRef(false);
  const alive = useRef(false);
  const epoch = useRef(0);
  const draft = useRef("");
  const original = useRef("");

  async function load() {
    if (pending.current) return;
    const generation = ++epoch.current;
    setLoading(true); setError(""); setDiscard(null); setFile(null);
    draft.current = ""; original.current = ""; setContent("");
    try {
      const next = await invoke<CodeyPluginConfigFile>("get_codey_plugin_config_file", { pluginId: plugin.id });
      if (!alive.current || generation !== epoch.current) return;
      if (!next || next.pluginId !== plugin.id || next.version !== plugin.version || typeof next.path !== "string" || typeof next.content !== "string" || !/^[a-f0-9]{64}$/i.test(next.sha256)) throw new Error("配置文件响应无效或插件版本已变化，请重新打开配置。");
      draft.current = next.content; original.current = next.content;
      setContent(next.content); setFile(next);
    } catch (cause) { if (alive.current && generation === epoch.current) setError(errorText(cause)); }
    finally { if (alive.current && generation === epoch.current) setLoading(false); }
  }
  useEffect(() => {
    alive.current = true; void load();
    return () => { alive.current = false; epoch.current++; };
  }, []);

  function request(action: "close" | "reload") {
    if (pending.current) return;
    if (draft.current !== original.current) setDiscard(action);
    else if (action === "close") onClose();
    else void load();
  }
  async function save() {
    if (pending.current || loading || !file) return;
    const validation = validatePluginConfigText(draft.current);
    if (validation) { setError(validation); return; }
    pending.current = true; setSaving(true); setError(""); setDiscard(null);
    try {
      const result = parseCodeyPluginsResult(await invoke("save_codey_plugin_config_file", { pluginId: plugin.id, content: draft.current, expectedSha256: file.sha256 }));
      if (alive.current) { onChanged(result); onClose(); }
    } catch (cause) { if (alive.current) setError(errorText(cause)); }
    finally { pending.current = false; if (alive.current) setSaving(false); }
  }
  return <Dialog open onOpenChange={open => { if (!open) request("close"); }}>
    <DialogContent container={container} className="w-full sm:w-[720px] max-w-[calc(100vw-32px)]" onEscapeKeyDown={event => { if (pending.current) event.preventDefault(); }}>
      <DialogHeader><DialogTitle>{plugin.name} · 配置</DialogTitle><DialogDescription>直接编辑 config.json。可用 _comments 对象中的字符串说明字段含义；只改说明无需重新启用，修改业务参数后需重新启用插件。</DialogDescription></DialogHeader>
      <div className="mt-4 grid max-h-[70vh] gap-3 overflow-y-auto pr-1" aria-busy={loading || saving}>
        <div className="grid gap-1"><label htmlFor="plugin-config-content" className="text-sm font-medium">config.json</label><p className="m-0 break-all font-mono text-xs text-muted-foreground select-text">{file?.path ?? plugin.configPath}</p></div>
        {error && <p role="alert" className="m-0 break-words text-sm text-red-600 dark:text-red-400">{error}</p>}
        {discard && <section role="alert" className="grid gap-2 rounded-lg border border-amber-300 p-3 text-sm"><p className="m-0">{discard === "close" ? "配置尚未保存，放弃修改并返回插件管理？" : "重新加载将丢弃尚未保存的修改，是否继续？"}</p><div className="flex gap-2"><Button size="sm" variant="destructive" disabled={saving} onClick={() => { if (pending.current) return; if (discard === "close") onClose(); else void load(); }}>放弃修改</Button><Button size="sm" variant="outline" disabled={saving} onClick={() => setDiscard(null)}>继续编辑</Button></div></section>}
        {loading ? <p role="status" className="m-0 text-sm text-muted-foreground">正在读取配置文件…</p> : file && <textarea id="plugin-config-content" aria-label="config.json 内容" spellCheck={false} autoCapitalize="off" autoCorrect="off" value={content} disabled={saving} onChange={event => { draft.current = event.target.value; setContent(event.target.value); }} onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key === "s") { event.preventDefault(); void save(); } }} className="min-h-[240px] h-[42vh] w-full resize-y rounded-lg border border-input bg-background p-3 font-mono text-sm leading-6 text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60" />}
        <div className="flex flex-wrap items-center justify-between gap-2"><Button size="sm" variant="outline" disabled={saving} onClick={() => request("close")}>返回插件管理</Button><div className="flex gap-2"><Button size="sm" variant="outline" disabled={saving || loading} onClick={() => request("reload")}>{file ? "重新加载" : "重试读取"}</Button><Button size="sm" disabled={saving || loading || !file || content === original.current} onClick={() => void save()}>{saving ? "保存中…" : "保存配置"}</Button></div></div>
      </div>
    </DialogContent>
  </Dialog>;
}
