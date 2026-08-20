import { createWebhookChannelEditor } from "./WebhookChannelEditor";

export const WecomChannelEditor = createWebhookChannelEditor(
  "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=...",
);
