import {
  memo,
  type Dispatch,
  type SetStateAction,
  useRef,
  useSyncExternalStore,
} from "react";

import type { Confirmation } from "./App.types";
import { ConfirmationDialog } from "./AppDialogs";

export type ConfirmationController = {
  clear: () => void;
  getSnapshot: () => Confirmation | null;
  setConfirmation: Dispatch<SetStateAction<Confirmation | null>>;
  subscribe: (listener: () => void) => () => void;
};

function createConfirmationController(): ConfirmationController {
  let confirmation: Confirmation | null = null;
  const listeners = new Set<() => void>();
  const setConfirmation: ConfirmationController["setConfirmation"] = (update) => {
    const next =
      typeof update === "function"
        ? (update as (current: Confirmation | null) => Confirmation | null)(
            confirmation,
          )
        : update;
    if (Object.is(next, confirmation)) return;
    confirmation = next;
    listeners.forEach((listener) => listener());
  };
  return {
    clear: () => setConfirmation(null),
    getSnapshot: () => confirmation,
    setConfirmation,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export function useConfirmationController(): ConfirmationController {
  const controllerRef = useRef<ConfirmationController | null>(null);
  controllerRef.current ??= createConfirmationController();
  return controllerRef.current;
}

type ConfirmationDialogHostProps = {
  container: HTMLElement | null;
  controller: ConfirmationController;
};

export const ConfirmationDialogHost = memo(function ConfirmationDialogHost({
  container,
  controller,
}: ConfirmationDialogHostProps) {
  const confirmation = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );
  return (
    <ConfirmationDialog
      confirmation={confirmation}
      container={container}
      onClose={controller.clear}
      onConfirm={(pending) => {
        controller.clear();
        pending.run();
      }}
    />
  );
});
