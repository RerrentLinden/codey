export type NotificationChannelKind = "feishu" | "wecom" | "telegram" | "wechatClaw";

export const MAX_NOTIFICATION_CHANNELS = 32;

export type NotificationChannel = {
  id: string;
  kind: NotificationChannelKind;
  enabled: boolean;
  url: string;
  urlConfigured: boolean;
  clearUrl?: boolean;
  botToken: string;
  botTokenConfigured: boolean;
  clearBotToken?: boolean;
  contextToken: string;
  contextTokenConfigured: boolean;
  clearContextToken?: boolean;
  getUpdatesBuf?: string;
  chatId: string;
};

export type NotificationChannelEditorProps = {
  channel: NotificationChannel;
  disabled: boolean;
  onChange: (patch: Partial<NotificationChannel>) => void;
};
