export type NotificationChannelKind = "feishu" | "telegram";

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
  chatId: string;
};

export type NotificationChannelEditorProps = {
  channel: NotificationChannel;
  disabled: boolean;
  revealSecrets?: boolean;
  onChange: (patch: Partial<NotificationChannel>) => void;
};
