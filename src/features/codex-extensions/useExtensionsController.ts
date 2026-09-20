import { useCallback, useEffect, useRef, useState } from "react";
import type {
  CheckResult,
  ExtensionTransport,
  Inventory,
  MutationResult,
  Scope,
} from "./types";
import { causeText } from "./state";
import { readInventory, invalidateInventory, withTimeout } from "./requests";

export function useExtensionsController(
  request: ExtensionTransport,
  active: boolean,
) {
  const [scope, setScope] = useState<Scope>({ kind: "user" });
  const [inventory, setInventory] = useState<Inventory | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [busyAction, setBusyAction] = useState("");
  const [uncertain, setUncertain] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [check, setCheck] = useState<
    (CheckResult & { id: string; revision?: string }) | null
  >(null);
  const [checks, setChecks] = useState<
    Record<string, CheckResult & { revision?: string }>
  >({});
  const epoch = useRef(0);
  const locked = useRef<object | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      epoch.current++;
    };
  }, []);
  const refresh = useCallback(
    async (force = true) => {
      const generation = ++epoch.current;
      setLoading(true);
      setError("");
      try {
        const result = await readInventory(request, scope, force);
        if (mounted.current && generation === epoch.current) {
          setInventory(result);
          setUncertain(false);
          return result;
        }
        return null;
      } catch (cause) {
        if (mounted.current && generation === epoch.current) {
          setError(causeText(cause));
        }
        return null;
      } finally {
        if (mounted.current && generation === epoch.current) setLoading(false);
      }
    },
    [request, scope],
  );
  useEffect(() => {
    setInventory(null);
    setCheck(null);
    setNotice("");
    setChecks({});
    setBusy(false);
    setBusyAction("");
    setUncertain(false);
    locked.current = null;
    if (active) void refresh(false);
    return () => {
      epoch.current++;
    };
  }, [active, refresh]);
  const run = useCallback(
    async <T>(
      action: Record<string, unknown>,
      onSuccess?: (result: T) => void,
    ): Promise<boolean> => {
      if (locked.current) return false;
      const token = {};
      locked.current = token;
      setBusy(true);
      setBusyAction(String(action.action));
      setError("");
      const generation = ++epoch.current;
      const mutation = ![
        "get_mcp",
        "read_skill",
        "export_skill",
        "pick_project",
        "pick_skill",
        "test_mcp",
        "validate_skill",
      ].includes(String(action.action));
      if (mutation && uncertain) {
        setError("上次操作结果尚不确定，请先刷新确认后再提交修改。");
        locked.current = null;
        setBusy(false);
        setBusyAction("");
        return false;
      }
      if (mutation) {
        // 项目 Skill 的启停也写入用户配置，所有范围的清单均需失效。
        invalidateInventory(request);
      }
      setLoading(false);
      try {
        const result = await withTimeout(
          request<T>({
            scope,
            revision: inventory?.revision,
            ...action,
          }),
          mutation,
          action.action === "test_mcp"
            ? 35000
            : String(action.action).startsWith("pick_")
              ? 120000
              : mutation
                ? 30000
                : 15000,
        );
        if (mounted.current && generation === epoch.current)
          onSuccess?.(result);
        return mounted.current && generation === epoch.current;
      } catch (cause) {
        const message = causeText(cause);
        if (mounted.current && generation === epoch.current) {
          setError(message);
          if (mutation && message.includes("结果尚不确定")) setUncertain(true);
        }
        return false;
      } finally {
        if (mutation) invalidateInventory(request);
        if (locked.current === token) {
          locked.current = null;
          if (mounted.current) {
            setBusy(false);
            setBusyAction("");
          }
        }
      }
    },
    [request, scope, inventory?.revision, uncertain],
  );
  const mutate = useCallback(
    (action: Record<string, unknown>) =>
      run<MutationResult>(action, (result) => {
        setInventory(result.inventory);
        setNotice(
          `${result.message}${result.applyStatus === "restart-required" ? " 配置已保存，请重启 Codex 后确认生效；运行时覆盖可能影响最终状态。" : ""}`,
        );
      }),
    [run],
  );
  const inspect = useCallback(
    (action: Record<string, unknown>) =>
      run<CheckResult>(action, (result) => {
        const id = String(action.id),
          revision = inventory?.revision;
        const checkedAt = new Date().toISOString();
        setCheck({ ...result, id, revision, checkedAt });
        setChecks((current) => ({
          ...current,
          [id]: { ...result, revision, checkedAt },
        }));
      }),
    [run, inventory?.revision],
  );
  return {
    scope,
    setScope,
    inventory,
    loading,
    busy,
    busyAction,
    uncertain,
    clearError: () => setError(""),
    error,
    notice,
    check,
    checks,
    refresh,
    run,
    mutate,
    inspect,
  };
}
