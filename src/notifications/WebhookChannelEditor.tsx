import { memo } from "react";
import { IconSend } from "@tabler/icons-react";

import { Button, Input } from "../components/mantine";
import { inputShellClass, insetInputClass } from "../uiClasses";
import type { NotificationChannelEditorProps } from "./types";

export function createWebhookChannelEditor(emptyPlaceholder: string) {
  function WebhookChannelEditor({
    channel,
    disabled,
    revealSecrets = false,
    onChange,
  }: NotificationChannelEditorProps) {
    return (
      <>
        <label className="field">
          <span>Webhook 地址</span>
          <div className={inputShellClass}>
            <IconSend size={15} aria-hidden="true" />
            <Input
              className={insetInputClass}
              type={revealSecrets ? "text" : "password"}
              value={channel.url}
              disabled={disabled}
              onChange={(event) =>
                onChange({
                  url: event.target.value,
                  clearUrl: false,
                })
              }
              placeholder={
                channel.urlConfigured
                  ? "已保存；输入新地址可替换"
                  : emptyPlaceholder
              }
              autoComplete="new-password"
              spellCheck={false}
            />
          </div>
        </label>
        {channel.urlConfigured ? (
          <div className="-mt-[7px] flex justify-end">
            <Button
              className="text-[#8e8e93] hover:text-[#d70015]"
              variant="ghost"
              size="xs"
              disabled={disabled}
              onClick={() =>
                onChange({
                  url: "",
                  urlConfigured: false,
                  clearUrl: true,
                })
              }
            >
              清除已保存地址
            </Button>
          </div>
        ) : null}
      </>
    );
  }

  return memo(WebhookChannelEditor);
}
