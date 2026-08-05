import {
  memo,
  type Dispatch,
  type SetStateAction,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  IconActivity as Activity,
  IconAlertCircle as CircleAlert,
  IconCircleCheck as CircleCheck,
  IconX as X,
} from "@tabler/icons-react";

import type { Notice } from "./App.types";
import { Button } from "./components/semi";

const NOTICE_AUTO_DISMISS_MS = 5_000;
const INITIAL_NOTICE: Notice = {
  tone: "info",
  text: "正在连接 Codey…",
};

export type AppNoticeController = {
  getSnapshot: () => Notice;
  setNotice: Dispatch<SetStateAction<Notice>>;
  subscribe: (listener: () => void) => () => void;
};

function createAppNoticeController(): AppNoticeController {
  let notice = INITIAL_NOTICE;
  const listeners = new Set<() => void>();
  return {
    getSnapshot: () => notice,
    setNotice: (update) => {
      const next =
        typeof update === "function"
          ? (update as (current: Notice) => Notice)(notice)
          : update;
      if (Object.is(next, notice)) return;
      notice = next;
      listeners.forEach((listener) => listener());
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export function useAppNoticeController(): AppNoticeController {
  const controllerRef = useRef<AppNoticeController | null>(null);
  controllerRef.current ??= createAppNoticeController();
  return controllerRef.current;
}

type NoticeSubscriberProps = {
  controller: AppNoticeController;
};

export const NoticeLoadingText = memo(function NoticeLoadingText({
  controller,
}: NoticeSubscriberProps) {
  const notice = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );
  return <>{notice.text}</>;
});

type NoticeToastProps = NoticeSubscriberProps & {
  autoDismissEnabled: boolean;
};

export const NoticeToast = memo(function NoticeToast({
  autoDismissEnabled,
  controller,
}: NoticeToastProps) {
  const notice = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );
  const [autoDismissPaused, setAutoDismissPaused] = useState(false);

  useEffect(() => {
    if (!notice.text) setAutoDismissPaused(false);
  }, [notice.text]);

  useEffect(() => {
    if (!autoDismissEnabled || !notice.text || autoDismissPaused) {
      return undefined;
    }
    const timeout = window.setTimeout(() => {
      setAutoDismissPaused(false);
      controller.setNotice((current) =>
        current.text === notice.text && current.tone === notice.tone
          ? { tone: "info", text: "" }
          : current,
      );
    }, NOTICE_AUTO_DISMISS_MS);
    return () => window.clearTimeout(timeout);
  }, [
    autoDismissEnabled,
    autoDismissPaused,
    controller,
    notice.text,
    notice.tone,
  ]);

  if (!notice.text) return null;
  return (
    <div
      className={`notice-toast ${notice.tone}`}
      role="status"
      aria-live="polite"
      onMouseEnter={() => setAutoDismissPaused(true)}
      onMouseLeave={() => setAutoDismissPaused(false)}
      onFocus={() => setAutoDismissPaused(true)}
      onBlur={() => setAutoDismissPaused(false)}
    >
      {notice.tone === "success" ? (
        <CircleCheck size={17} />
      ) : notice.tone === "error" ? (
        <CircleAlert size={17} />
      ) : (
        <Activity size={17} />
      )}
      <span>{notice.text}</span>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="关闭提示"
        onClick={() => {
          setAutoDismissPaused(false);
          controller.setNotice({ tone: "info", text: "" });
        }}
      >
        <X aria-hidden="true" />
      </Button>
    </div>
  );
});
