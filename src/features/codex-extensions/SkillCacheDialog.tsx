import { useEffect, useMemo, useRef, useState } from "react";
import { Button, Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "../../components/ui";
import { ConfirmationDialog } from "./ConfirmationDialog";
import { ExtensionError } from "./ExtensionError";
import { getSkillCacheRequests } from "./requests";
import { causeText } from "./state";
import type { ExtensionTransport, SkillCacheInventory, SkillEntry } from "./types";

export function SkillCacheDialog({ open, request, container, onClose }: {
  open: boolean;
  request: ExtensionTransport;
  container?: HTMLElement | null;
  onClose: () => void;
}) {
  const session = useMemo(() => getSkillCacheRequests(request), [request]);
  const generation = useRef(0);
  const running = useRef(false);
  const [cache, setCache] = useState<SkillCacheInventory | null>(null);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [blocked, setBlocked] = useState(true);
  const [confirmation, setConfirmation] = useState<SkillEntry | null>(null);
  const [document, setDocument] = useState<{ name: string; content: string } | null>(null);

  async function run<T>(operation: string, task: () => Promise<T>, apply: (result: T) => void) {
    if (running.current) return;
    running.current = true;
    const current = ++generation.current;
    setBusy(operation);
    setError("");
    try {
      const result = await task();
      if (current === generation.current) apply(result);
    } catch (cause) {
      if (current === generation.current) setError(causeText(cause));
    } finally {
      if (current === generation.current) {
        running.current = false;
        setBusy("");
        setBlocked(session.blocked);
      }
    }
  }
  const refresh = () => {
    setConfirmation(null);
    setDocument(null);
    setNotice("");
    void run("list", () => session.list(), setCache);
  };
  useEffect(() => {
    if (open) {
      setCache(null);
      setBlocked(true);
      refresh();
    }
    return () => {
      generation.current++;
      running.current = false;
    };
  }, [open, session]);

  if (!open) return null;
  const unavailable = !!busy || blocked || !cache;
  return (
    <>
      <Dialog open onOpenChange={(value) => { if (!value && busy !== "remove" && !confirmation) onClose(); }}>
        <DialogContent container={container} className="sm:w-[760px]" onEscapeKeyDown={(event) => { if (busy === "remove" || confirmation) event.preventDefault(); }}>
          <DialogHeader>
            <DialogTitle>缓存 Skill 管理</DialogTitle>
            <DialogDescription>查看当前用户的全局插件 Skill 缓存，不受项目范围影响。删除可能影响插件后续加载，插件更新或重新安装时可能再次生成缓存。</DialogDescription>
          </DialogHeader>
          <div className="max-h-[70vh] space-y-3 overflow-y-auto px-6 pb-6">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs text-muted">全局用户缓存{cache ? ` · 共 ${cache.skills.length} 项` : ""}</span>
              <Button size="sm" variant="outline" disabled={!!busy} onClick={refresh}>刷新缓存</Button>
            </div>
            {error && <ExtensionError message={error} />}
            {blocked && cache && <p role="status" className="text-xs text-warning">请刷新缓存确认当前状态，再继续删除。</p>}
            {notice && <p role="status" className="text-xs text-muted">{notice}</p>}
            {cache?.warnings.map((warning, index) => <p key={index} role="status" className="break-words text-xs text-warning">{warning}</p>)}
            {busy && <p role="status" className="text-sm text-muted">{busy === "list" ? "正在读取缓存…" : busy === "remove" ? "正在删除缓存…" : "正在读取 SKILL.md…"}</p>}
            {cache?.skills.length === 0 && <p className="py-8 text-center text-sm text-muted">暂无缓存 Skill。</p>}
            {cache?.skills.map((entry) => (
              <article key={entry.id} className="codey-card space-y-2 p-4">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0"><h3 className="m-0 break-words text-sm font-semibold">{entry.name}</h3><span className="text-xs text-muted">版本：{entry.version || "未提供"}</span></div>
                  <div className="flex gap-2">
                    <Button size="sm" variant="outline" disabled={!!busy} onClick={() => { setDocument(null); void run("read", () => session.read(entry.id), (value) => setDocument({ name: entry.name, content: value.content })); }}>查看</Button>
                    <Button size="sm" variant="destructive" disabled={unavailable || entry.canRemove === false} onClick={() => setConfirmation(entry)}>删除</Button>
                  </div>
                </div>
                <p className="m-0 break-words text-xs text-muted">{entry.description || "暂无描述"}</p>
                <p className="m-0 break-all text-xs text-muted">来源：{entry.sourcePath}</p>
                {entry.canRemove === false && entry.reason && <p className="m-0 text-xs text-warning">{entry.reason}</p>}
              </article>
            ))}
            {document && <section className="space-y-2" aria-label="缓存 Skill 内容"><h3 className="text-sm font-semibold">{document.name} · SKILL.md（只读）</h3><textarea aria-label="缓存 SKILL.md 内容" readOnly value={document.content} className="min-h-64 w-full rounded-lg border border-default bg-transparent p-3 font-mono text-xs" /></section>}
          </div>
        </DialogContent>
      </Dialog>
      {confirmation && <ConfirmationDialog container={container} title={`删除缓存 Skill：${confirmation.name}`} description={`将删除当前用户全局缓存中的 ${confirmation.name}${confirmation.version ? `（${confirmation.version}）` : ""} 目录及其数据，不受当前项目范围影响。此操作不能从本页恢复，可能影响对应插件的后续加载。来源：${confirmation.sourcePath}`} destructive busy={busy === "remove"} onCancel={() => setConfirmation(null)} onConfirm={() => {
        if (unavailable || !cache) return;
        void run("remove", () => session.remove(confirmation.id, cache.revision), (result) => { setCache(result.cache); setDocument(null); setNotice(result.message); }).then(() => setConfirmation(null));
      }} />}
    </>
  );
}
