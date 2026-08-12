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
  function updateNotificationChannels(
    update: (channels: NotificationChannel[]) => NotificationChannel[],
  ) {
    setConfig((current) =>
      current
        ? {
            ...current,
            webhook: {
              ...current.webhook,
              channels: update(current.webhook.channels),
            },
          }
        : current,
    );
  }

  function addNotificationChannel(channel: NotificationChannel) {
    updateNotificationChannels((channels) =>
      channels.some((existing) => existing.id === channel.id)
        ? channels
        : [...channels, channel],
    );
    setDirty(true);
  }

  function updateNotificationChannel(
    channelId: string,
    patch: Partial<NotificationChannel>,
  ) {
    updateNotificationChannels((channels) =>
      channels.map((channel) =>
        channel.id === channelId ? { ...channel, ...patch } : channel,
      ),
    );
    setDirty(true);
  }

  function removeNotificationChannel(channelId: string) {
    updateNotificationChannels((channels) =>
      channels.filter((channel) => channel.id !== channelId),
    );
    setDirty(true);
  }

  return {
    addNotificationChannel,
    updateNotificationChannel,
    removeNotificationChannel,
  };
}
