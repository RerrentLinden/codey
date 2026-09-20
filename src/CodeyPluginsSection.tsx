import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  IconAlertTriangle,
  IconCheck,
  IconFilePlus,
  IconPuzzle,
  IconRefresh,
  IconSearch,
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
  Input,
  Switch,
} from "./components/ui";
import { PluginConfigDialog } from "./PluginConfigDialog";
import {
  parseCodeyPluginsResult,
  pluginStatusLabel,
  type CodeyPlugin,
  type CodeyPluginPreview,
  type CodeyPluginsResult,
} from "./codeyPlugins";
import { SettingsPageHeader } from "./SettingsPageHeader";

export function CodeyPluginsSection({ container }: { container?: HTMLElement | null }) {
  const [result, setResult] = useState<CodeyPluginsResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [customPath, setCustomPath] = useState("");
  const [showCustomPathInput, setShowCustomPathInput] = useState(false);
  const [preview, setPreview] = useState<CodeyPluginPreview | null>(null);
  const [editId, setEditId] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ kind: "enable" | "uninstall"; plugin: CodeyPlugin } | null>(null);
  const [removeData, setRemoveData] = useState(false);
  const [known, setKnown] = useState(false);

  const pending = useRef(false);
  const epoch = useRef(0);
  const configurationSaved = useRef(false);

  const accept = useCallback((data: unknown) => {
    try {
      const next = parseCodeyPluginsResult(data);
      setResult(next);
      setKnown(true);
      setError("");
    } catch (cause) {
      setKnown(false);
      throw cause;
    }
  }, []);

  const refresh = useCallback(async () => {
    const generation = epoch.current;
    setKnown(false);
    setPreview(null);
    setConfirm(null);
    try {
      const data = await invoke("list_codey_plugins");
      if (epoch.current === generation) accept(data);
    } catch (cause) {
      if (epoch.current === generation) {
        setKnown(false);
        setError(errorText(cause));
      }
      throw cause;
    }
  }, [accept]);

  const mutate = useCallback(async (
    command: "set_codey_plugin_enabled" | "install_codey_plugin" | "uninstall_codey_plugin",
    args: Record<string, unknown>,
  ) => {
    const generation = epoch.current;
    try {
      const data = await invoke(command, args);
      if (epoch.current !== generation) throw new Error("操作已过期");
      accept(data);
    } catch (cause) {
      if (epoch.current === generation) {
        try {
          await refresh();
        } catch {
          // refresh 已标记未知状态
        }
      }
      throw cause;
    }
  }, [accept, refresh]);

  const run = useCallback(async (action: () => Promise<void>, retry = false) => {
    if (pending.current || (!known && !retry)) return;
    const generation = ++epoch.current;
    pending.current = true;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await action();
    } catch (cause) {
      if (epoch.current === generation) setError(errorText(cause));
    } finally {
      if (epoch.current === generation) {
        pending.current = false;
        setBusy(false);
      }
    }
  }, [known]);

  useEffect(() => {
    let cancelled = false;
    const generation = ++epoch.current;
    setLoading(true);
    setError("");
    void invoke<CodeyPluginsResult>("list_codey_plugins")
      .then((data) => {
        if (cancelled || generation !== epoch.current) return;
        accept(data);
      })
      .catch((cause) => {
        if (cancelled || generation !== epoch.current) return;
        setResult(null);
        setError(errorText(cause));
      })
      .finally(() => {
        if (!cancelled && generation === epoch.current) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [accept]);

  const togglePlugin = useCallback(async (plugin: CodeyPlugin, enabled: boolean) => {
    await mutate("set_codey_plugin_enabled", { pluginId: plugin.id, enabled });
    setConfirm(null);
    setNotice(enabled ? `插件「${plugin.name}」已启用` : `插件「${plugin.name}」已停用`);
  }, [mutate]);

  const handleSelectPackage = useCallback(() => {
    void run(async () => {
      const selected = await invoke<CodeyPluginPreview | null>("select_codey_plugin_package");
      if (selected) {
        setPreview(selected);
        setConfirm(null);
      }
    }, true);
  }, [run]);

  const handleInspectCustomPath = useCallback(() => {
    if (!customPath.trim()) return;
    void run(async () => {
      const inspected = await invoke<CodeyPluginPreview>("inspect_codey_plugin", { path: customPath.trim() });
      setPreview(inspected);
      setConfirm(null);
    }, true);
  }, [customPath, run]);

  const filteredPlugins = useMemo(() => {
    if (!result?.plugins) return [];
    const q = searchQuery.trim().toLowerCase();
    if (!q) return result.plugins;
    return result.plugins.filter((p) =>
      p.name.toLowerCase().includes(q) ||
      p.id.toLowerCase().includes(q) ||
      (p.description && p.description.toLowerCase().includes(q)) ||
      (p.capabilities && p.capabilities.some((c) => c.toLowerCase().includes(q)))
    );
  }, [result?.plugins, searchQuery]);

  const upgrading = preview && result?.plugins.find((p) => p.id === preview.manifest.id);
  const blocked = busy || !known || loading;
  const editing = result?.plugins.find((p) => p.id === editId);

  return (
    <section className="secondary-section codey-plugins-section" aria-labelledby="codey-plugins-title">
      <SettingsPageHeader
        id="codey-plugins-title"
        title="Codey 插件"
        icon={<IconPuzzle size={15} />}
        description="安装独立功能模块，扩展 Codex 能力与环境支持。"
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={loading || busy}
              onClick={() => void run(refresh, true)}
              aria-label="刷新插件列表"
            >
              <IconRefresh size={14} className={loading ? "animate-spin" : ""} />
              <span>刷新</span>
            </Button>
            {result?.platform !== "linux" && (
              <Button
                size="sm"
                variant="default"
                disabled={blocked}
                onClick={handleSelectPackage}
              >
                <IconFilePlus size={14} aria-hidden="true" />
                <span>导入插件包</span>
              </Button>
            )}
          </div>
        }
      />

      {/* 顶部统计与工具栏 */}
      <div className="mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
          {loading ? (
            <span>正在读取插件…</span>
          ) : result ? (
            <>
              <Badge variant="secondary" className="font-normal">
                共 {result.plugins.length} 个插件
              </Badge>
              <Badge variant="success" className="font-normal">
                已启用 {result.plugins.filter((p) => p.enabled).length} 个
              </Badge>
              <span className="hidden text-muted/40 sm:inline">·</span>
              <span className="font-mono text-muted/75">
                {result.platform} / {result.arch}
              </span>
            </>
          ) : (
            <span className="text-red-500">插件状态未知，请点击刷新重试</span>
          )}
        </div>

        <div className="flex items-center gap-2">
          <div className="relative min-w-[200px] flex-1 sm:w-60">
            <Input
              aria-label="搜索插件"
              placeholder="搜索插件名称、ID 或能力…"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              leftSection={<IconSearch size={14} className="text-muted" />}
              className="h-8 text-xs"
            />
          </div>
          <Button
            size="sm"
            variant="ghost"
            className="text-xs text-muted hover:text-foreground"
            onClick={() => setShowCustomPathInput((prev) => !prev)}
          >
            {showCustomPathInput ? "收起路径导入" : "本地路径导入"}
          </Button>
        </div>
      </div>

      {/* 本地文件路径展开导入框 */}
      {showCustomPathInput && (
        <div className="mb-4 rounded-xl border border-default/70 bg-default/20 p-3 text-xs">
          <div className="mb-1.5 font-medium text-foreground">通过本地文件路径导入插件</div>
          <div className="flex gap-2">
            <Input
              aria-label="插件包路径"
              placeholder=".codey-plugin 文件的完整路径"
              value={customPath}
              disabled={blocked}
              onChange={(e) => {
                setCustomPath(e.target.value);
                setPreview(null);
              }}
              className="flex-1 text-xs"
            />
            <Button
              variant="outline"
              size="sm"
              disabled={blocked || !customPath.trim()}
              onClick={handleInspectCustomPath}
            >
              检查安装包
            </Button>
          </div>
        </div>
      )}

      {/* 提示与错误信息 */}
      {error && (
        <div role="alert" className="mb-4 flex items-start gap-2 rounded-xl border border-red-200/60 bg-red-50/70 p-3 text-xs text-red-700 dark:border-red-900/60 dark:bg-red-950/40 dark:text-red-300">
          <IconAlertTriangle size={15} className="mt-0.5 shrink-0" />
          <div className="flex-1 break-words">{error}</div>
        </div>
      )}
      {notice && (
        <div role="status" className="mb-4 flex items-center gap-2 rounded-xl border border-green-200/60 bg-green-50/70 p-3 text-xs text-green-700 dark:border-green-900/60 dark:bg-green-950/40 dark:text-green-300">
          <IconCheck size={15} className="shrink-0" />
          <div className="flex-1">{notice}</div>
        </div>
      )}

      {/* 安装 / 升级预览 Card */}
      {preview && (
        <div className="mb-5 overflow-hidden rounded-2xl border-2 border-blue-500/30 bg-blue-50/20 p-5 dark:border-blue-500/40 dark:bg-blue-950/20">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="flex items-center gap-2">
                <span className="rounded-md bg-blue-500/10 px-2 py-0.5 text-xs font-semibold text-blue-600 dark:text-blue-400">
                  {upgrading ? "升级确认" : "安装确认"}
                </span>
                <h3 className="m-0 text-base font-semibold text-foreground">
                  {preview.manifest.name}
                </h3>
              </div>
              <p className="mb-0 mt-1 font-mono text-xs text-muted">
                {preview.manifest.id} · {upgrading ? `${upgrading.version} → ` : "v"}{preview.manifest.version}
              </p>
            </div>
            <div className="flex items-center gap-2">
              <Button
                size="sm"
                variant="default"
                disabled={blocked}
                onClick={() =>
                  void run(async () => {
                    await mutate("install_codey_plugin", { path: preview.path, sha256: preview.sha256 });
                    setPreview(null);
                    setNotice(`插件「${preview.manifest.name}」安装成功，默认处于停用状态。`);
                  })
                }
              >
                {upgrading ? "确认升级" : "确认安装"}
              </Button>
              <Button size="sm" variant="outline" disabled={blocked} onClick={() => setPreview(null)}>
                取消
              </Button>
            </div>
          </div>

          {preview.manifest.description && (
            <p className="mb-0 mt-3 text-xs text-foreground/80 leading-relaxed">
              {preview.manifest.description}
            </p>
          )}

          <div className="mt-3 rounded-lg bg-black/[0.03] p-3 dark:bg-white/[0.04]">
            <div className="text-xs text-muted">
              <strong>声明能力：</strong>
              {[...(preview.manifest.capabilities ?? []), ...(preview.manifest.permissions ?? [])].join("、") || "无"}
            </div>
            {(preview.manifest.headerNames?.length ?? 0) > 0 && (
              <div className="mt-1 text-xs text-muted">
                <strong>可修改请求头：</strong>
                {preview.manifest.headerNames!.join("、")}
              </div>
            )}
            <p className="mb-0 mt-2 text-[11px] text-amber-600 dark:text-amber-400">
              提示：原生插件与 Codey 在同一进程运行，具有相同系统权限。安装后默认停用，可在下方卡片中配置并信任启用。
            </p>
          </div>
        </div>
      )}

      {/* 插件卡片网格 */}
      {filteredPlugins.length === 0 ? (
        known && !loading && <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-default py-14 text-center">
          <div className="mb-3 flex size-12 items-center justify-center rounded-2xl bg-default/40 text-muted">
            <IconPuzzle size={24} stroke={1.5} />
          </div>
          {searchQuery ? (
            <>
              <h4 className="m-0 text-sm font-semibold text-foreground">未找到匹配的插件</h4>
              <p className="mb-3 mt-1 text-xs text-muted">没有符合「{searchQuery}」的插件，请尝试其他关键词。</p>
              <Button size="xs" variant="outline" onClick={() => setSearchQuery("")}>
                清除搜索
              </Button>
            </>
          ) : (
            <>
              <h4 className="m-0 text-sm font-semibold text-foreground">尚未安装任何插件</h4>
              <p className="mb-4 mt-1 max-w-sm text-xs text-muted leading-relaxed">
                通过导入 .codey-plugin 插件包，一键扩展环境适配、模型代理拦截与自定增强功能。
              </p>
              {result?.platform !== "linux" ? (
                <Button size="sm" variant="default" disabled={blocked} onClick={handleSelectPackage}>
                  <IconFilePlus size={14} aria-hidden="true" />
                  <span>导入第一个插件包</span>
                </Button>
              ) : (
                <Button size="sm" variant="outline" onClick={() => setShowCustomPathInput(true)}>
                  填写本地路径导入
                </Button>
              )}
            </>
          )}
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
          {filteredPlugins.map((plugin) => {
            const hasError = Boolean(plugin.lastError || plugin.status === "failed" || plugin.status === "error");
            const badgeVariant = !known
              ? "secondary"
              : hasError
                ? "destructive"
                : plugin.restartRequired
                  ? "warning"
                  : plugin.enabled
                    ? "success"
                    : "secondary";

            return (
              <article
                key={plugin.id}
                className="codey-card flex flex-col justify-between"
              >
                <div className="p-5">
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex items-start gap-3 min-w-0 flex-1">
                      <div
                        className={`flex size-10 shrink-0 items-center justify-center rounded-xl transition-colors ${
                          plugin.enabled
                            ? "bg-blue-500/10 text-blue-600 dark:bg-blue-500/20 dark:text-blue-400 ring-1 ring-blue-500/20"
                            : "bg-default/40 text-muted"
                        }`}
                      >
                        <IconPuzzle size={20} stroke={1.8} aria-hidden="true" />
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-1.5">
                          <h4 className="m-0 truncate text-sm font-semibold text-foreground">
                            {plugin.name}
                          </h4>
                          <span className="inline-flex items-center rounded-md border border-black/[0.06] bg-black/[0.03] px-1.5 py-0.5 font-mono text-[10.5px] font-medium text-muted dark:border-white/[0.06] dark:bg-white/[0.05]">
                            v{plugin.version}
                          </span>
                        </div>
                        <p className="mb-0 mt-0.5 truncate font-mono text-xs text-muted/75">
                          {plugin.id}
                        </p>
                      </div>
                    </div>
                    <div className="flex items-center gap-2.5 shrink-0">
                      <Badge variant={badgeVariant}>
                        {!known && "上次状态："}
                        {pluginStatusLabel(plugin)}
                      </Badge>
                      <Switch
                        size="sm"
                        checked={plugin.enabled}
                        disabled={blocked}
                        aria-label={plugin.enabled ? `停用插件 ${plugin.name}` : `启用插件 ${plugin.name}`}
                        onCheckedChange={(checked) => {
                          if (checked) {
                            setConfirm({ kind: "enable", plugin });
                            setPreview(null);
                          } else {
                            void run(() => togglePlugin(plugin, false));
                          }
                        }}
                      />
                    </div>
                  </div>

                  {plugin.description && (
                    <p className="mb-0 mt-3 line-clamp-2 text-xs text-foreground/80 leading-relaxed">
                      {plugin.description}
                    </p>
                  )}

                  {plugin.capabilities && plugin.capabilities.length > 0 && (
                    <div className="mt-3 flex flex-wrap gap-1.5">
                      {plugin.capabilities.map((cap) => (
                        <span
                          key={cap}
                          className="inline-flex items-center rounded-md bg-default/40 px-2 py-0.5 text-[11px] text-muted"
                        >
                          {cap}
                        </span>
                      ))}
                    </div>
                  )}

                  {plugin.restartRequired && (
                    <div className="mt-3 rounded-lg border border-amber-200/70 bg-amber-50/70 p-2.5 text-[11.5px] text-amber-800 dark:border-amber-900/60 dark:bg-amber-950/40 dark:text-amber-300">
                      需要重启 Codey 或停用后重新启用，以应用版本更新或配置变更。
                    </div>
                  )}

                  {plugin.lastError && (
                    <div role="alert" className="mt-3 rounded-lg border border-red-200/70 bg-red-50/70 p-2.5 text-[11.5px] text-red-700 dark:border-red-900/60 dark:bg-red-950/40 dark:text-red-300">
                      {plugin.lastError}
                    </div>
                  )}

                  {plugin.pluginDir && (
                    <details className="mt-3 text-[11.5px] text-muted">
                      <summary className="cursor-pointer select-none font-medium hover:text-foreground">
                        文件与存储目录
                      </summary>
                      <dl className="mt-2 grid gap-1 rounded-lg bg-black/[0.02] p-2.5 font-mono text-[11px] dark:bg-white/[0.03]">
                        <div className="flex flex-col">
                          <dt className="text-muted/60">插件目录</dt>
                          <dd className="m-0 break-all select-text text-foreground/90">{plugin.pluginDir}</dd>
                        </div>
                        {plugin.dataDir && (
                          <div className="flex flex-col">
                            <dt className="text-muted/60">数据目录</dt>
                            <dd className="m-0 break-all select-text text-foreground/90">{plugin.dataDir}</dd>
                          </div>
                        )}
                        {plugin.logDir && (
                          <div className="flex flex-col">
                            <dt className="text-muted/60">日志目录</dt>
                            <dd className="m-0 break-all select-text text-foreground/90">{plugin.logDir}</dd>
                          </div>
                        )}
                      </dl>
                    </details>
                  )}
                </div>

                <div className="flex items-center justify-between gap-2 border-t border-black/[0.06] bg-black/[0.015] px-5 py-2.5 dark:border-white/[0.06] dark:bg-white/[0.02]">
                  <div className="flex items-center gap-2">
                    {plugin.restartRequired && plugin.enabled ? (
                      <Button
                        size="xs"
                        variant="warning"
                        disabled={blocked}
                        onClick={() =>
                          void run(async () => {
                            await mutate("set_codey_plugin_enabled", { pluginId: plugin.id, enabled: false });
                            try {
                              await togglePlugin(plugin, true);
                              setNotice("插件已重新启用，变更已生效");
                            } catch (cause) {
                              const message = errorText(cause);
                              throw new Error(`重新启用失败：${message}。请检查状态后重试启用。`);
                            }
                          })
                        }
                      >
                        <IconRefresh size={13} aria-hidden="true" />
                        <span>重新启用</span>
                      </Button>
                    ) : null}
                  </div>

                  <div className="flex items-center gap-1.5 ml-auto">
                    <Button
                      size="icon-sm"
                      variant="outline"
                      disabled={blocked}
                      title="插件配置"
                      aria-label={`配置 ${plugin.name}`}
                      onClick={() => setEditId(plugin.id)}
                    >
                      <IconSettings size={14} aria-hidden="true" />
                    </Button>

                    <Button
                      size="icon-sm"
                      variant="destructive-light"
                      disabled={blocked || plugin.enabled}
                      title={plugin.enabled ? "请先停用插件再卸载" : "卸载插件"}
                      aria-label={`卸载 ${plugin.name}`}
                      onClick={() => {
                        setRemoveData(false);
                        setConfirm({ kind: "uninstall", plugin });
                        setPreview(null);
                      }}
                    >
                      <IconTrash size={14} aria-hidden="true" />
                    </Button>
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      )}

      {/* 启用信任 / 卸载确认对话框 */}
      {confirm && (
        <Dialog open={Boolean(confirm)} onOpenChange={(open) => { if (!open) setConfirm(null); }}>
          <DialogContent container={container} className="max-w-md">
            <DialogHeader>
              <DialogTitle>
                {confirm.kind === "enable" ? "信任并启用插件" : "确认卸载插件"}
              </DialogTitle>
              <DialogDescription>
                {confirm.kind === "enable" ? (
                  <>
                    您正在启用「<strong>{confirm.plugin.name}</strong>」。
                  </>
                ) : (
                  <>
                    确定要从系统卸载「<strong>{confirm.plugin.name}</strong>」吗？
                  </>
                )}
              </DialogDescription>
            </DialogHeader>

            <div className="py-2 text-xs">
              {confirm.kind === "enable" ? (
                <div className="space-y-2">
                  <p className="m-0 break-words text-muted">
                    声明能力：{confirm.plugin.capabilities.join("、") || "无特定权限声明"}
                  </p>
                  <p className="m-0 rounded-lg bg-amber-500/10 p-2.5 text-amber-700 dark:text-amber-300">
                    启用后将执行插件本地代码，插件具有与 Codey 相同的系统运行权限。请确认来源安全。
                  </p>
                </div>
              ) : (
                <div className="space-y-3">
                  <p className="m-0 text-muted">
                    卸载后将移除插件安装包与加载入口。
                  </p>
                  <Checkbox
                    disabled={blocked}
                    checked={removeData}
                    onCheckedChange={(next) => setRemoveData(next === true)}
                    label="同时彻底删除该插件的配置、数据和历史日志（默认保留数据）"
                  />
                </div>
              )}
            </div>

            <div className="flex justify-end gap-2 pt-2">
              <Button variant="outline" size="sm" disabled={blocked} onClick={() => setConfirm(null)}>
                取消
              </Button>
              <Button
                size="sm"
                variant={confirm.kind === "uninstall" ? "destructive" : "default"}
                disabled={blocked}
                onClick={() =>
                  void run(async () => {
                    if (confirm.kind === "enable") {
                      await togglePlugin(confirm.plugin, true);
                    } else {
                      await mutate("uninstall_codey_plugin", {
                        pluginId: confirm.plugin.id,
                        removeData,
                      });
                      setConfirm(null);
                      setEditId(null);
                      setNotice(`插件「${confirm.plugin.name}」已成功卸载`);
                    }
                  })
                }
              >
                {confirm.kind === "enable" ? "信任并启用" : "确认卸载"}
              </Button>
            </div>
          </DialogContent>
        </Dialog>
      )}

      {/* 插件独立配置弹层 */}
      {editing && (
        <PluginConfigDialog
          key={editing.id}
          plugin={editing}
          container={container}
          onClose={() => {
            setEditId(null);
            if (!configurationSaved.current) void run(refresh, true);
            configurationSaved.current = false;
          }}
          onChanged={(data) => {
            accept(data);
            configurationSaved.current = true;
            setNotice(`插件「${editing.name}」配置已保存`);
          }}
        />
      )}
    </section>
  );
}
