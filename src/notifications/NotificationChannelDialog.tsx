import { memo, useEffect, useLayoutEffect, useState } from "react";
import {
  IconArrowLeft,
  IconCheck,
  IconChevronRight,
  IconLoader2 as LoaderCircle,
  IconSend,
} from "@tabler/icons-react";

import { invoke } from "../api";
import { errorText, withTimeout } from "../appUtils";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Switch,
} from "../components/semi";
import {
  createNotificationChannel,
  getNotificationChannelDefinition,
  notificationChannelDefinitions,
} from "./channelRegistry";
import type { NotificationChannel, NotificationChannelKind } from "./types";

type NotificationChannelDialogProps = {
  container: HTMLElement | null;
  editingChannel: NotificationChannel | null;
  isBusy: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (channel: NotificationChannel) => void;
};

function NotificationChannelDialogComponent({
  container,
  editingChannel,
  isBusy,
  open,
  onOpenChange,
  onSave,
}: NotificationChannelDialogProps) {
  const [draft, setDraft] = useState<NotificationChannel | null>(null);
  const [isRevealing, setIsRevealing] = useState(false);
  const [revealError, setRevealError] = useState("");
  const [isTesting, setIsTesting] = useState(false);
  const [testResult, setTestResult] = useState<{
    text: string;
    tone: "idle" | "pending" | "success" | "error";
  }>({ tone: "idle", text: "" });
  const [hasSuccessfulTest, setHasSuccessfulTest] = useState(false);
  const isEditing = editingChannel !== null;
  const definition = draft
    ? getNotificationChannelDefinition(draft.kind)
    : null;

  useLayoutEffect(() => {
    if (!open) {
      setDraft(null);
      setIsRevealing(false);
      setRevealError("");
      setIsTesting(false);
      setTestResult({ tone: "idle", text: "" });
      setHasSuccessfulTest(false);
      return;
    }
    setDraft(editingChannel ? { ...editingChannel } : null);
    setIsRevealing(editingChannel !== null);
    setRevealError("");
    setIsTesting(false);
    setTestResult({ tone: "idle", text: "" });
    setHasSuccessfulTest(false);
  }, [editingChannel?.id, open]);

  useEffect(() => {
    if (!open || !editingChannel) return;
    let cancelled = false;
    void invoke<{ channel: NotificationChannel }>(
      "reveal_notification_channel",
      { channelId: editingChannel.id },
    )
      .then(({ channel }) => {
        if (!cancelled) setDraft(channel);
      })
      .catch((error) => {
        if (!cancelled) setRevealError(errorText(error));
      })
      .finally(() => {
        if (!cancelled) setIsRevealing(false);
      });
    return () => {
      cancelled = true;
    };
  }, [editingChannel?.id, open]);

  function selectChannel(kind: NotificationChannelKind) {
    setDraft(createNotificationChannel(kind));
    resetTestResult();
  }

  function resetTestResult() {
    setHasSuccessfulTest(false);
    setTestResult({ tone: "idle", text: "" });
  }

  function updateDraft(patch: Partial<NotificationChannel>) {
    setDraft((current) =>
      current ? { ...current, ...patch } : current,
    );
    resetTestResult();
  }

  function closeDialog() {
    if (!isBusy && !isTesting) onOpenChange(false);
  }

  function saveChannel() {
    if (
      !draft ||
      isRevealing ||
      !definition?.isConfigured(draft) ||
      !hasSuccessfulTest
    ) {
      return;
    }
    onSave(draft);
    onOpenChange(false);
  }

  async function testChannel() {
    if (
      !draft ||
      !definition?.isConfigured(draft) ||
      isBusy ||
      isRevealing ||
      isTesting
    ) {
      return;
    }
    setIsTesting(true);
    setHasSuccessfulTest(false);
    setTestResult({ tone: "pending", text: "正在发送测试通知…" });
    try {
      await withTimeout(
        invoke("test_notification_channel", { channel: draft }),
        12_000,
        `${definition.addLabel}测试在 12 秒内没有完成，请检查渠道配置和网络`,
      );
      setHasSuccessfulTest(true);
      setTestResult({ tone: "success", text: "测试发送成功" });
    } catch (error) {
      setHasSuccessfulTest(false);
      setTestResult({ tone: "error", text: errorText(error) });
    } finally {
      setIsTesting(false);
    }
  }

  const ChannelIcon = definition?.Icon;
  const ChannelEditor = definition?.Editor;
  const formBusy = isBusy || isRevealing || isTesting;
  const canTest = Boolean(draft && definition?.isConfigured(draft));
  const canSave = Boolean(
    draft && definition?.isConfigured(draft) && hasSuccessfulTest,
  );

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => {
      if (nextOpen || (!isBusy && !isTesting)) onOpenChange(nextOpen);
    }}>
      <DialogContent
        className="notification-channel-dialog"
        container={container}
        onEscapeKeyDown={(event) => {
          if (isBusy || isTesting) event.preventDefault();
        }}
        onPointerDownOutside={(event) => {
          if (isBusy || isTesting) event.preventDefault();
        }}
      >
        {isEditing && isRevealing ? (
          <>
            <DialogHeader>
              <DialogTitle>正在读取渠道配置</DialogTitle>
              <DialogDescription>
                正在按本次编辑请求读取已保存的渠道内容。
              </DialogDescription>
            </DialogHeader>
            <div className="notification-reveal-loading" role="status">
              <LoaderCircle className="spinner" aria-hidden="true" />
              正在读取已保存配置…
            </div>
            <DialogFooter>
              <Button variant="outline" disabled={isBusy} onClick={closeDialog}>
                取消
              </Button>
            </DialogFooter>
          </>
        ) : draft && definition && ChannelIcon && ChannelEditor ? (
          <>
            <DialogHeader>
              <DialogTitle>
                {isEditing ? `编辑${definition.addLabel}渠道` : `配置${definition.addLabel}渠道`}
              </DialogTitle>
              <DialogDescription>
                填写渠道专属配置；启用状态也会在这里一起保存。
              </DialogDescription>
            </DialogHeader>
            <div className="notification-dialog-channel">
              <div className="notification-title">
                <span className={definition.iconClassName}>
                  <ChannelIcon size={18} aria-hidden="true" />
                </span>
                <div>
                  <strong>{definition.title}</strong>
                  <small>{definition.description}</small>
                </div>
              </div>
              {!isEditing ? (
                <Button
                  variant="ghost"
                  size="xs"
                  disabled={isBusy}
                  onClick={() => {
                    setDraft(null);
                    resetTestResult();
                  }}
                >
                  <IconArrowLeft aria-hidden="true" />
                  更换渠道
                </Button>
              ) : null}
            </div>
            <div className="notification-fields notification-dialog-fields">
              <ChannelEditor
                channel={draft}
                disabled={formBusy}
                revealSecrets={isEditing && !revealError}
                onChange={updateDraft}
              />
            </div>
            <div className="notification-dialog-actions">
              <div className="notification-enabled-control">
                <div>
                  <strong>启用此渠道</strong>
                  <small>关闭后不接收自动通知</small>
                </div>
                <Switch
                  checked={draft.enabled}
                  disabled={formBusy}
                  onCheckedChange={(enabled) => updateDraft({ enabled })}
                  aria-label={`启用${definition.addLabel}通知`}
                />
              </div>
              <div className="notification-dialog-test">
                <div>
                  <strong>测试发送</strong>
                  <span
                    className={`inline-result ${testResult.tone}`}
                    role="status"
                    aria-live="polite"
                  >
                    {testResult.tone === "error"
                      ? "测试失败，请检查配置"
                      : testResult.text || (canTest
                        ? "测试成功后可保存"
                        : "填写配置后可测试")}
                  </span>
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={formBusy || !canTest}
                  onClick={() => void testChannel()}
                >
                  {isTesting ? (
                    <LoaderCircle className="spinner" aria-hidden="true" />
                  ) : (
                    <IconSend aria-hidden="true" />
                  )}
                  {isTesting ? "正在测试" : "测试发送"}
                </Button>
              </div>
            </div>
            {testResult.tone === "error" ? (
              <p className="notification-test-error" role="alert">
                {testResult.text}
              </p>
            ) : null}
            {revealError ? (
              <p className="notification-reveal-error" role="alert">
                无法回显已保存内容：{revealError}。你仍可填写新内容后保存。
              </p>
            ) : null}
            <DialogFooter>
              <Button variant="outline" disabled={formBusy} onClick={closeDialog}>
                取消
              </Button>
              <Button
                disabled={formBusy || !canSave}
                onClick={saveChannel}
              >
                <IconCheck aria-hidden="true" />
                {isEditing ? "保存配置" : "添加渠道"}
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>添加通知渠道</DialogTitle>
              <DialogDescription>
                选择一个发送渠道，再填写该渠道需要的专属配置。
              </DialogDescription>
            </DialogHeader>
            <div className="notification-channel-picker" role="list">
              {notificationChannelDefinitions.map((item) => {
                const PickerIcon = item.Icon;
                return (
                  <Button
                    className="notification-channel-picker-item"
                    key={item.kind}
                    variant="outline"
                    disabled={isBusy}
                    role="listitem"
                    onClick={() => selectChannel(item.kind)}
                  >
                    <span className={item.iconClassName}>
                      <PickerIcon size={18} aria-hidden="true" />
                    </span>
                    <span className="notification-channel-picker-copy">
                      <strong>{item.title}</strong>
                      <small>{item.description}</small>
                    </span>
                    <IconChevronRight aria-hidden="true" />
                  </Button>
                );
              })}
            </div>
            <DialogFooter>
              <Button variant="outline" disabled={isBusy} onClick={closeDialog}>
                取消
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

export const NotificationChannelDialog = memo(
  NotificationChannelDialogComponent,
);
