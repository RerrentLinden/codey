import { useEffect, useRef, useState } from "react";
import {
  IconAdjustments,
  IconAlertCircle,
  IconAlertTriangle,
  IconCheck,
  IconChevronDown,
  IconChevronUp,
  IconCopy,
  IconFileCode,
  IconInfoCircle,
  IconX,
} from "@tabler/icons-react";
import { invoke } from "./api";
import { errorText } from "./appUtils";
import {
  Badge,
  Button,
  Drawer,
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
  const [copied, setCopied] = useState(false);
  const [rulesOpen, setRulesOpen] = useState(false);
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

  async function copyPath() {
    const target = file?.path ?? plugin.configPath;
    if (!target) return;
    try {
      await navigator.clipboard.writeText(target);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // 忽略剪贴板写入失败
    }
  }

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
      <DrawerContent container={container} placement="right" className="w-full sm:w-[700px] max-w-[100vw]" onEscapeKeyDown={event => { if (pending.current) event.preventDefault(); }}>
        <div className="contents" onKeyDown={event => { if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") { event.preventDefault(); void save(); } }}>
          <DrawerHeader className="px-5 py-3.5 bg-muted/30 border-b border-border/60">
            <div className="flex items-center gap-2.5 min-w-0">
              <div className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
                <IconAdjustments size={18} aria-hidden="true" />
              </div>
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <DrawerTitle className="truncate text-sm font-semibold">{plugin.name} · 配置</DrawerTitle>
                  <Badge variant="outline" className="shrink-0 text-[11px] font-mono px-1.5 py-0 h-5">v{plugin.version}</Badge>
                </div>
                <DrawerDescription className="text-[11px] text-muted-foreground truncate">
                  管理插件运行参数与规则配置
                </DrawerDescription>
              </div>
            </div>
            <button
              type="button"
              onClick={() => request("close")}
              disabled={saving}
              aria-label="关闭"
              className="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
            >
              <IconX size={16} aria-hidden="true" />
            </button>
          </DrawerHeader>

          {error && (
            <div className="mx-5 mt-4 flex items-start gap-2.5 rounded-xl border border-red-500/20 bg-red-500/10 p-3 text-red-700 dark:text-red-400">
              <IconAlertCircle size={16} className="shrink-0 mt-0.5" aria-hidden="true" />
              <p role="alert" className="m-0 shrink-0 break-words text-xs leading-relaxed font-sans">{error}</p>
            </div>
          )}

          {discard && (
            <section role="alert" className="mx-5 mt-4 flex items-start gap-3 rounded-xl border border-amber-500/30 bg-amber-500/10 p-3.5 text-sm text-amber-900 dark:text-amber-200">
              <IconAlertTriangle size={18} className="shrink-0 mt-0.5 text-amber-500" aria-hidden="true" />
              <div className="grid gap-2.5 flex-1 min-w-0">
                <p className="m-0 text-xs font-medium leading-relaxed">
                  {discard === "close" ? "配置尚未保存，放弃修改并返回插件管理？" : "重新加载将丢弃尚未保存的修改，是否继续？"}
                </p>
                <div className="flex items-center gap-2">
                  <Button size="xs" variant="destructive" disabled={saving} onClick={() => { if (pending.current) return; if (discard === "close") onClose(); else void load(); }}>放弃修改</Button>
                  <Button size="xs" variant="outline" disabled={saving} onClick={() => setDiscard(null)}>继续编辑</Button>
                </div>
              </div>
            </section>
          )}

          <div className="flex-1 min-h-0 space-y-4 overflow-y-auto px-5 py-4" aria-busy={loading || saving}>
            <div className="rounded-xl border border-border/60 bg-muted/20 p-3 text-xs">
              <div className="flex items-center justify-between gap-2">
                <div className="flex items-center gap-2 min-w-0 text-muted-foreground">
                  <IconInfoCircle size={15} className="shrink-0 text-primary" aria-hidden="true" />
                  <span className="font-medium text-foreground text-[12px]">JSON 配置规则</span>
                  <span className="hidden sm:inline text-[11px] text-muted-foreground">· 字符串加引号，数字与布尔不加引号</span>
                </div>
                <button
                  type="button"
                  onClick={() => setRulesOpen(v => !v)}
                  className="flex items-center gap-1 text-[11px] text-primary hover:underline cursor-pointer select-none"
                >
                  <span>{rulesOpen ? "收起规则" : "查看说明"}</span>
                  {rulesOpen ? <IconChevronUp size={13} /> : <IconChevronDown size={13} />}
                </button>
              </div>
              {rulesOpen && (
                <div className="mt-2.5 pt-2.5 border-t border-border/40 space-y-1.5 text-[11px] text-muted-foreground leading-relaxed">
                  <p className="m-0">1. <span className="text-foreground font-medium">基本类型</span>：字符串带双引号（换行请写为 \n），数字和 true / false 不加引号。</p>
                  <p className="m-0">2. <span className="text-foreground font-medium">数组增删</span>：数值、文本等简单数组可直接增删元素；含对象的数组逐项修改字段值。</p>
                  <p className="m-0">3. <span className="text-foreground font-medium">只读与生效</span>：字段名和说明只读；保存后重新启用插件生效。</p>
                </div>
              )}
            </div>

            <div className="rounded-xl border border-border/60 bg-muted/20 p-3">
              <div className="flex items-center justify-between gap-2 mb-1.5">
                <div className="flex items-center gap-1.5">
                  <IconFileCode size={15} className="text-muted-foreground" aria-hidden="true" />
                  <span className="text-xs font-semibold text-foreground">config.json</span>
                  {edits.size > 0 && <span className="inline-flex items-center rounded-full bg-amber-500/15 px-1.5 py-0.2 text-[10px] font-medium text-amber-600 dark:text-amber-400">已修改</span>}
                </div>
                <button
                  type="button"
                  onClick={() => void copyPath()}
                  className="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground cursor-pointer select-none transition-colors"
                  title="复制完整路径"
                >
                  {copied ? <IconCheck size={13} className="text-green-600 dark:text-green-400" /> : <IconCopy size={13} />}
                  <span>{copied ? "已复制" : "复制路径"}</span>
                </button>
              </div>
              <p className="m-0 select-text break-all font-mono text-[11px] text-muted-foreground bg-background/60 rounded-md px-2 py-1 border border-border/40">
                {file?.path ?? plugin.configPath}
              </p>
            </div>

            {loading ? (
              <div className="flex items-center justify-center py-12 text-sm text-muted-foreground">
                <p role="status" className="m-0 text-sm text-muted-foreground">正在读取配置文件…</p>
              </div>
            ) : document && (
              <div
                aria-label="JSON 配置编辑器"
                className="overflow-x-auto rounded-xl border border-border/80 bg-background/60 p-3.5 font-mono text-sm shadow-xs"
              >
                <div className="leading-7 text-muted-foreground">{"{"}</div>
                <div className="min-w-0 pl-4 sm:pl-6">
                  {document.entries.map((entry, index) => renderEntry(entry, index === document.entries.length - 1))}
                </div>
                <div className="leading-7 text-muted-foreground">{"}"}</div>
              </div>
            )}
          </div>

          <DrawerFooter className="px-5 py-3.5 bg-background/95 backdrop-blur-sm border-t border-border/60">
            <Button size="sm" variant="outline" disabled={saving} onClick={() => request("close")}>
              返回插件管理
            </Button>
            <div className="flex items-center gap-2">
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
            </div>
          </DrawerFooter>
        </div>
      </DrawerContent>
    </Drawer>
  );
}
