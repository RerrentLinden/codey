import {
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";

import { invoke } from "./api";
import type { Config, InlineResult, Notice } from "./App.types";
import { errorText, withTimeout } from "./appUtils";
import {
  createNotificationChannel,
  getNotificationChannelDefinition,
} from "./notifications";
import type {
  NotificationChannel,
  NotificationChannelKind,
} from "./notifications";

type UseNotificationsOptions = {
  config: Config | null;
  isBusy: boolean;
  setConfig: Dispatch<SetStateAction<Config | null>>;
  setDirty: Dispatch<SetStateAction<boolean>>;
  setBusy: Dispatch<SetStateAction<string | null>>;
  setNotice: Dispatch<SetStateAction<Notice>>;
  persist: (config: Config) => Promise<unknown>;
};

export function useNotifications({
  config,
  isBusy,
  setConfig,
  setDirty,
  setBusy,
  setNotice,
  persist,
}: UseNotificationsOptions) {
  const [webhookResults, setWebhookResults] = useState<
    Record<string, InlineResult>
  >({});

  function addNotificationChannel(kind: NotificationChannelKind) {
    const channel = createNotificationChannel(kind);
    setConfig((current) =>
      current
        ? {
            ...current,
            webhook: {
              ...current.webhook,
              channels: [...current.webhook.channels, channel],
            },
          }
        : current,
    );
    setDirty(true);
  }

  function updateNotificationChannel(
    channelId: string,
    patch: Partial<NotificationChannel>,
  ) {
    setConfig((current) =>
      current
        ? {
            ...current,
            webhook: {
              ...current.webhook,
              channels: current.webhook.channels.map((channel) =>
                channel.id === channelId ? { ...channel, ...patch } : channel,
              ),
            },
          }
        : current,
    );
    setDirty(true);
    setWebhookResults((current) => ({
      ...current,
      [channelId]: { tone: "idle", text: "" },
    }));
  }

  function removeNotificationChannel(channelId: string) {
    setConfig((current) =>
      current
        ? {
            ...current,
            webhook: {
              ...current.webhook,
              channels: current.webhook.channels.filter(
                (channel) => channel.id !== channelId,
              ),
            },
          }
        : current,
    );
    setDirty(true);
    setWebhookResults((current) => {
      const next = { ...current };
      delete next[channelId];
      return next;
    });
  }

  async function testWebhook(channelId: string) {
    if (!config || isBusy) return;
    const channel = config.webhook.channels.find(
      (candidate) => candidate.id === channelId,
    );
    if (!channel) return;
    const definition = getNotificationChannelDefinition(channel.kind);
    setBusy(`test-webhook-${channelId}`);
    setWebhookResults((current) => ({
      ...current,
      [channelId]: { tone: "pending", text: "正在发送测试通知…" },
    }));
    try {
      await persist(config);
      await withTimeout(
        invoke("test_webhook", { channelId }),
        12_000,
        `${definition.addLabel}测试在 12 秒内没有完成，请检查渠道配置和网络`,
      );
      setWebhookResults((current) => ({
        ...current,
        [channelId]: { tone: "success", text: "测试通知已发送" },
      }));
      setNotice({
        tone: "success",
        text: `${definition.displayName}连接成功`,
      });
    } catch (error) {
      const text = errorText(error);
      setWebhookResults((current) => ({
        ...current,
        [channelId]: { tone: "error", text },
      }));
      setNotice({ tone: "error", text });
    } finally {
      setBusy(null);
    }
  }

  return {
    webhookResults,
    addNotificationChannel,
    updateNotificationChannel,
    removeNotificationChannel,
    testWebhook,
  };
}
