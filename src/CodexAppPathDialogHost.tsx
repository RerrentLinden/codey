import { memo, useRef, useSyncExternalStore } from "react";

import { CodexAppPathDialog } from "./AppDialogs";

type CodexAppPathDialogState = {
  error: string;
  open: boolean;
  selectedPath: string;
};

export type CodexAppPathDialogController = {
  getSnapshot: () => CodexAppPathDialogState;
  setError: (error: string) => void;
  setOpen: (open: boolean) => void;
  setSelectedPath: (selectedPath: string) => void;
  subscribe: (listener: () => void) => () => void;
};

function createCodexAppPathDialogController(): CodexAppPathDialogController {
  let state: CodexAppPathDialogState = {
    error: "",
    open: false,
    selectedPath: "",
  };
  const listeners = new Set<() => void>();
  const update = (patch: Partial<CodexAppPathDialogState>) => {
    const next = { ...state, ...patch };
    if (
      next.error === state.error &&
      next.open === state.open &&
      next.selectedPath === state.selectedPath
    ) return;
    state = next;
    listeners.forEach((listener) => listener());
  };
  return {
    getSnapshot: () => state,
    setError: (error) => update({ error }),
    setOpen: (open) => update({ open }),
    setSelectedPath: (selectedPath) => update({ selectedPath }),
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export function useCodexAppPathDialogController(): CodexAppPathDialogController {
  const controllerRef = useRef<CodexAppPathDialogController | null>(null);
  controllerRef.current ??= createCodexAppPathDialogController();
  return controllerRef.current;
}

type CodexAppPathDialogHostProps = {
  busy: string | null;
  container: HTMLElement | null;
  controller: CodexAppPathDialogController;
  isBusy: boolean;
  onChooseDirectory: () => void;
  onConfirm: () => void;
  onOpenChange: (open: boolean) => void;
};

export const CodexAppPathDialogHost = memo(function CodexAppPathDialogHost({
  busy,
  container,
  controller,
  isBusy,
  onChooseDirectory,
  onConfirm,
  onOpenChange,
}: CodexAppPathDialogHostProps) {
  const state = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );
  return (
    <CodexAppPathDialog
      open={state.open}
      selectedPath={state.selectedPath}
      error={state.error}
      isBusy={isBusy}
      busy={busy}
      container={container}
      onOpenChange={onOpenChange}
      onChooseDirectory={onChooseDirectory}
      onConfirm={onConfirm}
    />
  );
});
