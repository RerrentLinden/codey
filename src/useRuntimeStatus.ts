import { useCallback, useEffect, useRef, useState } from "react";

import { invoke } from "./api";
import type { RuntimeStatus } from "./App.types";
import { reconcileRuntimeStatus } from "./runtimeStatusSnapshot";

const INJECTION_STATUS_CHANGED_EVENT = "codey-injection-status-changed";
export const SETTINGS_OPENED_EVENT = "codey-settings-opened";

type UseRuntimeStatusOptions = {
  embedded: boolean;
};

export function useRuntimeStatus({ embedded }: UseRuntimeStatusOptions) {
  const [status, setStatus] = useState<RuntimeStatus>({ running: false });
  const injectionStatusRefreshRef = useRef<Promise<RuntimeStatus> | null>(null);
  const settingsOpenRefreshRequestedRef = useRef(false);

  const requestRuntimeStatus = useCallback(
    async (shouldRefreshInjectionStatus: boolean) => {
      if (shouldRefreshInjectionStatus) {
        await invoke("refresh_injection_status");
      }
      return invoke<RuntimeStatus>("runtime_status");
    },
    [],
  );

  const refreshStatus = useCallback(async () => {
    const next = await requestRuntimeStatus(false);
    setStatus((current) => reconcileRuntimeStatus(current, next));
    return next;
  }, [requestRuntimeStatus]);

  const refreshInjectionStatus = useCallback(() => {
    if (injectionStatusRefreshRef.current) {
      return injectionStatusRefreshRef.current;
    }
    const refresh = requestRuntimeStatus(true)
      .then((next) => {
        setStatus((current) => reconcileRuntimeStatus(current, next));
        return next;
      })
      .finally(() => {
        if (injectionStatusRefreshRef.current === refresh) {
          injectionStatusRefreshRef.current = null;
        }
      });
    injectionStatusRefreshRef.current = refresh;
    return refresh;
  }, [requestRuntimeStatus]);

  const refreshStatusForLoad = useCallback(() => {
    const shouldRefreshInjectionStatus =
      !embedded || !settingsOpenRefreshRequestedRef.current;
    return shouldRefreshInjectionStatus
      ? refreshInjectionStatus()
      : refreshStatus();
  }, [embedded, refreshInjectionStatus, refreshStatus]);

  useEffect(() => {
    const handleInjectionStatusChanged = () => {
      void refreshInjectionStatus().catch(() => {});
    };
    window.addEventListener(
      INJECTION_STATUS_CHANGED_EVENT,
      handleInjectionStatusChanged,
    );
    return () => {
      window.removeEventListener(
        INJECTION_STATUS_CHANGED_EVENT,
        handleInjectionStatusChanged,
      );
    };
  }, [refreshInjectionStatus]);

  useEffect(() => {
    const handleSettingsOpened = () => {
      settingsOpenRefreshRequestedRef.current = true;
      void refreshInjectionStatus().catch(() => {});
    };
    window.addEventListener(SETTINGS_OPENED_EVENT, handleSettingsOpened);
    return () => {
      window.removeEventListener(SETTINGS_OPENED_EVENT, handleSettingsOpened);
    };
  }, [refreshInjectionStatus]);

  useEffect(() => {
    if (
      !status.traceLogStats?.pending &&
      !status.crashpadPendingStats?.pending
    ) return;
    const delays = [250, 500, 1_000, 2_000, 5_000];
    let cancelled = false;
    let timer = 0;
    let delayIndex = 0;
    const poll = () => {
      if (cancelled) return;
      const delay = delays[delayIndex];
      delayIndex = Math.min(delayIndex + 1, delays.length - 1);
      timer = window.setTimeout(async () => {
        try {
          const next = await invoke<RuntimeStatus>("runtime_status");
          if (cancelled) return;
          setStatus((current) => reconcileRuntimeStatus(current, next));
          if (
            next.traceLogStats?.pending ||
            next.crashpadPendingStats?.pending
          ) poll();
        } catch {
          poll();
        }
      }, delay);
    };
    poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [
    status.crashpadPendingStats?.pending,
    status.traceLogStats?.pending,
  ]);

  useEffect(() => {
    if (!status.restartInProgress) return;
    let cancelled = false;
    let timer = 0;
    const poll = () => {
      timer = window.setTimeout(async () => {
        try {
          const next = await invoke<RuntimeStatus>("runtime_status");
          if (cancelled) return;
          setStatus((current) => reconcileRuntimeStatus(current, next));
          if (next.restartInProgress) poll();
        } catch {
          if (!cancelled) poll();
        }
      }, 500);
    };
    poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [status.restartInProgress]);

  return {
    status,
    setStatus,
    refreshStatus,
    refreshStatusForLoad,
  };
}
