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

type StatusPollScheduler = {
  add: (task: StatusPollTask) => void;
  remove: (task: StatusPollTask) => void;
};

function createStatusPollScheduler(
  requestRuntimeStatus: (
    refreshesInjectionStatus: boolean,
  ) => Promise<RuntimeStatus>,
): StatusPollScheduler {
  const tasks = new Map<StatusPollTask["kind"], StatusPollTask>();
  let timer = 0;
  let requestInProgress = false;

  const schedule = () => {
    if (requestInProgress) return;
    window.clearTimeout(timer);
    timer = 0;
    if (tasks.size === 0) return;
    const nextAt = Math.min(...[...tasks.values()].map((task) => task.nextAt));
    timer = window.setTimeout(() => {
      timer = 0;
      void poll();
    }, Math.max(0, nextAt - Date.now()));
  };

  const poll = async () => {
    if (requestInProgress || tasks.size === 0) return;
    const requestStartedAt = Date.now();
    const dueTasks = [...tasks.values()].filter(
      (task) => task.nextAt <= requestStartedAt,
    );
    if (dueTasks.length === 0) {
      schedule();
      return;
    }

    requestInProgress = true;
    try {
      const next = await requestRuntimeStatus(
        dueTasks.some((task) => task.refreshesInjectionStatus),
      );
      const completedAt = Date.now();
      for (const task of dueTasks) {
        if (tasks.get(task.kind) !== task) continue;
        task.errors = 0;
        task.delayIndex = Math.min(
          task.delayIndex + 1,
          task.delays.length - 1,
        );
        if (completedAt >= task.deadline || !task.pending(next)) {
          tasks.delete(task.kind);
          continue;
        }
        task.nextAt =
          completedAt +
          (task.kind === "restart" ? 500 : task.delays[task.delayIndex]);
      }
    } catch {
      const failedAt = Date.now();
      for (const task of dueTasks) {
        if (tasks.get(task.kind) !== task) continue;
        task.errors += 1;
        task.delayIndex = Math.min(
          task.delayIndex + 1,
          task.delays.length - 1,
        );
        if (
          failedAt >= task.deadline ||
          task.errors >= STATUS_POLL_MAX_CONSECUTIVE_ERRORS
        ) {
          tasks.delete(task.kind);
          continue;
        }
        task.nextAt =
          failedAt +
          (task.kind === "restart"
            ? Math.min(500 * 2 ** task.errors, 5_000)
            : task.delays[task.delayIndex]);
      }
    } finally {
      requestInProgress = false;
      schedule();
    }
  };

  return {
    add(task) {
      tasks.set(task.kind, task);
      schedule();
    },
    remove(task) {
      if (tasks.get(task.kind) === task) {
        tasks.delete(task.kind);
        schedule();
      }
    },
  };
}

function createStatusPollTask(
  task: Omit<StatusPollTask, "deadline" | "delayIndex" | "errors" | "nextAt">,
  duration: number,
): StatusPollTask {
  const now = Date.now();
  return {
    ...task,
    deadline: now + duration,
    delayIndex: 0,
    errors: 0,
    nextAt: now + task.delays[0],
  };
}

export function useRuntimeStatus({
  active,
  embedded,
}: UseRuntimeStatusOptions) {
  const [status, setStatus] = useState<RuntimeStatus>({ running: false });
  const runtimeStatusFlightRef = useRef<RuntimeStatusFlight | null>(null);
  const statusPollSchedulerRef = useRef<StatusPollScheduler | null>(null);
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

  if (statusPollSchedulerRef.current === null) {
    statusPollSchedulerRef.current = createStatusPollScheduler(
      requestRuntimeStatus,
    );
  }
  const statusPollScheduler = statusPollSchedulerRef.current;

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
  const gitGuardProbePending = gitGuardStatus === "executed";
  const wmiSamplerProbePending = wmiSamplerStatus === "executed";

  useEffect(() => {
    if (!active || (!gitGuardProbePending && !wmiSamplerProbePending)) return;
    const task = createStatusPollTask(
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
      wmiSamplerProbePending
        ? WMI_SAMPLER_PROBE_MAX_DURATION_MS
        : GIT_GUARD_PROBE_MAX_DURATION_MS,
    );
    statusPollScheduler.add(task);
    return () => statusPollScheduler.remove(task);
  }, [
    active,
    gitGuardProbePending,
    statusPollScheduler,
    wmiSamplerProbePending,
  ]);

  useEffect(() => {
    if (
      !active ||
      (!status.traceLogStats?.pending &&
        !status.crashpadPendingStats?.pending)
    )
      return;
    const task = createStatusPollTask(
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
    statusPollScheduler.add(task);
    return () => statusPollScheduler.remove(task);
  }, [
    active,
    status.crashpadPendingStats?.pending,
    status.traceLogStats?.pending,
    statusPollScheduler,
  ]);

  useEffect(() => {
    if (!active || !status.restartInProgress) return;
    const task = createStatusPollTask(
      {
        kind: "restart",
        delays: [500],
        pending: (next) => Boolean(next.restartInProgress),
        refreshesInjectionStatus: false,
      },
      STATUS_POLL_MAX_DURATION_MS,
    );
    statusPollScheduler.add(task);
    return () => statusPollScheduler.remove(task);
  }, [active, status.restartInProgress, statusPollScheduler]);

  return {
    status,
    setStatus,
    refreshStatus,
    refreshStatusForLoad,
  };
}
