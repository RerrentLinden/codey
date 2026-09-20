import { useState, useMemo, useDeferredValue, useEffect } from "react";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "../../components/ui";
import {
  IconBook2,
  IconPlus,
  IconRefresh,
  IconServer,
} from "@tabler/icons-react";
import { SettingsPageHeader } from "../../SettingsPageHeader";
import { ConfirmationDialog } from "./ConfirmationDialog";
import { SkillCacheDialog } from "./SkillCacheDialog";
import { ExtensionEditor } from "./ExtensionEditor";
import { ExtensionScope, ExtensionFilters } from "./ExtensionToolbar";
import { ExtensionError } from "./ExtensionError";
import { ExtensionList } from "./ExtensionList";
import {
  draftChanged,
  selectResources,
  pageResources,
  canToggle,
  batchTargets,
  editorError,
} from "./state";
import type {
  Confirmation,
  EditorDraft,
  ExtensionTransport,
  McpEntry,
  SkillEntry,
} from "./types";
import {
  useExtensionsController,
  type RunOutcome,
} from "./useExtensionsController";
import { downloadResource } from "./download";
import {
  NEW_MCP_JSON,
  selectedMcpJson,
  mcpJsonError,
  exportMcpJson,
} from "./mcpJson";

export function CodexExtensionsPage({
  kind,
  request,
  container,
  active = true,
}: {
  kind: "mcp" | "skill";
  request: ExtensionTransport;
  container?: HTMLElement | null;
  active?: boolean;
}) {
  const controller = useExtensionsController(request, active);
  const { inventory, scope, busy, loading } = controller;
  const [cacheOpen, setCacheOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [source, setSource] = useState("all");
  const [sort, setSort] = useState("name");
  const [page, setPage] = useState(1);
  const [selected, setSelected] = useState<string[]>([]);
  const [projectMode, setProjectMode] = useState(false);
  const deferredQuery = useDeferredValue(query);
  const [projectPath, setProjectPath] = useState("");
  const [draft, setDraft] = useState<EditorDraft | null>(null);
  const [comparison, setComparison] = useState<{
    content: string;
    revision: string;
  } | null>(null);
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [discard, setDiscard] = useState<(() => void) | null>(null);
  useEffect(() => setComparison(null), [draft?.id, draft?.kind]);
  useEffect(() => {
    setPage(1);
    setSelected([]);
  }, [deferredQuery, filter, source, sort, inventory?.revision, scope]);
  const guard = (next: () => void) => {
    if (busy) return;
    if (draftChanged(draft)) setDiscard(() => next);
    else next();
  };
  const pickProject = () =>
    void controller.run<{ path: string } | null>(
      { action: "pick_project" },
      (result) => {
        if (result) setProjectPath(result.path);
      },
    );
  const pickSkill = () =>
    void controller.run<{ path: string } | null>(
      { action: "pick_skill" },
      (result) => {
        if (result)
          setDraft((current) =>
            current ? { ...current, content: result.path } : current,
          );
      },
    );
  const changeScope = (project: boolean) =>
    guard(() => {
      setDraft(null);
      controller.setScope(
        project
          ? { kind: "project", projectPath: projectPath.trim() }
          : { kind: "user" },
      );
    });
  const edit = (kind: "mcp" | "skill", entry: McpEntry | SkillEntry) => {
    if (kind === "mcp")
      void controller.run<{
        id: string;
        configJson: Record<string, unknown>;
        revision: string;
      }>({ action: "get_mcp", id: entry.id }, (result) =>
        setDraft({
          kind,
          id: result.id,
          originalId: result.id,
          content: JSON.stringify(result.configJson, null, 2),
          original: JSON.stringify(result.configJson, null, 2),
          revision: result.revision,
          readOnly: entry.readOnly || entry.canEdit === false,
          entry,
        }),
      );
    else
      void controller.run<{
        id: string;
        content: string;
        revision: string;
        readOnly: boolean;
      }>({ action: "read_skill", id: entry.id }, (result) =>
        setDraft({
          kind,
          id: result.id,
          originalId: result.id,
          content: result.content,
          original: result.content,
          revision: result.revision,
          readOnly:
            result.readOnly ||
            entry.canEdit === false ||
            !("ownership" in entry) ||
            entry.ownership !== "managed",
          entry,
        }),
      );
  };
  const onAction = (action: string, entry: McpEntry | SkillEntry) => {
    if (action === "export") {
      if (kind === "mcp")
        void controller.run<{ configJson: Record<string, unknown> }>(
          { action: "get_mcp", id: entry.id },
          (result) =>
            downloadResource(
              `${entry.id}.json`,
              exportMcpJson(entry.id, result.configJson),
              "application/json",
            ),
        );
      else
        void controller.run<{
          filename: string;
          mediaType: string;
          dataBase64: string;
        }>({ action: "export_skill", id: entry.id }, (result) =>
          downloadResource(
            result.filename,
            Uint8Array.from(atob(result.dataBase64), (char) =>
              char.charCodeAt(0),
            ),
            result.mediaType,
          ),
        );
      return;
    }
    if (action === "edit") {
      edit(kind, entry);
      return;
    }
    if (action === "check") {
      if (kind === "skill") {
        void controller.inspect({ action: "validate_skill", id: entry.id });
        return;
      }
      setConfirmation({
        title: `测试 ${entry.name}`,
        description:
          "测试将执行该 MCP 配置的启动命令，或连接远程地址，并完成协议握手。程序可能访问当前用户允许的文件和网络。仅在信任此服务时继续；测试不会调用业务工具。",
        action: {
          action: "test_mcp",
          id: entry.id,
          revision: inventory?.revision,
          confirmed: true,
        },
      });
      return;
    }
    if (action === "toggle") {
      const toggle = {
        action: `set_${kind}_enabled`,
        id: entry.id,
        enabled: !entry.enabled,
        confirmed: true,
      };
      if (!entry.enabled)
        setConfirmation({
          title: `启用 ${entry.name}`,
          description:
            kind === "mcp"
              ? "启用后将自动刷新 Codex 的 MCP 配置。请确认信任其内容和来源。"
              : "启用后 Codex 可能在新会话中加载该资源。请确认信任其内容和来源。",
          action: toggle,
        });
      else void controller.mutate(toggle);
      return;
    }
    setConfirmation({
      title: `${kind === "mcp" ? "移除" : "卸载"} ${entry.name}`,
      description:
        kind === "mcp"
          ? "将移除该服务的配置注册并自动刷新 Codex，不删除外部程序。移除后如需再次使用，请重新导入配置。"
          : "ownership" in entry && entry.ownership === "external"
            ? "将直接删除该外部安装目录及其全部资源文件，不经过 Codey 托管记录，删除后无法从本页恢复。目录内容无法完整校验时会拒绝删除。"
            : "将删除这份 Codey 托管安装及其本地文件，保留原始来源。卸载后如需再次使用，请重新安装。若文件已被外部修改，后端可能拒绝卸载。",
      destructive: true,
      action: {
        action: kind === "mcp" ? "remove_mcp" : "uninstall_skill",
        id: entry.id,
        confirmed: true,
      },
    });
  };
  const save = async () => {
    if (!draft || busy || loading || mcpJsonError(draft) || editorError(draft))
      return;
    if (
      draft.kind === "mcp" &&
      draft.isNew &&
      inventory?.mcps.some((entry) => entry.id === draft.id)
    )
      return;
    const action =
      draft.kind === "install"
        ? { action: "install_skill", sourcePath: draft.content.trim() }
        : draft.kind === "mcp"
          ? {
              action: "save_mcp",
              id: draft.id,
              configJson: selectedMcpJson(draft).config,
              createOnly: draft.isNew === true,
              confirmed: draft.isNew === true,
            }
          : {
              action: draft.isNew ? "create_skill" : "save_skill",
              id: draft.id,
              content: draft.content,
            };
    if (
      draft.kind === "mcp" &&
      !draft.isNew &&
      (draft.entry?.enabled || selectedMcpJson(draft).config.enabled !== false)
    ) {
      setConfirmation({
        title: "保存 MCP 配置",
        description:
          "保存后会立即刷新到 Codex 并生效，无需重启；草稿未显式禁用的服务会被启用。请确认信任修改后的程序或远程地址。",
        action: { ...action, revision: draft.revision, confirmed: true },
      });
      return;
    }
    settleSave(await controller.mutate({ ...action, revision: draft.revision }));
  };

  // 保存已落库（ok）时清掉草稿；被抢占（superseded）时后端同样已写入，
  // 继续保留草稿会让用户误以为未提交，因此清草稿并刷新清单确认最新状态。
  const settleSave = (outcome: RunOutcome) => {
    if (outcome === "failed") return;
    setDraft(null);
    if (outcome === "superseded") void controller.refresh();
  };

  const loadLatest = () => {
    if (!draft) return;
    if (draft.isNew || draft.kind === "install") {
      // 新建没有旧文件可比对；由用户明确刷新范围后保留草稿并重新检查重名。
      void controller.refresh().then((latest) => {
        if (latest)
          setDraft((current) =>
            current === draft
              ? { ...current, revision: latest.revision }
              : current,
          );
      });
      return;
    }
    void controller.run<{
      content?: string;
      configJson?: Record<string, unknown>;
      revision: string;
    }>(
      { action: draft.kind === "mcp" ? "get_mcp" : "read_skill", id: draft.id },
      (result) =>
        setComparison({
          content:
            draft.kind === "mcp"
              ? JSON.stringify(result.configJson, null, 2)
              : (result.content ?? ""),
          revision: result.revision,
        }),
    );
  };
  const startDraft = (kind: "mcp" | "skill" | "install") => {
    controller.clearError();
    setComparison(null);
    setDraft({
      kind,
      id: kind === "mcp" ? "my-server" : "",
      originalId: "",
      content:
        kind === "mcp"
          ? NEW_MCP_JSON
          : kind === "skill"
            ? "---\nname: new-skill\ndescription: 描述此技能适用的任务。\n---\n\n# 使用说明\n"
            : "",
      original: "",
      revision: inventory?.revision ?? "",
      readOnly: false,
      isNew: true,
    });
  };
  const unavailable = busy || loading || !inventory;
  const title = kind === "mcp" ? "MCP 管理" : "Skill 管理";
  const entries = inventory
    ? kind === "mcp"
      ? inventory.mcps
      : inventory.skills.filter((entry) => entry.ownership !== "plugin" && entry.scope !== "plugin")
    : [];
  const filtered = useMemo(
    () => selectResources(entries, deferredQuery, filter, source, sort),
    [inventory, kind, deferredQuery, filter, source, sort],
  );
  const currentPage = pageResources(filtered, page);
  const clearFilters = () => {
    setQuery("");
    setFilter("all");
    setSource("all");
  };
  const enableTargets = batchTargets(entries, selected, true);
  const disableTargets = batchTargets(entries, selected, false);
  const batch = (enabled: boolean) => {
    const ids = enabled ? enableTargets : disableTargets;
    if (!ids.length) return;
    setConfirmation({
      title: `${enabled ? "启用" : "禁用"} ${ids.length} 项`,
      description: `选中 ${selected.length} 项，其中 ${ids.length} 项需要变更。${enabled ? "请确认信任这些资源的内容与来源。" : ""}${kind === "mcp" ? "保存后自动刷新 Codex 的 MCP 配置。" : "新会话的实际加载状态需在 Codex 中确认。"}`,
      action: {
        action: kind === "mcp" ? "set_mcps_enabled" : "set_skills_enabled",
        ids,
        enabled,
        confirmed: true,
      },
    });
  };
  if (!active) return null;
  return (
    <section
      className="secondary-section"
      aria-labelledby={`codex-${kind}-title`}
    >
      <SettingsPageHeader
        id={`codex-${kind}-title`}
        title={title}
        icon={
          kind === "mcp" ? <IconServer size={15} /> : <IconBook2 size={15} />
        }
        badge={
          <span className="codey-badge codey-badge-info">
            {kind === "mcp" ? "Model Context Protocol" : "Codex 增强技能"}
          </span>
        }
        description={
          kind === "mcp"
            ? "管理 MCP 服务配置，检查连接与可用性。"
            : "管理本地 Skill，检查配置与可用性。"
        }
        actions={
          <div className="flex flex-wrap items-center gap-2">
            {kind === "skill" && (
              <Button size="sm" variant="outline" onClick={() => setCacheOpen(true)}>
                缓存 Skill 管理
              </Button>
            )}
            <Button
              size="sm"
              variant="outline"
              disabled={busy || loading}
              onClick={() => void controller.refresh()}
            >
              <IconRefresh size={14} className="mr-1 inline-block" />
              刷新
            </Button>
            <Button
              size="sm"
              disabled={unavailable}
              onClick={() => startDraft(kind)}
            >
              <IconPlus size={14} className="mr-1 inline-block" />
              {kind === "mcp" ? "新增 MCP" : "新增 Skill"}
            </Button>
            {kind === "skill" && (
              <Button
                size="sm"
                variant="outline"
                disabled={unavailable}
                onClick={() => startDraft("install")}
              >
                导入目录 / ZIP
              </Button>
            )}
          </div>
        }
      />
      <div className="space-y-4">
        <ExtensionScope
          kind={kind}
          scope={scope}
          inventory={inventory}
          count={entries.length}
          projectMode={projectMode}
          projectPath={projectPath}
          busy={busy}
          onMode={(project) => {
            setProjectMode(project);
            if (!project) changeScope(false);
          }}
          onPath={setProjectPath}
          onPick={pickProject}
          onRead={() => changeScope(true)}
        />

        {controller.error && !draft && (
          <div className="space-y-2">
            <ExtensionError message={controller.error} />
            <Button
              size="sm"
              variant="outline"
              disabled={busy || loading}
              onClick={() => void controller.refresh()}
            >
              重新读取当前范围
            </Button>
          </div>
        )}
        {controller.notice && (
          <p role="status" className="rounded-lg bg-emerald-500/10 p-3 text-xs">
            {controller.notice}
          </p>
        )}
        {inventory?.warnings.map((warning, index) => (
          <p
            key={index}
            role="status"
            className="m-0 break-words text-xs text-warning"
          >
            {warning}
          </p>
        ))}
        <ExtensionFilters
          kind={kind}
          query={query}
          filter={filter}
          source={source}
          sort={sort}
          onQuery={setQuery}
          onFilter={setFilter}
          onSource={setSource}
          onSort={setSort}
          onClear={clearFilters}
        />
        {inventory && (
          <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
            <span>
              匹配 {filtered.length} / {entries.length} 项
            </span>
            <Button
              size="xs"
              variant="outline"
              disabled={unavailable || !currentPage.entries.some(canToggle)}
              onClick={() =>
                setSelected(
                  currentPage.entries
                    .filter(canToggle)
                    .map((entry) => entry.id),
                )
              }
            >
              选择本页可操作项
            </Button>
            {selected.length > 0 && (
              <>
                <span>已选 {selected.length} 项，最多 100 项</span>
                <Button
                  size="xs"
                  variant="outline"
                  disabled={
                    unavailable || !batchTargets(entries, selected, true).length
                  }
                  onClick={() => batch(true)}
                >
                  启用
                </Button>
                <Button
                  size="xs"
                  variant="outline"
                  disabled={
                    unavailable ||
                    !batchTargets(entries, selected, false).length
                  }
                  onClick={() => batch(false)}
                >
                  禁用
                </Button>
                <Button
                  size="xs"
                  variant="outline"
                  onClick={() => setSelected([])}
                >
                  清空选择
                </Button>
              </>
            )}
          </div>
        )}
        {loading && (
          <p role="status" className="p-6 text-center text-muted">
            {inventory ? "正在刷新，暂时显示上次读取结果…" : "正在读取配置…"}
          </p>
        )}
        {inventory && (
          <ExtensionList
            kind={kind}
            entries={currentPage.entries}
            busy={busy || loading}
            onAction={onAction}
            selected={selected}
            onSelect={(id) =>
              setSelected((current) =>
                current.includes(id)
                  ? current.filter((value) => value !== id)
                  : [...current, id].slice(0, 100),
              )
            }
            checks={controller.checks}
            revision={inventory.revision}
            filtered={!!query.trim() || filter !== "all" || source !== "all"}
          />
        )}
        {inventory && filtered.length > 0 && (
          <nav
            aria-label="资源分页"
            className="flex flex-wrap items-center justify-end gap-3 text-xs text-muted"
          >
            <span>
              第 {currentPage.page} / {currentPage.pages} 页 · 每页 20 项
            </span>
            <Button
              size="sm"
              variant="outline"
              disabled={currentPage.page <= 1}
              onClick={() => setPage(currentPage.page - 1)}
            >
              上一页
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={currentPage.page >= currentPage.pages}
              onClick={() => setPage(currentPage.page + 1)}
            >
              下一页
            </Button>
          </nav>
        )}
        {controller.check && (
          <div
            role={controller.check.ok ? "status" : "alert"}
            className={`codey-card p-4 border-l-4 ${
              controller.check.ok
                ? "border-l-emerald-500"
                : "border-l-amber-500"
            }`}
          >
            <div className="flex items-center gap-2 mb-2">
              <span
                className={`codey-badge ${
                  controller.check.ok
                    ? "codey-badge-success"
                    : "codey-badge-warning"
                }`}
              >
                {controller.check.ok ? "检查通过" : "存在异常"}
              </span>
              <h3 className="m-0 text-sm font-semibold">
                {entries.find((entry) => entry.id === controller.check?.id)
                  ?.name ?? controller.check.id}
                ：{controller.check.summary}
              </h3>
            </div>
            {controller.check.revision !== inventory?.revision && (
              <p className="text-xs text-warning">
                配置已变化，此检查结果已失效。
              </p>
            )}
            {(controller.check.serverInfo?.name ||
              controller.check.serverInfo?.version ||
              controller.check.protocolVersion) && (
              <p className="text-xs text-muted">
                服务：{controller.check.serverInfo?.name ?? "未返回名称"}{" "}
                {controller.check.serverInfo?.version ?? ""} · 协议：
                {controller.check.protocolVersion ?? "未返回"}
              </p>
            )}
            <ul className="mb-0 space-y-1.5 pl-4 text-xs text-muted">
              {controller.check.checks.map((item, index) => (
                <li key={index} className="flex items-center gap-2">
                  <span
                    className={`inline-block w-1.5 h-1.5 rounded-full ${
                      item.ok ? "bg-emerald-500" : "bg-amber-500"
                    }`}
                  />
                  <span className="font-medium text-[var(--color-text-secondary)]">
                    {item.name}：
                  </span>
                  <span>{item.message}</span>
                </li>
              ))}
            </ul>
          </div>
        )}
        {(controller.busyAction === "test_mcp" ||
          controller.busyAction === "validate_skill") && (
          <p role="status" className="text-sm text-muted">
            正在检查资源，请稍候…
          </p>
        )}
      </div>
      <SkillCacheDialog open={cacheOpen && kind === "skill"} request={request} container={container} onClose={() => setCacheOpen(false)} />
      {draft && (
        <Dialog
          open
          onOpenChange={(open) => {
            if (!open) guard(() => setDraft(null));
          }}
        >
          <DialogContent
            container={container}
            className="sm:w-[760px]"
            onEscapeKeyDown={(event) => {
              if (busy || discard) event.preventDefault();
            }}
          >
            <DialogHeader>
              <DialogTitle>
                {draft.kind === "install"
                  ? "新增 Skill"
                  : `${draft.readOnly ? "查看" : draft.isNew ? "新增" : "编辑"} ${draft.kind === "mcp" ? "MCP" : "Skill"}`}
              </DialogTitle>
              <DialogDescription>
                {draft.kind === "install"
                  ? "选择本地 Skill 目录或 ZIP 文件进行安装。"
                  : "查看或修改当前范围内的资源配置。"}
              </DialogDescription>
            </DialogHeader>
            <div className="max-h-[70vh] overflow-y-auto px-6 pb-6">
              <ExtensionEditor
                draft={draft}
                busy={busy || loading}
                submissionError={controller.error}
                onChange={setDraft}
                onSave={() => void save()}
                onCancel={() => guard(() => setDraft(null))}
                onPick={pickSkill}
                inventory={inventory}
                onReload={loadLatest}
                saving={[
                  "save_mcp",
                  "save_skill",
                  "create_skill",
                  "install_skill",
                ].includes(controller.busyAction)}
              />
              {loading && (
                <p role="status" className="text-xs text-muted">
                  正在重新读取当前范围，草稿已保留…
                </p>
              )}
              {controller.uncertain && (
                <Button
                  size="sm"
                  variant="outline"
                  disabled={busy || loading}
                  onClick={() => void controller.refresh()}
                >
                  刷新确认上次操作结果
                </Button>
              )}
              {comparison && (
                <section
                  className="mt-4 space-y-3 rounded-lg border border-default p-4"
                  aria-label="与最新配置比对"
                >
                  <h4 className="m-0 text-sm">最新保存内容</h4>
                  <p className="text-xs text-muted">
                    上方保留了你的草稿。请对照下方最新内容手动合并，确认后再保存。
                  </p>
                  <textarea
                    aria-label="最新保存内容"
                    readOnly
                    value={comparison.content}
                    className="min-h-40 w-full rounded-lg border border-default bg-transparent p-3 font-mono text-xs"
                  />
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => {
                      setDraft({
                        ...draft,
                        revision: comparison.revision,
                        original: comparison.content,
                      });
                      setComparison(null);
                    }}
                  >
                    已比对，使用最新版本作为保存基础
                  </Button>
                </section>
              )}
            </div>
          </DialogContent>
        </Dialog>
      )}
      {confirmation && (
        <ConfirmationDialog
          {...confirmation}
          busy={busy}
          container={container}
          onCancel={() => setConfirmation(null)}
          onConfirm={() => {
            const current = confirmation;
            void (
              current.action.action === "test_mcp"
                ? controller.inspect(current.action)
                : controller.mutate(current.action)
            ).then((outcome) => {
              setConfirmation(null);
              if (current.action.action === "save_mcp") settleSave(outcome);
            });
          }}
        />
      )}
      {discard && (
        <ConfirmationDialog
          title="放弃未保存的修改？"
          description="当前草稿尚未保存，继续操作会丢弃这些修改。"
          destructive
          busy={false}
          container={container}
          onCancel={() => setDiscard(null)}
          onConfirm={() => {
            const next = discard;
            setDiscard(null);
            next();
          }}
        />
      )}
    </section>
  );
}
