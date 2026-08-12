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
const GIT_GUARD_PROBE_DELAYS_MS = [500, 1_000, 2_000, 5_000];
const GIT_GUARD_PROBE_MAX_DURATION_MS = 30_000;
const WMI_SAMPLER_PROBE_MAX_DURATION_MS = 60_000;
const DIAGNOSTIC_PROBE_DELAYS_MS = [250, 500, 1_000, 2_000, 5_000];

type RuntimeStatusFlight = {
  refreshesInjectionStatus: boolean;
  promise: Promise<RuntimeStatus>;
};

type StatusPollTask = {
  deadline: number;
  delayIndex: number;
  delays: readonly number[];
  errors: number;
  kind: "injection" | "diagnostics" | "restart";
  nextAt: number;
  pending: (next: RuntimeStatus) => boolean;
  refreshesInjectionStatus: boolean;
};

export function useRuntimeStatus({
  active,
  embedded,
}: UseRuntimeStatusOptions) {
  const [status, setStatus] = useState<RuntimeStatus>({ running: false });
  const runtimeStatusFlightRef = useRef<RuntimeStatusFlight | null>(null);
  const settingsOpenRefreshRequestedRef = useRef(false);
  const activeRef = useRef(active);
  activeRef.current = active;

  const requestRuntimeStatus = useCallback(
    (shouldRefreshInjectionStatus: boolean): Promise<RuntimeStatus> => {
      const startRequest = (refreshesInjectionStatus: boolean) =>
        invoke<RuntimeStatus>("runtime_status", {
          refreshInjectionStatus: refreshesInjectionStatus,
        }).then((next) => {
          setStatus((current) => reconcileRuntimeStatus(current, next));
          return next;
        });

      const currentFlight = runtimeStatusFlightRef.current;
      if (currentFlight) {
        if (
          !shouldRefreshInjectionStatus ||
          currentFlight.refreshesInjectionStatus
        ) {
          return currentFlight.promise;
        }

        const queuedFlight: RuntimeStatusFlight = {
          refreshesInjectionStatus: true,
          promise: currentFlight.promise
            .catch(() => undefined)
            .then(() => startRequest(true)),
        };
        runtimeStatusFlightRef.current = queuedFlight;
        const clearQueuedFlight = () => {
          if (runtimeStatusFlightRef.current === queuedFlight) {
            runtimeStatusFlightRef.current = null;
          }
        };
        void queuedFlight.promise.then(clearQueuedFlight, clearQueuedFlight);
        return queuedFlight.promise;
      }

      const flight: RuntimeStatusFlight = {
        refreshesInjectionStatus: shouldRefreshInjectionStatus,
        promise: startRequest(shouldRefreshInjectionStatus),
      };
      runtimeStatusFlightRef.current = flight;
      const clearFlight = () => {
        if (runtimeStatusFlightRef.current === flight) {
          runtimeStatusFlightRef.current = null;
        }
      };
      void flight.promise.then(clearFlight, clearFlight);
      return flight.promise;
    },
    [],
  );

  const refreshStatus = useCallback(
    () => requestRuntimeStatus(false),
    [requestRuntimeStatus],
  );

  const refreshInjectionStatus = useCallback(
    () => requestRuntimeStatus(true),
    [requestRuntimeStatus],
  );

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

  const gitGuardStatus = status.injectionScripts?.find(
    (script) => script.id === "git-request-guard",
  )?.status;
  const wmiSamplerStatus = status.injectionScripts?.find(
    (script) => script.id === "windows-wmi-sampler",
  )?.status;

  useEffect(() => {
    if (!active) return;
    const now = Date.now();
    let tasks: StatusPollTask[] = [];
    const addTask = (
      task: Omit<StatusPollTask, "deadline" | "delayIndex" | "errors" | "nextAt">,
      duration: number,
    ) => {
      tasks.push({
        ...task,
        deadline: now + duration,
        delayIndex: 0,
        errors: 0,
        nextAt: now + task.delays[0],
      });
    };

    if (gitGuardStatus === "executed" || wmiSamplerStatus === "executed") {
      addTask(
        {
          kind: "injection",
          delays: GIT_GUARD_PROBE_DELAYS_MS,
          pending: (next) =>
            next.injectionScripts?.some(
              (script) =>
                (script.id === "git-request-guard" ||
                  script.id === "windows-wmi-sampler") &&
                script.status === "executed",
            ) ?? false,
          refreshesInjectionStatus: true,
        },
        wmiSamplerStatus === "executed"
          ? WMI_SAMPLER_PROBE_MAX_DURATION_MS
          : GIT_GUARD_PROBE_MAX_DURATION_MS,
      );
    }
    if (
      status.traceLogStats?.pending ||
      status.crashpadPendingStats?.pending
    ) {
      addTask(
        {
          kind: "diagnostics",
          delays: DIAGNOSTIC_PROBE_DELAYS_MS,
          pending: (next) =>
            Boolean(
              next.traceLogStats?.pending ||
              next.crashpadPendingStats?.pending,
            ),
          refreshesInjectionStatus: false,
        },
        STATUS_POLL_MAX_DURATION_MS,
      );
    }
    if (status.restartInProgress) {
      addTask(
        {
          kind: "restart",
          delays: [500],
          pending: (next) => Boolean(next.restartInProgress),
          refreshesInjectionStatus: false,
        },
        STATUS_POLL_MAX_DURATION_MS,
      );
    }
    if (tasks.length === 0) return;

    let cancelled = false;
    let timer = 0;
    const schedule = () => {
      if (cancelled || tasks.length === 0) return;
      const nextAt = Math.min(...tasks.map((task) => task.nextAt));
      timer = window.setTimeout(async () => {
        const requestStartedAt = Date.now();
        const dueTasks = tasks.filter(
          (task) => task.nextAt <= requestStartedAt && requestStartedAt < task.deadline,
        );
        if (dueTasks.length === 0) {
          tasks = tasks.filter((task) => requestStartedAt < task.deadline);
          schedule();
          return;
        }
        try {
          const next = await requestRuntimeStatus(
            dueTasks.some((task) => task.refreshesInjectionStatus),
          );
          if (cancelled) return;
          const completedAt = Date.now();
          for (const task of dueTasks) {
            task.errors = 0;
            task.delayIndex = Math.min(
              task.delayIndex + 1,
              task.delays.length - 1,
            );
            task.nextAt =
              completedAt +
              (task.kind === "restart"
                ? 500
                : task.delays[task.delayIndex]);
          }
          tasks = tasks.filter(
            (task) => completedAt < task.deadline && task.pending(next),
          );
        } catch {
          if (cancelled) return;
          const failedAt = Date.now();
          for (const task of dueTasks) {
            task.errors += 1;
            task.delayIndex = Math.min(
              task.delayIndex + 1,
              task.delays.length - 1,
            );
            task.nextAt =
              failedAt +
              (task.kind === "restart"
                ? Math.min(500 * 2 ** task.errors, 5_000)
                : task.delays[task.delayIndex]);
          }
          tasks = tasks.filter(
            (task) =>
              failedAt < task.deadline &&
              task.errors < STATUS_POLL_MAX_CONSECUTIVE_ERRORS,
          );
        }
        schedule();
      }, Math.max(0, nextAt - Date.now()));
    };
    schedule();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [
    active,
    gitGuardStatus,
    requestRuntimeStatus,
    status.crashpadPendingStats?.pending,
    status.restartInProgress,
    status.traceLogStats?.pending,
    wmiSamplerStatus,
  ]);

  return {
    status,
    setStatus,
    refreshStatus,
    refreshStatusForLoad,
  };
}
