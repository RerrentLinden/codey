import type { Dispatch, SetStateAction } from "react";

import type { Config } from "./App.types";
import type { NotificationChannel } from "./notifications";

type UseNotificationsOptions = {
  setConfig: Dispatch<SetStateAction<Config | null>>;
  setDirty: Dispatch<SetStateAction<boolean>>;
};

export function useNotifications({
  setConfig,
  setDirty,
}: UseNotificationsOptions) {
  function addNotificationChannel(channel: NotificationChannel) {
    setConfig((current) =>
      current
        ? {
            ...current,
            webhook: {
              ...current.webhook,
              channels: current.webhook.channels.some(
                (existing) => existing.id === channel.id,
              )
                ? current.webhook.channels
                : [...current.webhook.channels, channel],
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
  }

  return {
    addNotificationChannel,
    updateNotificationChannel,
    removeNotificationChannel,
  };
}
