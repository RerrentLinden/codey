import { Button, Input } from "../../components/ui";
import { useState } from "react";
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
  return (
    <div className="space-y-4">
      {entry && <ResourceInfo entry={entry} inventory={inventory} />}
      <div>
        <p className="m-0 text-xs text-muted">
          {draft.kind === "mcp"
            ? draft.isNew
              ? "粘贴 JSON 或导入 .json 文件，支持 mcpServers、mcp_servers 和单个服务配置。导入只填充草稿；新服务保存后默认禁用。"
              : "编辑 JSON 或导入文件替换草稿，服务标识保持不变。保留脱敏占位符即可沿用原值，保存前会校验配置。"
            : draft.kind === "install"
              ? "从本地目录或 ZIP 复制安装，保留来源，不执行脚本。新安装默认禁用。"
              : "仅 Codey 托管的 Skill 可编辑。保存后将更新本地文件。"}
        </p>
        {draft.kind === "mcp" && !draft.isNew && (
          <p className="mb-0 mt-2 text-xs text-muted">
            导出会隐藏凭证和未知字段。迁移到其他环境时，请补齐这些配置；导入只填充草稿。
          </p>
        )}
      </div>
      {draft.kind === "mcp" && (
        <label className="block space-y-1 text-xs">
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
        <div className="flex flex-col gap-2 sm:flex-row">
          <Input
            aria-label="Skill 来源绝对路径"
            placeholder="目录或 .zip 文件的绝对路径"
            value={draft.content}
            disabled={busy}
            onChange={(event) =>
              onChange({ ...draft, content: event.target.value })
            }
          />
          <Button variant="outline" disabled={busy} onClick={onPick}>
            选择目录
          </Button>
        </div>
      ) : (
        <label className="block space-y-1 text-xs">
          <span>SKILL.md</span>
          <textarea
            aria-label="SKILL.md"
            className="min-h-64 w-full resize-y rounded-lg border border-default bg-transparent p-3 font-mono text-xs leading-6 outline-none focus:border-accent"
            spellCheck={false}
            value={draft.content}
            disabled={busy}
            readOnly={draft.readOnly}
            onChange={(event) =>
              onChange({ ...draft, content: event.target.value })
            }
          />
        </label>
      )}
      {error && !draft.readOnly && (
        <p role="status" className="text-xs text-warning">
          {error}
        </p>
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
      <div className="flex justify-end gap-2">
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
              (!draft.isNew && !draftChanged(draft))
            }
            onClick={onSave}
          >
            {draft.kind === "install" ? "检查并安装" : "校验并保存"}
          </Button>
        )}
      </div>
    </div>
  );
}
