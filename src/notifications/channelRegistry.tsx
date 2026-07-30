import type { ComponentType } from "react";
import {
  IconBell,
  IconBrandTelegram,
} from "@tabler/icons-react";

import { FeishuChannelEditor } from "./FeishuChannelEditor";
import { TelegramChannelEditor } from "./TelegramChannelEditor";
import type {
  NotificationChannel,
  NotificationChannelEditorProps,
  NotificationChannelKind,
} from "./types";

type NotificationChannelDefinition = {
  kind: NotificationChannelKind;
  addLabel: string;
  displayName: string;
  title: string;
  description: string;
  iconClassName?: string;
  Icon: typeof IconBell;
  Editor: ComponentType<NotificationChannelEditorProps>;
  isConfigured: (channel: NotificationChannel) => boolean;
};

const CHANNEL_DEFINITIONS: Record<
  NotificationChannelKind,
  NotificationChannelDefinition
> = {
  feishu: {
    kind: "feishu",
    addLabel: "飞书",
    displayName: "飞书机器人",
    title: "飞书机器人 Webhook",
    description: "发送完成、失败和等待介入提醒",
    Icon: IconBell,
    Editor: FeishuChannelEditor,
    isConfigured: (channel) =>
      Boolean(channel.url.trim() || channel.urlConfigured),
  },
  telegram: {
    kind: "telegram",
    addLabel: "Telegram",
    displayName: "Telegram Bot",
    title: "Telegram Bot",
    description: "发送完成、失败和等待介入提醒",
    iconClassName: "telegram",
    Icon: IconBrandTelegram,
    Editor: TelegramChannelEditor,
    isConfigured: (channel) =>
      Boolean(
        (channel.botToken.trim() || channel.botTokenConfigured) &&
          channel.chatId.trim(),
      ),
  },
};

export const notificationChannelDefinitions = Object.values(
  CHANNEL_DEFINITIONS,
);

export function getNotificationChannelDefinition(
  kind: NotificationChannelKind,
) {
  return CHANNEL_DEFINITIONS[kind];
}

export function createNotificationChannel(
  kind: NotificationChannelKind,
): NotificationChannel {
  const id =
    globalThis.crypto?.randomUUID?.() ??
    `notification-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  return {
    id,
    kind,
    enabled: true,
    url: "",
    urlConfigured: false,
    clearUrl: false,
    botToken: "",
    botTokenConfigured: false,
    clearBotToken: false,
    chatId: "",
  };
}
