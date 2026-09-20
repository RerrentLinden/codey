import {
  IconFolder,
  IconFolderCode,
  IconSearch,
  IconUser,
} from "@tabler/icons-react";
import { Button, Input, Select } from "../../components/ui";
import type { Inventory, Scope } from "./types";

export function ExtensionScope({
  kind,
  scope,
  inventory,
  count,
  projectMode,
  projectPath,
  busy,
  onMode,
  onPath,
  onPick,
  onRead,
}: {
  kind: "mcp" | "skill";
  scope: Scope;
  inventory: Inventory | null;
  count: number;
  projectMode: boolean;
  projectPath: string;
  busy: boolean;
  onMode: (project: boolean) => void;
  onPath: (path: string) => void;
  onPick: () => void;
  onRead: () => void;
}) {
  const validPath = /^(\/|[A-Za-z]:[\\/]|\\\\)/.test(projectPath.trim());
  return (
    <div className="codey-card p-4 space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-wrap items-center gap-3">
          <span className="text-xs font-semibold text-muted">生效范围</span>
          <div
            className="codey-segmented-control"
            role="group"
            aria-label="配置范围"
          >
            <button
              type="button"
              aria-pressed={!projectMode}
              className={`codey-segmented-item ${!projectMode ? "active" : ""}`}
              disabled={busy}
              onClick={() => onMode(false)}
            >
              <IconUser size={13} className="inline-block mr-1" />
              当前用户
            </button>
            <button
              type="button"
              aria-pressed={projectMode}
              className={`codey-segmented-item ${projectMode ? "active" : ""}`}
              disabled={busy}
              onClick={() => onMode(true)}
            >
              <IconFolderCode size={13} className="inline-block mr-1" />
              指定项目
            </button>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <span
            className={`codey-badge ${scope.kind === "project" ? "codey-badge-info" : "codey-badge-neutral"}`}
          >
            {scope.kind === "project" ? "项目专属" : "全局用户"}
          </span>
          {inventory && (
            <span className="codey-badge codey-badge-neutral">
              共 {count} 项
            </span>
          )}
        </div>
      </div>
      {projectMode && (
        <>
          <div className="flex flex-wrap items-center gap-2 pt-1 border-t border-[var(--color-border-subtle)]">
            <div className="min-w-0 basis-full sm:flex-1 sm:basis-auto">
              <Input
                aria-label="项目绝对路径"
                className="w-full text-xs font-mono"
                placeholder="项目绝对路径"
                disabled={busy}
                value={projectPath}
                onChange={(event) => onPath(event.target.value)}
              />
            </div>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={onPick}
            >
              <IconFolder size={14} className="mr-1 inline-block" />
              选择项目
            </Button>
            <Button
              size="sm"
              variant={scope.kind === "project" ? "default" : "outline"}
              disabled={busy || !validPath}
              onClick={onRead}
            >
              读取项目
            </Button>
          </div>
          {!validPath && (
            <p className="text-xs text-muted">
              请输入或选择项目绝对路径，再读取该项目的配置。
            </p>
          )}
        </>
      )}
      {inventory && (
        <div className="rounded-lg bg-[var(--color-bg-tertiary)] p-3 text-xs text-muted border border-[var(--color-border-subtle)] space-y-1">
          <p className="m-0 break-all font-medium text-[var(--color-text-primary)]">
            当前范围：{scope.kind === "user" ? "当前用户" : scope.projectPath} ·
            共 {count} 项
          </p>
          <p className="mb-0 mt-1 break-all">
            配置位置：
            {kind === "skill"
              ? inventory.skillConfigPath || inventory.configPath
              : inventory.configPath}
          </p>
          <p className="mb-0 mt-1">
            {inventory.applyNotice ||
              "配置变更保存后需重启 Codex，再确认当前会话的有效状态。"}
          </p>
        </div>
      )}
    </div>
  );
}

export function ExtensionFilters({
  kind,
  query,
  filter,
  source,
  sort,
  onQuery,
  onFilter,
  onSource,
  onSort,
  onClear,
}: {
  kind: "mcp" | "skill";
  query: string;
  filter: string;
  source: string;
  sort: string;
  onQuery: (value: string) => void;
  onFilter: (value: string) => void;
  onSource: (value: string) => void;
  onSort: (value: string) => void;
  onClear: () => void;
}) {
  return (
    <div className="flex flex-wrap gap-2">
      <Input
        type="search"
        aria-label={`搜索 ${kind === "mcp" ? "MCP" : "Skill"}`}
        className="min-w-40 flex-1 text-xs"
        placeholder="搜索名称、描述或路径"
        value={query}
        onChange={(event) => onQuery(event.target.value)}
        leftSection={<IconSearch size={14} className="text-muted" />}
      />
      <Select
        aria-label="状态筛选"
        className="w-32 text-xs"
        value={filter}
        onChange={(value) => onFilter(String(value))}
        optionList={[
          { value: "all", label: "全部状态" },
          { value: "enabled", label: "已启用" },
          { value: "disabled", label: "已禁用" },
          { value: "readonly", label: "只读" },
          { value: "unknown", label: "状态待确认" },
          { value: "invalid", label: "配置无效" },
        ]}
      />
      <Select
        aria-label="来源筛选"
        className="w-32 text-xs"
        value={source}
        onChange={(value) => onSource(String(value))}
        optionList={
          kind === "mcp"
            ? [
                { value: "all", label: "全部连接类型" },
                { value: "stdio", label: "本地进程" },
                { value: "http", label: "HTTP" },
                { value: "unknown", label: "未知类型" },
              ]
            : [
                { value: "all", label: "全部来源" },
                { value: "managed", label: "Codey 托管" },
                { value: "external", label: "外部安装" },
                { value: "builtin", label: "系统内置" },
              ]
        }
      />
      <Select
        aria-label="排序"
        className="w-32 text-xs"
        value={sort}
        onChange={(value) => onSort(String(value))}
        optionList={[
          { value: "name", label: "名称排序" },
          { value: "updated", label: "最近更新" },
          { value: "enabled", label: "启用优先" },
        ]}
      />
      {(query.trim() || filter !== "all" || source !== "all") && (
        <Button size="sm" variant="outline" onClick={onClear}>
          清除筛选
        </Button>
      )}
    </div>
  );
}
