import { useEffect, useRef, useState } from "react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import {
  Button,
  Drawer,
  DrawerBody,
  DrawerContent,
  DrawerDescription,
  DrawerFooter,
  DrawerHeader,
  DrawerTitle,
} from "./components/ui";
import { parseCodeyPluginsResult, type CodeyPlugin, type CodeyPluginConfigFile, type CodeyPluginsResult } from "./codeyPlugins";
import {
  isEditableConfigArray,
  parsePluginConfigDocument,
  serializePluginConfigDocument,
  validatePluginConfigValue,
  type PluginConfigDocument,
  type PluginConfigEntry,
} from "./pluginConfigDocument";

type Props = { plugin: CodeyPlugin; onClose: () => void; onChanged: (result: CodeyPluginsResult) => void; container?: HTMLElement | null };

const valueToken = (entry: PluginConfigEntry) => entry.kind === "string" ? JSON.stringify(entry.valueText)
  : isEditableConfigArray(entry) ? entry.valueText.replace(/"(?:\\.|[^"\\])*"|[\t\n\r ]+/g, token => token.startsWith('"') ? token : "") : entry.valueText;
function decodedValue(entry: PluginConfigEntry, token: string): string {
  if (entry.kind !== "string") return token;
  let value: unknown;
  try { value = JSON.parse(token); } catch { throw new Error("请输入完整的 JSON 字符串，包含双引号；换行请写为 \\n。"); }
  if (typeof value !== "string") throw new Error("此配置项必须是带双引号的 JSON 字符串。");
  return value;
}
function tokenError(entry: PluginConfigEntry, token: string): string | undefined {
  try { return validatePluginConfigValue(entry, decodedValue(entry, token)); }
  catch (cause) { return errorText(cause); }
}

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
    if (value === valueToken(entry)) next.delete(entry.id); else next.set(entry.id, value);
    draft.current = next; setEdits(next);
  }
  async function save() {
    if (pending.current || loading || !file || !document || !draft.current.size) return;
    let content: string;
    try {
      const values = new Map<string, string>();
      const collect = (entries: PluginConfigEntry[]) => entries.forEach(entry => {
        if (draft.current.has(entry.id)) {
          const token = draft.current.get(entry.id)!;
          const validation = tokenError(entry, token);
          if (validation) throw new Error(`${JSON.stringify(entry.path)}：${validation}`);
          values.set(entry.id, decodedValue(entry, token));
        } else collect(entry.children);
      });
      collect(document.entries);
      content = serializePluginConfigDocument(document, values);
    }
    catch (cause) { setError(errorText(cause)); return; }
    pending.current = true; setSaving(true); setError(""); setDiscard(null);
    try {
      const result = parseCodeyPluginsResult(await invoke("save_codey_plugin_config_file", { pluginId: plugin.id, content, expectedSha256: file.sha256 }));
      if (alive.current) { onChanged(result); onClose(); }
    } catch (cause) { if (alive.current) setError(errorText(cause)); }
    finally { pending.current = false; if (alive.current) setSaving(false); }
  }
  const invalid = (entries: PluginConfigEntry[]): boolean => entries.some(entry => edits.has(entry.id) ? !!tokenError(entry, edits.get(entry.id)!) : invalid(entry.children));

  function renderEntry(entry: PluginConfigEntry, last: boolean) {
    const value = edits.get(entry.id) ?? valueToken(entry);
    const validation = edits.has(entry.id) ? tokenError(entry, value) : undefined;
    const fieldId = `plugin-config-value-${encodeURIComponent(entry.id)}`;
    const commentId = `${fieldId}-comment`, errorId = `${fieldId}-error`;
    const composite = entry.kind === "object" || (entry.kind === "array" && !isEditableConfigArray(entry));
    const key = typeof entry.path[entry.path.length - 1] === "string" ? `${JSON.stringify(entry.key)}: ` : "";
    const open = entry.kind === "array" ? "[" : "{";
    const close = entry.kind === "array" ? "]" : "}";
    return <div key={entry.id} className="min-w-0">
      {entry.comment && <p id={commentId} className="mb-0 mt-2 whitespace-pre-wrap break-words font-sans text-[11px] font-normal leading-4 text-zinc-500 dark:text-zinc-400" style={{ userSelect: "none", WebkitUserSelect: "none" }}>{entry.comment}</p>}
      <div className="flex min-w-0 items-baseline leading-7">
        {key && <label htmlFor={composite ? undefined : fieldId} className="shrink-0 whitespace-pre">{key}</label>}
        {composite ? <span>{open}{!entry.children.length && `${close}${last ? "" : ","}`}</span> : <>
          <input id={fieldId} aria-label={entry.path.map(String).join(".")} aria-describedby={[entry.comment ? commentId : "", validation ? errorId : ""].filter(Boolean).join(" ") || undefined} aria-invalid={!!validation}
            value={value} disabled={saving} type="text" spellCheck={false} autoCapitalize="off" autoCorrect="off"
            className="min-w-[3ch] max-w-full rounded-sm border-0 bg-muted/30 px-1 py-0 font-mono text-sm leading-7 text-foreground outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-60"
            style={{ width: `${Math.max(3, value.length + 1)}ch` }} onChange={event => edit(entry, event.target.value)} />
          {!last && <span>,</span>}
        </>}
      </div>
      {composite && !!entry.children.length && <><div className="min-w-0 pl-4 sm:pl-6">{entry.children.map((child, index) => renderEntry(child, index === entry.children.length - 1))}</div><div className="leading-7">{close}{!last && ","}</div></>}
      {validation && <p id={errorId} role="alert" className="m-0 break-words font-sans text-xs text-red-600 dark:text-red-400">{validation}</p>}
    </div>;
  }

  return (
    <Drawer open onOpenChange={open => { if (!open) request("close"); }}>
      <DrawerContent
        container={container}
        placement="right"
        className="w-[75%] min-w-[380px] max-w-full h-full border-l border-border"
        onEscapeKeyDown={event => { if (pending.current) event.preventDefault(); }}
      >
        <div className="contents" onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") { event.preventDefault(); void save(); } }}>
          <DrawerHeader>
            <DrawerTitle>{plugin.name} · 配置</DrawerTitle>
            <DrawerDescription>
              按 JSON 格式修改值：字符串带双引号，数字和 true / false 不加引号。数值、文本等值数组可直接增删项；含对象的数组逐项修改字段值。字段名和说明只读。保存后重新启用插件生效。
            </DrawerDescription>
          </DrawerHeader>

          {error && <p role="alert" className="mx-6 mt-3 mb-0 shrink-0 break-words text-xs text-red-600 dark:text-red-400">{error}</p>}

          {discard && (
            <section role="alert" className="mx-6 mt-3 grid shrink-0 gap-2 rounded-lg border border-amber-300 p-3 text-xs">
              <p className="m-0">{discard === "close" ? "配置尚未保存，放弃修改？" : "重新加载将丢弃尚未保存的修改，是否继续？"}</p>
              <div className="flex gap-2">
                <Button size="xs" variant="destructive" disabled={saving} onClick={() => { if (pending.current) return; if (discard === "close") onClose(); else void load(); }}>放弃修改</Button>
                <Button size="xs" variant="outline" disabled={saving} onClick={() => setDiscard(null)}>继续编辑</Button>
              </div>
            </section>
          )}

          <DrawerBody aria-busy={loading || saving} className="mt-2 space-y-3">
            <div className="flex items-baseline gap-2 text-xs">
              <span className="font-medium text-foreground shrink-0">config.json</span>
              <p className="m-0 select-text truncate font-mono text-[11px] text-muted-foreground" title={file?.path ?? plugin.configPath}>
                {file?.path ?? plugin.configPath}
              </p>
            </div>

            {loading ? (
              <p role="status" className="m-0 text-sm text-muted-foreground">正在读取配置文件…</p>
            ) : document && (
              <div
                aria-label="JSON 配置编辑器"
                className="overflow-x-auto rounded-md border border-input bg-background p-3 font-mono text-sm"
              >
                <div className="leading-7">{"{"}</div>
                <div className="min-w-0 pl-4 sm:pl-6">
                  {document.entries.map((entry, index) => renderEntry(entry, index === document.entries.length - 1))}
                </div>
                <div className="leading-7">{"}"}</div>
              </div>
            )}
          </DrawerBody>

          <DrawerFooter className="gap-2">
            <Button size="sm" variant="outline" disabled={saving || loading} onClick={() => request("reload")}>
              {file ? "重新加载" : "重试读取"}
            </Button>
            <Button
              size="sm"
              disabled={saving || loading || !document || !edits.size || invalid(document.entries)}
              onClick={() => void save()}
            >
              {saving ? "保存中…" : "保存配置"}
            </Button>
          </DrawerFooter>
        </div>
      </DrawerContent>
    </Drawer>
  );
}
