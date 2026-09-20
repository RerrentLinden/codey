import { useEffect, useRef, useState } from "react";
import { Select } from "../../components/ui";
import type { EditorDraft } from "./types";
import { parseMcpJson, updateMcpJsonDraft } from "./mcpJson";
import { ExtensionError } from "./ExtensionError";

export function McpJsonImport({
  draft,
  busy,
  onChange,
  onReadingChange,
}: {
  draft: EditorDraft;
  busy: boolean;
  onChange: (draft: EditorDraft) => void;
  onReadingChange: (reading: boolean) => void;
}) {
  const sequence = useRef(0);
  const current = useRef(draft);
  current.current = draft;
  const [error, setError] = useState("");
  const [importedFile, setImportedFile] = useState("");
  const [reading, setLocalReading] = useState(false);
  const setReading = (value: boolean) => {
    setLocalReading(value);
    onReadingChange(value);
  };
  useEffect(
    () => () => {
      sequence.current++;
    },
    [],
  );
  let services: ReturnType<typeof parseMcpJson> = [];
  try {
    services = parseMcpJson(draft.content);
  } catch {
    /* 主编辑器展示校验提示。 */
  }
  return (
    <div className="space-y-3">
      {!draft.readOnly && (
        <label className="block space-y-1 text-xs">
          <span>从本地 JSON 导入草稿</span>
          <input
            aria-label="导入 JSON 文件"
            type="file"
            accept=".json,application/json"
            disabled={busy}
            className="block w-full text-xs"
            onChange={async (event) => {
              const file = event.target.files?.[0];
              event.target.value = "";
              if (!file) return;
              const request = ++sequence.current;
              setError("");
              setImportedFile("");
              setReading(true);
              try {
                if (!file.name.toLowerCase().endsWith(".json"))
                  throw new Error("请选择 .json 文件。");
                if (file.size > 1024 * 1024)
                  throw new Error("JSON 文件不能超过 1 MB。");
                const content = await file.text();
                if (request !== sequence.current) return;
                parseMcpJson(content);
                onChange(updateMcpJsonDraft(current.current, content));
                setImportedFile(file.name);
              } catch (cause) {
                if (request === sequence.current)
                  setError(
                    cause instanceof Error
                      ? cause.message
                      : "文件读取失败，请重试。",
                  );
              } finally {
                if (request === sequence.current) setReading(false);
              }
            }}
          />
        </label>
      )}
      {importedFile && (
        <p role="status" className="text-xs text-muted">
          文件 {importedFile} 已载入草稿，请检查配置后保存。
        </p>
      )}
      {reading && (
        <p role="status" className="text-xs text-muted">
          正在读取 JSON 文件…
        </p>
      )}
      {error && <ExtensionError message={error} draftPreserved />}
      <label className="block space-y-1 text-xs">
        <span>服务配置 JSON</span>
        <textarea
          aria-label="服务配置 JSON"
          spellCheck={false}
          readOnly={draft.readOnly}
          disabled={busy}
          className="min-h-64 w-full resize-y rounded-lg border border-default bg-transparent p-3 font-mono text-xs leading-6 outline-none focus:border-accent"
          value={draft.content}
          onChange={(event) => {
            sequence.current++;
            setReading(false);
            setError("");
            onChange(updateMcpJsonDraft(draft, event.target.value));
          }}
        />
      </label>
      {services.length > 1 && (
        <label className="block space-y-1 text-xs">
          <span>本次导入的服务（共 {services.length} 个，一次保存一个）</span>
          <Select
            aria-label="选择导入服务"
            value={draft.jsonService ?? ""}
            disabled={busy || draft.readOnly}
            placeholder="请选择一个服务"
            className="w-full"
            optionList={services.map((service) => ({
              value: service.name,
              label: service.name,
            }))}
            onChange={(value) => {
              sequence.current++;
              setReading(false);
              const name = String(value ?? "");
              onChange({
                ...draft,
                jsonService: name,
                id:
                  !draft.isNew || name === draft.jsonService ? draft.id : name,
              });
            }}
          />
        </label>
      )}
    </div>
  );
}
