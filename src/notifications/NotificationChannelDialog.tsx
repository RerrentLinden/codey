import {
  memo,
  useEffect,
  useLayoutEffect,
  useState,
} from "react";
import {
  IconCheck,
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
  Select,
  Switch,
} from "../components/mantine";
import { SETTINGS_OVERLAY_Z_INDEX } from "../overlay.constants";
import {
  createNotificationChannel,
  getNotificationChannelDefinition,
  notificationChannelDefinitions,
} from "./channelRegistry";
import type { NotificationChannel, NotificationChannelKind } from "./types";

const defaultNotificationChannelKind = notificationChannelDefinitions[0].kind;

type NotificationChannelDialogProps = {
  container: HTMLElement | null;
  popupContainer: HTMLElement | null;
  editingChannel: NotificationChannel | null;
  isBusy: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (channel: NotificationChannel) => Promise<boolean>;
};

function NotificationChannelDialogComponent({
  container,
  popupContainer,
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
  const [isSaving, setIsSaving] = useState(false);
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
      setIsSaving(false);
      setTestResult({ tone: "idle", text: "" });
      setHasSuccessfulTest(false);
      return;
    }
    setDraft(
      editingChannel
        ? { ...editingChannel }
        : createNotificationChannel(defaultNotificationChannelKind),
    );
    setIsRevealing(editingChannel !== null);
    setRevealError("");
    setIsTesting(false);
    setIsSaving(false);
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
    if (draft?.kind === kind) return;
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
    if (!isBusy && !isTesting && !isSaving) onOpenChange(false);
  }

  async function saveChannel() {
    if (
      !draft ||
      isRevealing ||
      !definition?.isConfigured(draft) ||
      !hasSuccessfulTest
    ) {
      return;
    }
    setIsSaving(true);
    try {
      if (await onSave(draft)) onOpenChange(false);
    } finally {
      setIsSaving(false);
    }
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

  const ChannelEditor = definition?.Editor;
  const SelectedChannelIcon = definition?.Icon;
  const notificationChannelOptions = notificationChannelDefinitions.map((item) => ({
    Icon: item.Icon,
    label: item.displayName,
    value: item.kind,
  }));
  const formBusy = isBusy || isRevealing || isTesting || isSaving;
  const canTest = Boolean(draft && definition?.isConfigured(draft));
  const canSave = Boolean(
    draft && definition?.isConfigured(draft) && hasSuccessfulTest,
  );

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => {
      if (nextOpen || (!isBusy && !isTesting && !isSaving)) onOpenChange(nextOpen);
    }}>
      <DialogContent
        className="notification-channel-dialog"
        container={container}
        onEscapeKeyDown={(event) => {
          if (isBusy || isTesting || isSaving) event.preventDefault();
        }}
        onPointerDownOutside={(event) => {
          if (isBusy || isTesting || isSaving) event.preventDefault();
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
              <LoaderCircle className="animate-spin" aria-hidden="true" />
              正在读取已保存配置…
            </div>
            <DialogFooter>
              <Button variant="outline" disabled={isBusy} onClick={closeDialog}>
                取消
              </Button>
            </DialogFooter>
          </>
        ) : draft && definition && ChannelEditor && SelectedChannelIcon ? (
          <>
            <DialogHeader>
              <DialogTitle>
                {isEditing ? `编辑${definition.addLabel}渠道` : "添加通知渠道"}
              </DialogTitle>
              <DialogDescription>
                {isEditing
                  ? "更新渠道配置；启用状态也会在这里一起保存。"
                  : "选择发送渠道，并填写该渠道需要的专属配置。"}
              </DialogDescription>
            </DialogHeader>
            <div className="mt-[18px] grid gap-[7px]">
              <span
                id="notification-channel-select-label"
                className="text-[11px] font-semibold text-[#6e6e73]"
              >
                发送渠道
              </span>
              <div className="relative w-[min(100%,260px)]">
                <Select
                  className="w-full"
                  inputClassName="h-10! rounded-lg! border-black/15! bg-white! text-xs font-semibold hover:border-blue-500/40! focus:border-blue-500/40! focus:ring-3 focus:ring-blue-500/8"
                  sectionClassName="text-[#8e8e93] data-[position=right]:mr-2.5"
                  value={draft.kind}
                  disabled={isEditing || formBusy}
                  aria-labelledby="notification-channel-select-label"
                  optionList={notificationChannelOptions}
                  dropdownClassName="rounded-[10px]"
                  showClear={false}
                  filter={false}
                  leftSectionPointerEvents="none"
                  leftSectionWidth={38}
                  prefix={
                    <span className="grid size-[22px] shrink-0 place-items-center">
                      <SelectedChannelIcon size={20} aria-hidden="true" />
                    </span>
                  }
                  getPopupContainer={() => popupContainer ?? document.body}
                  zIndex={SETTINGS_OVERLAY_Z_INDEX}
                  renderOptionItem={(option) => {
                    const optionDefinition = notificationChannelDefinitions.find(
                      (item) => item.kind === option.value,
                    );
                    const OptionIcon = optionDefinition?.Icon;
                    const selected = option.selected === true;
                    const focused = option.focused === true;
                    const label =
                      optionDefinition?.displayName ?? option.label;
                    if (!OptionIcon) return label;
                    return (
                      <div
                        className={`flex min-h-[34px] w-full items-center gap-2 rounded-md px-2.5 text-left text-xs font-semibold text-[#1d1d1f] ${option.className ?? ""} ${(selected || focused) ? "bg-blue-500/8" : ""}`}
                        role="option"
                        aria-selected={selected}
                        style={option.style}
                        onMouseEnter={option.onMouseEnter}
                        onClick={option.onClick}
                      >
                        <span className="grid size-[22px] shrink-0 place-items-center">
                          <OptionIcon size={20} aria-hidden="true" />
                        </span>
                        <span>{label}</span>
                      </div>
                    );
                  }}
                  onChange={(value) =>
                    selectChannel(String(value) as NotificationChannelKind)
                  }
                />
              </div>
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
                  className="shrink-0"
                  variant="secondary"
                  size="sm"
                  disabled={formBusy || !canTest}
                  onClick={() => void testChannel()}
                >
                  {isTesting ? (
                    <LoaderCircle className="animate-spin" aria-hidden="true" />
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
                onClick={() => void saveChannel()}
              >
                {isSaving ? (
                  <LoaderCircle className="animate-spin" aria-hidden="true" />
                ) : (
                  <IconCheck aria-hidden="true" />
                )}
                {isSaving
                  ? "正在保存"
                  : isEditing
                    ? "保存配置"
                    : "添加渠道"}
              </Button>
            </DialogFooter>
          </>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

export const NotificationChannelDialog = memo(
  NotificationChannelDialogComponent,
);
