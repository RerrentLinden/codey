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

/**
 * run 的结果：ok 提交成功且本地状态同步；superseded 后端已落库但结果被抢占；
 * failed 未提交或提交失败，调用方应保留草稿。
 */
export type RunOutcome = "ok" | "superseded" | "failed";

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
  // 读写各用一条序号：run 抢占刷新不再作废在飞的清单读取，刷新也不会作废在飞的提交。
  const readEpoch = useRef(0);
  const writeEpoch = useRef(0);
  const locked = useRef<object | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      readEpoch.current++;
      writeEpoch.current++;
    };
  }, []);
  const refresh = useCallback(
    async (force = true) => {
      const generation = ++readEpoch.current;
      setLoading(true);
      setError("");
      try {
        const result = await readInventory(request, scope, force);
        if (mounted.current && generation === readEpoch.current) {
          setInventory(result);
          setUncertain(false);
          return result;
        }
        return null;
      } catch (cause) {
        if (mounted.current && generation === readEpoch.current) {
          setError(causeText(cause));
        }
        return null;
      } finally {
        if (mounted.current && generation === readEpoch.current) setLoading(false);
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
    // 序号失效后旧请求不会再清 loading，这里显式收尾，避免切走范围时卡在加载态。
    setLoading(false);
    locked.current = null;
    if (active) void refresh(false);
    return () => {
      readEpoch.current++;
      writeEpoch.current++;
    };
  }, [active, refresh]);
  const run = useCallback(
    async <T>(
      action: Record<string, unknown>,
      onSuccess?: (result: T) => void,
    ): Promise<RunOutcome> => {
      if (locked.current) return "failed";
      const token = {};
      locked.current = token;
      setBusy(true);
      setBusyAction(String(action.action));
      setError("");
      const generation = ++writeEpoch.current;
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
        return "failed";
      }
      if (mutation) {
        // 项目 Skill 的启停也写入用户配置，所有范围的清单均需失效。
        invalidateInventory(request);
      }
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
        if (!mounted.current) return "failed";
        // 被新的操作或范围切换抢占：后端已落库，但本地状态不能再用这次结果覆盖。
        if (generation !== writeEpoch.current) return "superseded";
        onSuccess?.(result);
        return "ok";
      } catch (cause) {
        const message = causeText(cause);
        if (mounted.current && generation === writeEpoch.current) {
          setError(message);
          if (mutation && message.includes("结果尚不确定")) setUncertain(true);
        }
        return "failed";
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
    async (action: Record<string, unknown>) => {
      const outcome = await run<MutationResult>(action, (result) => {
        setInventory(result.inventory);
        setNotice(
          `${result.message}${result.applyStatus === "restart-required" ? " 配置已保存，请重启 Codex 后确认生效；运行时覆盖可能影响最终状态。" : ""}`,
        );
      });
      // 提交已落库但清单结果被抢占：不能静默丢弃，提示用户刷新确认。
      if (outcome === "superseded")
        setNotice("操作已提交，请刷新确认最新状态。");
      return outcome;
    },
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
