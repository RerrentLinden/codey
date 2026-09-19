import { useEffect, useRef, useState } from "react";
import { Button } from "./components/ui";
import { validatePluginConfig, type PluginSchema } from "./codeyPlugins";
import { pluginHtmlDocument, pluginJsonObject, validPluginReady } from "./pluginHtmlBridge";

export function PluginHtmlConfig({ html, value, schema, disabled, onSave, onFailure, onDraftChange }: {
  html: string; value: Record<string, unknown>; schema: PluginSchema; disabled: boolean;
  onSave: (config: Record<string, unknown>) => void; onFailure: (message: string) => void;
  onDraftChange: (config: Record<string, unknown>, invalidDraft?: boolean) => void;
}) {
  const iframe = useRef<HTMLIFrameElement>(null);
  const [document] = useState(() => { const token = crypto.randomUUID(); return { token, html: pluginHtmlDocument(html, token) }; });
  const [draft, setDraft] = useState(value);
  const [ready, setReady] = useState(false);
  const [validity, setValidity] = useState({ valid: true, message: "" });
  const [payloadError, setPayloadError] = useState("");
  const initialValue = useRef(value);
  const currentDraft = useRef(value);
  const loads = useRef(0), invalidated = useRef(false);
  const portRef = useRef<MessagePort | null>(null);
  const latest = useRef({ disabled, onFailure, onDraftChange }); latest.current = { disabled, onFailure, onDraftChange };
  function invalidate(message: string) { invalidated.current = true; setReady(false); portRef.current?.close(); latest.current.onFailure(message); }
  useEffect(() => {
    let stopped = false, bound = false;
    const attemptId = crypto.randomUUID();
    setReady(false);
    const connect = () => {
      if (stopped || bound || invalidated.current) return;
      iframe.current?.contentWindow?.postMessage({ type: "codey-plugin-connect", token: document.token, attemptId }, "*");
    };
    const retry = window.setInterval(connect, 250);
    const timer = window.setTimeout(() => {
      window.clearInterval(retry);
      if (!bound && !stopped) invalidate("插件配置页面未能建立连接，请使用自动表单或重新打开配置。");
    }, 10000);
    function receive(event: MessageEvent) {
      if (stopped || invalidated.current || !validPluginReady(event, iframe.current?.contentWindow, document.token, bound, attemptId)) return;
      bound = true; window.clearTimeout(timer); window.clearInterval(retry);
      const port = event.ports[0]; portRef.current = port;
      port.onmessage = message => {
        if (stopped || invalidated.current) return;
        const data = message.data;
        if (data?.type === "unload") { invalidate("插件配置页面已离开，连接已关闭。请重新打开配置。"); return; }
        if (latest.current.disabled) return;
        if (data?.type === "config") {
          const next = pluginJsonObject(data.config);
          if (next) { currentDraft.current = next; setDraft(next); latest.current.onDraftChange(next); setPayloadError(""); }
          else { latest.current.onDraftChange(currentDraft.current, true); setPayloadError("插件返回的配置必须为不超过 1 MiB 的 JSON 对象。"); }
        } else if (data?.type === "validity" && typeof data.valid === "boolean") {
          setValidity({ valid: data.valid, message: typeof data.message === "string" ? data.message.slice(0, 2000) : "" });
        }
      };
      port.start();
      port.postMessage({ type: "init", config: initialValue.current, theme: window.document.documentElement.classList.contains("dark") ? "dark" : "light" });
      setReady(true);
    }
    window.addEventListener("message", receive);
    connect();
    return () => { stopped = true; window.clearTimeout(timer); window.clearInterval(retry); window.removeEventListener("message", receive); portRef.current?.close(); portRef.current = null; };
  }, [document]);
  const errors = validatePluginConfig(draft, schema);
  return <div className="grid gap-3">
    <iframe ref={iframe} title="插件自定义配置" sandbox="allow-scripts" inert={disabled} referrerPolicy="no-referrer" srcDoc={document.html}
      onLoad={() => { if (++loads.current > 1) invalidate("插件配置页面发生跳转，连接已关闭。请重新打开配置。"); }}
      className="h-[360px] w-full rounded-xl border border-black/10 bg-white dark:border-white/15" />
    {!ready && <p className="m-0 text-sm text-muted-foreground">正在连接插件配置页面…</p>}
    {(payloadError || !validity.valid || errors.length > 0) && <p role="alert" className="m-0 break-words text-sm text-red-600">{payloadError || (!validity.valid ? validity.message || "请检查插件配置" : errors[0])}</p>}
    <div className="flex justify-end"><Button variant="default" disabled={disabled || !ready || Boolean(payloadError) || !validity.valid || errors.length > 0} onClick={() => onSave(draft)}>{disabled ? "保存中…" : "保存配置"}</Button></div>
  </div>;
}
