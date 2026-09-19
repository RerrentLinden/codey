import { useEffect, useRef, useState } from "react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import { Button, Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./components/ui";
import { SchemaConfigForm } from "./SchemaConfigForm";
import { parseCodeyPluginsResult, samePluginValue, type CodeyPlugin, type CodeyPluginsResult } from "./codeyPlugins";
import { PluginHtmlConfig } from "./PluginHtmlConfig";

type PluginConfigDialogProps = {
  plugin: CodeyPlugin; onClose: () => void; onChanged: (result: CodeyPluginsResult) => void; container?: HTMLElement | null;
};

export function PluginConfigDialog(props: PluginConfigDialogProps) {
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const { id, version, config, configSchema, configUi } = props.plugin;
  // 仅配置内容变化时重建表单；对象键顺序和运行状态刷新不应丢弃草稿。
  const editorKey = JSON.stringify([id, version, config, configSchema, configUi ? [configUi.type, configUi.entry, configUi.sha256] : null],
    (_key, value) => value && typeof value === "object" && !Array.isArray(value)
      ? Object.fromEntries(Object.keys(value).sort().map(key => [key, value[key]])) : value);
  const activeEditor = useRef(editorKey);
  activeEditor.current = editorKey;
  async function save(config: Record<string, unknown>): Promise<string | undefined> {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    try {
      const result = parseCodeyPluginsResult(await invoke<CodeyPluginsResult>("configure_codey_plugin", { pluginId: id, config }));
      if (activeEditor.current === editorKey) { props.onChanged(result); props.onClose(); }
    } catch (cause) { if (activeEditor.current === editorKey) return errorText(cause); }
    finally { pending.current = false; setBusy(false); }
  }
  return <PluginConfigEditor key={editorKey} {...props} busy={busy} pending={pending} onSave={save} />;
}

function PluginConfigEditor({ plugin, onClose, container, busy, pending, onSave }: PluginConfigDialogProps & {
  busy: boolean; pending: { current: boolean }; onSave: (config: Record<string, unknown>) => Promise<string | undefined>;
}) {
  const [error, setError] = useState("");
  const draft = useRef(plugin.config);
  const dirty = useRef(false);
  const [discard, setDiscard] = useState(false);
  function updateDraft(config: Record<string, unknown>, invalidDraft = false, changed?: boolean) {
    draft.current = config;
    dirty.current = invalidDraft || (changed ?? !samePluginValue(config, plugin.config));
  }
  function requestClose() {
    if (pending.current) return;
    if (dirty.current) setDiscard(true);
    else onClose();
  }
  const [html, setHtml] = useState<string | null>(null);
  const [uiError, setUiError] = useState("");
  const [fallback, setFallback] = useState(false);
  useEffect(() => {
    if (!plugin.configUi) return;
    let cancelled = false;
    void invoke<{ pluginId: string; version: string; html: string } | null>("get_codey_plugin_config_ui", { pluginId: plugin.id }).then(result => {
      if (cancelled) return;
      if (!result || result.pluginId !== plugin.id || result.version !== plugin.version || typeof result.html !== "string") throw new Error("插件配置页面不可用或版本已变化，请重新打开配置。");
      setHtml(result.html);
    }).catch(cause => { if (!cancelled) setUiError(errorText(cause)); });
    return () => { cancelled = true; };
  }, [plugin.id, plugin.version, plugin.configUi?.type, plugin.configUi?.entry, plugin.configUi?.sha256]);
  async function save(config: Record<string, unknown>) {
    setError("");
    const message = await onSave(config);
    if (message) setError(message);
  }
  return <Dialog open onOpenChange={open => { if (!open) requestClose(); }}>
    <DialogContent container={container} className="w-full sm:w-[640px] max-w-[calc(100vw-32px)]" onEscapeKeyDown={event => { if (pending.current) event.preventDefault(); }}>
      <DialogHeader><DialogTitle>{plugin.name} · 配置</DialogTitle><DialogDescription>保存后，正在运行的实例需要停用并重新启用，或重启 Codey 后生效。</DialogDescription></DialogHeader>
      <div className="mt-4 grid max-h-[68vh] gap-3 overflow-y-auto pr-1" aria-busy={busy}>
        {error && <p role="alert" className="m-0 break-words text-sm text-red-600 dark:text-red-400">{error}</p>}
        {discard && <section role="alert" className="grid gap-2 rounded-lg border border-amber-300 p-3 text-sm"><p className="m-0">配置尚未保存，放弃修改并返回插件管理？</p><div className="flex gap-2"><Button size="sm" variant="destructive" disabled={busy} onClick={() => { if (!pending.current) onClose(); }}>放弃修改</Button><Button size="sm" variant="outline" disabled={busy} onClick={() => setDiscard(false)}>继续编辑</Button></div></section>}
        {plugin.configUi && !fallback ? <>
          {uiError ? <div className="grid gap-3"><p role="alert" className="m-0 break-words text-sm text-red-600">{uiError}</p><Button variant="outline" onClick={() => setFallback(true)}>改用自动表单</Button></div>
            : html !== null ? <PluginHtmlConfig html={html} value={plugin.config} schema={plugin.configSchema ?? {}} disabled={busy} onSave={config => void save(config)} onFailure={setUiError} onDraftChange={updateDraft} />
              : <p className="m-0 text-sm text-muted-foreground">正在加载插件配置页面…</p>}
        </> : <SchemaConfigForm schema={plugin.configSchema ?? {}} value={draft.current} disabled={busy} onSave={config => void save(config)} onDraftChange={(config, invalid, changed) => updateDraft(config, invalid, fallback ? undefined : changed)} />}
        <div><Button size="sm" variant="outline" disabled={busy} onClick={requestClose}>返回插件管理</Button></div>
      </div>
    </DialogContent>
  </Dialog>;
}
