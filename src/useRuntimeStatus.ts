import { useCallback, useEffect, useRef, useState } from "react";

import { invoke } from "./api";
import type { RuntimeStatus } from "./App.types";
import { reconcileRuntimeStatus } from "./runtimeStatusSnapshot";

const INJECTION_STATUS_CHANGED_EVENT = "codey-injection-status-changed";
export const SETTINGS_OPENED_EVENT = "codey-settings-opened";

type UseRuntimeStatusOptions = {
  active: boolean;
  embedded: boolean;
};

const STATUS_POLL_MAX_DURATION_MS = 5 * 60 * 1_000;
const STATUS_POLL_MAX_CONSECUTIVE_ERRORS = 5;

export function useRuntimeStatus({
  active,
  embedded,
}: UseRuntimeStatusOptions) {
  const [status, setStatus] = useState<RuntimeStatus>({ running: false });
  const injectionStatusRefreshRef = useRef<Promise<RuntimeStatus> | null>(null);
  const settingsOpenRefreshRequestedRef = useRef(false);
  const activeRef = useRef(active);
  activeRef.current = active;

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
      if (!activeRef.current) return;
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
      !active ||
      !status.traceLogStats?.pending &&
      !status.crashpadPendingStats?.pending
    ) return;
    const delays = [250, 500, 1_000, 2_000, 5_000];
    let cancelled = false;
    let timer = 0;
    let delayIndex = 0;
    let consecutiveErrors = 0;
    const deadline = Date.now() + STATUS_POLL_MAX_DURATION_MS;
    const poll = () => {
      if (cancelled || Date.now() >= deadline) return;
      const delay = delays[delayIndex];
      delayIndex = Math.min(delayIndex + 1, delays.length - 1);
      timer = window.setTimeout(async () => {
        try {
          const next = await invoke<RuntimeStatus>("runtime_status");
          if (cancelled) return;
          consecutiveErrors = 0;
          setStatus((current) => reconcileRuntimeStatus(current, next));
          if (
            next.traceLogStats?.pending ||
            next.crashpadPendingStats?.pending
          ) poll();
        } catch {
          consecutiveErrors += 1;
          if (consecutiveErrors < STATUS_POLL_MAX_CONSECUTIVE_ERRORS) poll();
        }
      }, delay);
    };
    poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [
    active,
    status.crashpadPendingStats?.pending,
    status.traceLogStats?.pending,
  ]);

  useEffect(() => {
    if (!active || !status.restartInProgress) return;
    let cancelled = false;
    let timer = 0;
    let consecutiveErrors = 0;
    const deadline = Date.now() + STATUS_POLL_MAX_DURATION_MS;
    const poll = (delay = 500) => {
      if (cancelled || Date.now() >= deadline) return;
      timer = window.setTimeout(async () => {
        try {
          const next = await invoke<RuntimeStatus>("runtime_status");
          if (cancelled) return;
          consecutiveErrors = 0;
          setStatus((current) => reconcileRuntimeStatus(current, next));
          if (next.restartInProgress) poll();
        } catch {
          consecutiveErrors += 1;
          if (
            !cancelled &&
            consecutiveErrors < STATUS_POLL_MAX_CONSECUTIVE_ERRORS
          ) {
            poll(Math.min(500 * 2 ** consecutiveErrors, 5_000));
          }
        }
      }, delay);
    };
    poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [active, status.restartInProgress]);

  return {
    status,
    setStatus,
    refreshStatus,
    refreshStatusForLoad,
  };
}
