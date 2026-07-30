import { memo } from "react";
import {
  IconBell,
  IconLoader2 as LoaderCircle,
  IconPlus,
  IconSend,
  IconTrash,
} from "@tabler/icons-react";

import type { Config, InlineResult } from "../App.types";
import { Button, Card, Switch } from "../components/semi";
import {
  getNotificationChannelDefinition,
  notificationChannelDefinitions,
} from "./channelRegistry";
import type {
  NotificationChannel,
  NotificationChannelKind,
} from "./types";

type NotificationChannelsCardProps = {
  config: Config;
  busy: string | null;
  isBusy: boolean;
  webhookResults: Record<string, InlineResult>;
  onAddChannel: (kind: NotificationChannelKind) => void;
  onChannelChange: (
    channelId: string,
    patch: Partial<NotificationChannel>,
  ) => void;
  onRemoveChannel: (channelId: string) => void;
  onTestWebhook: (channelId: string) => void;
};

function NotificationChannelsCardComponent({
  config,
  busy,
  isBusy,
  webhookResults,
  onAddChannel,
  onChannelChange,
  onRemoveChannel,
  onTestWebhook,
}: NotificationChannelsCardProps) {
  return (
    <section className="secondary-section" aria-labelledby="notification-title">
      <div className="section-title compact">
        <div>
          <h2 id="notification-title">消息通知</h2>
          <p>按需添加渠道，完成、失败和等待提醒会同时发送。</p>
        </div>
        <div className="notification-add-actions">
          {notificationChannelDefinitions.map((definition) => (
            <Button
              key={definition.kind}
              variant="secondary"
              size="xs"
              disabled={isBusy}
              onClick={() => onAddChannel(definition.kind)}
            >
              <IconPlus aria-hidden="true" />
              {definition.addLabel}
            </Button>
          ))}
        </div>
      </div>
      <div className="notification-channel-list">
        {config.webhook.channels.length === 0 ? (
          <Card className="secondary-card notification-empty">
            <IconBell size={20} aria-hidden="true" />
            <strong>还没有通知渠道</strong>
            <small>点击上方按钮添加通知渠道。</small>
          </Card>
        ) : (
          config.webhook.channels.map((channel) => {
            const definition = getNotificationChannelDefinition(channel.kind);
            const result = webhookResults[channel.id] ?? {
              tone: "idle",
              text: "",
            };
            const isTesting = busy === `test-webhook-${channel.id}`;
            const ChannelIcon = definition.Icon;
            const ChannelEditor = definition.Editor;
            return (
              <Card
                className={`secondary-card notification-card ${channel.enabled ? "active" : ""}`}
                key={channel.id}
              >
                <div className="notification-card-header">
                  <div className="notification-title">
                    <span className={definition.iconClassName}>
                      <ChannelIcon size={18} aria-hidden="true" />
                    </span>
                    <div>
                      <strong>{definition.title}</strong>
                      <small>{definition.description}</small>
                    </div>
                  </div>
                  <div className="notification-channel-controls">
                    <span>{channel.enabled ? "已开启" : "已关闭"}</span>
                    <Switch
                      checked={channel.enabled}
                      disabled={isBusy}
                      onCheckedChange={(checked) =>
                        onChannelChange(channel.id, { enabled: checked })
                      }
                      aria-label={`启用${definition.addLabel}通知`}
                    />
                    <Button
                      className="notification-remove-button"
                      variant="ghost"
                      size="icon-sm"
                      disabled={isBusy}
                      onClick={() => onRemoveChannel(channel.id)}
                      aria-label={`删除${definition.addLabel}通知渠道`}
                    >
                      <IconTrash size={15} aria-hidden="true" />
                    </Button>
                  </div>
                </div>
                <div className="notification-fields">
                  <ChannelEditor
                    channel={channel}
                    disabled={isBusy}
                    onChange={(patch) => onChannelChange(channel.id, patch)}
                  />
                </div>
                <div className="notification-action">
                  <span
                    className={`inline-result ${result.tone}`}
                    role="status"
                    aria-live="polite"
                  >
                    {result.text || ""}
                  </span>
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={isBusy || !definition.isConfigured(channel)}
                    onClick={() => onTestWebhook(channel.id)}
                  >
                    {isTesting ? (
                      <LoaderCircle className="spinner" aria-hidden="true" />
                    ) : (
                      <IconSend aria-hidden="true" />
                    )}
                    测试通知
                  </Button>
                </div>
              </Card>
            );
          })
        )}
      </div>
    </section>
  );
}

export const NotificationChannelsCard = memo(
  NotificationChannelsCardComponent,
);
