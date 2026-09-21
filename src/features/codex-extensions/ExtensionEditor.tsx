import { Button, Input } from "../../components/ui";
import { useState } from "react";
import {
  IconAlertCircle,
  IconCheck,
  IconDownload,
  IconFileCode,
  IconFolderOpen,
  IconInfoCircle,
  IconShieldCheck,
} from "@tabler/icons-react";
import type { EditorDraft, Inventory } from "./types";
import { editorError, draftChanged } from "./state";
import { ExtensionError } from "./ExtensionError";
import { ResourceInfo } from "./ResourceInfo";
import { McpJsonImport } from "./McpJsonImport";
import { mcpJsonError } from "./mcpJson";

export function ExtensionEditor({
  draft,
  busy,
  submissionError,
  onChange,
  onSave,
  onCancel,
  onPick,
  inventory,
  onReload,
  saving,
}: {
  draft: EditorDraft;
  busy: boolean;
  submissionError?: string;
  onChange: (draft: EditorDraft) => void;
  onSave: () => void;
  onCancel: () => void;
  onPick: () => void;
  inventory?: Inventory | null;
  onReload?: () => void;
  saving?: boolean;
}) {
  const error =
    draft.kind === "mcp" &&
    draft.isNew &&
    inventory?.mcps.some((entry) => entry.id === draft.id)
      ? "此服务标识已存在，请使用其他标识；如需修改，请从列表打开原有服务。"
      : mcpJsonError(draft) || editorError(draft);
  const [jsonReading, setJsonReading] = useState(false);
  const entry = draft.entry;
  const lineCount = (draft.content.match(/\n/g)?.length ?? 0) + 1;

  return (
    <div className="space-y-4">
      {entry && <ResourceInfo entry={entry} inventory={inventory} />}

      {draft.kind === "install" && (
        <div className="flex items-start gap-2.5 rounded-xl border border-blue-500/15 bg-blue-500/8 p-3 text-xs leading-relaxed text-blue-950 dark:border-blue-400/20 dark:bg-blue-500/10 dark:text-blue-200">
          <IconShieldCheck size={16} className="mt-0.5 shrink-0 text-blue-600 dark:text-blue-400" />
          <div>
            从本地目录或 ZIP 复制安装，保留来源且不执行外部脚本。新安装默认处于禁用状态，确认无误后可在列表中随时启用。
          </div>
        </div>
      )}

      {draft.kind === "mcp" && (
        <div className="flex items-start gap-2.5 rounded-xl border border-black/[0.06] bg-black/[0.02] p-3 text-xs leading-relaxed text-muted dark:border-white/[0.08] dark:bg-white/[0.03]">
          <IconInfoCircle size={16} className="mt-0.5 shrink-0 text-blue-600 dark:text-blue-400" />
          <div className="space-y-1">
            <div>
              {draft.isNew
                ? "粘贴 JSON 或导入 .json 文件，支持 mcpServers、mcp_servers 和单个服务配置。新服务保存后自动启用并刷新 Codex，无需重启；请仅保存可信服务。"
                : "编辑 JSON 或导入文件替换草稿，服务标识保持不变。保留脱敏占位符即可沿用原值；保存后自动刷新 Codex，无需重启。"}
            </div>
            {!draft.isNew && (
              <p className="mb-0 mt-1 text-[11px] text-muted">
                导出会隐藏凭证和未知字段。迁移到其他环境时，请补齐这些配置；导入只填充草稿。
              </p>
            )}
          </div>
        </div>
      )}

      {draft.kind === "mcp" && (
        <label className="block space-y-1.5 text-xs font-medium text-foreground">
          <span>服务标识</span>
          <Input
            aria-label="服务标识"
            value={draft.id}
            disabled={busy || jsonReading || !draft.isNew}
            onChange={(event) => onChange({ ...draft, id: event.target.value })}
          />
        </label>
      )}

      {draft.kind === "mcp" ? (
        <McpJsonImport
          draft={draft}
          busy={busy}
          onChange={onChange}
          onReadingChange={setJsonReading}
        />
      ) : draft.kind === "install" ? (
        <div className="space-y-2">
          <label className="block text-xs font-medium text-foreground">
            Skill 来源路径
          </label>
          <div className="flex gap-2">
            <Input
              aria-label="Skill 来源绝对路径"
              placeholder="输入或粘贴目录或 .zip 文件的绝对路径"
              value={draft.content}
              disabled={busy}
              leftSection={<IconFolderOpen size={16} className="text-muted" />}
              onChange={(event) =>
                onChange({ ...draft, content: event.target.value })
              }
              className="flex-1 text-xs"
            />
            <Button
              variant="outline"
              disabled={busy}
              onClick={onPick}
              className="shrink-0 gap-1.5"
            >
              <IconFolderOpen size={15} />
              <span>选择目录</span>
            </Button>
          </div>
          {!draft.content.trim() ? (
            <p className="m-0 text-[11px] text-muted">
              支持包含 SKILL.md 的本地目录，或打包好的 .zip 归档文件。
            </p>
          ) : null}
        </div>
      ) : (
        <div className="space-y-2">
          <div className="overflow-hidden rounded-xl border border-black/[0.12] bg-[var(--codey-surface,#fff)] shadow-2xs transition-colors focus-within:border-accent focus-within:ring-2 focus-within:ring-accent/20 dark:border-white/[0.12] dark:bg-black/20">
            <div className="flex items-center justify-between border-b border-black/[0.08] bg-black/[0.02] px-3.5 py-2 dark:border-white/[0.08] dark:bg-white/[0.03]">
              <div className="flex items-center gap-2">
                <IconFileCode size={16} className="text-blue-600 dark:text-blue-400" />
                <span className="font-mono text-xs font-semibold text-foreground">
                  SKILL.md
                </span>
                <span className="rounded bg-black/[0.05] px-1.5 py-0.5 font-mono text-[10px] text-muted dark:bg-white/[0.08]">
                  Markdown / YAML
                </span>
                {draft.readOnly && (
                  <span className="rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] font-medium text-amber-600 dark:text-amber-400">
                    只读
                  </span>
                )}
              </div>
              <div className="flex items-center gap-2 font-mono text-[11px] text-muted">
                <span>{lineCount} 行</span>
                <span>·</span>
                <span>{draft.content.length} 字符</span>
              </div>
            </div>
            <textarea
              aria-label="SKILL.md"
              className="min-h-[300px] w-full resize-y bg-transparent p-3.5 font-mono text-xs leading-relaxed text-foreground outline-none"
              spellCheck={false}
              value={draft.content}
              disabled={busy}
              readOnly={draft.readOnly}
              onChange={(event) =>
                onChange({ ...draft, content: event.target.value })
              }
              placeholder="编写 SKILL.md 内容..."
            />
          </div>
          {!draft.readOnly ? (
            <p className="m-0 text-[11px] text-muted">
              头部必须包含 <code className="rounded bg-black/[0.05] px-1 py-0.5 font-mono text-[10px] text-foreground dark:bg-white/[0.08]">name</code> 与 <code className="rounded bg-black/[0.05] px-1 py-0.5 font-mono text-[10px] text-foreground dark:bg-white/[0.08]">description</code> 声明。仅 Codey 托管的 Skill 可编辑。
            </p>
          ) : null}
        </div>
      )}

      {error && !draft.readOnly && (draft.kind !== "install" || !!draft.content.trim()) && (
        <div
          role="status"
          className="flex items-center gap-2 rounded-xl border border-warning/30 bg-warning/10 px-3.5 py-2.5 text-xs text-warning"
        >
          <IconAlertCircle size={16} className="shrink-0" />
          <span>{error}</span>
        </div>
      )}

      {submissionError && (
        <ExtensionError message={submissionError} draftPreserved />
      )}

      {!draft.readOnly &&
        ((!draft.isNew && draft.kind !== "install") || !!submissionError) && (
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={onReload}
          >
            {draft.isNew || draft.kind === "install"
              ? "重新读取范围，保留新建草稿"
              : "加载最新内容，与保留的草稿比对"}
          </Button>
        )}

      <div className="mt-5 flex items-center justify-end gap-2.5 border-t border-black/[0.06] pt-4 dark:border-white/[0.08]">
        <Button variant="outline" disabled={busy} onClick={onCancel}>
          {draft.readOnly ? "返回列表" : "取消"}
        </Button>
        {!draft.readOnly && (
          <Button
            loading={saving}
            disabled={
              busy ||
              jsonReading ||
              !!error ||
              (draft.kind !== "mcp" && !draft.isNew && !draftChanged(draft))
            }
            onClick={onSave}
            className="gap-1.5"
          >
            {draft.kind === "install" ? (
              <>
                <IconDownload size={15} />
                <span>检查并安装</span>
              </>
            ) : (
              <>
                <IconCheck size={15} />
                <span>校验并保存</span>
              </>
            )}
          </Button>
        )}
      </div>
    </div>
  );
}
