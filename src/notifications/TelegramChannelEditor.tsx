import { memo } from "react";
import {
  IconBrandTelegram,
  IconKey,
} from "@tabler/icons-react";

import { Button, Input } from "../components/semi";
import type { NotificationChannelEditorProps } from "./types";

function TelegramChannelEditorComponent({
  channel,
  disabled,
  revealSecrets = false,
  onChange,
}: NotificationChannelEditorProps) {
  return (
    <>
      <label className="field">
        <span>Bot Token</span>
        <div className="input-shell">
          <IconKey size={15} aria-hidden="true" />
          <Input
            type={revealSecrets ? "text" : "password"}
            value={channel.botToken}
            disabled={disabled}
            onChange={(event) =>
              onChange({
                botToken: event.target.value,
                clearBotToken: false,
              })
            }
            placeholder={
              channel.botTokenConfigured
                ? "已保存；输入新 Token 可替换"
                : "从 BotFather 获取"
            }
            autoComplete="new-password"
            spellCheck={false}
          />
        </div>
      </label>
      {channel.botTokenConfigured ? (
        <div className="notification-secret-action">
          <Button
            variant="ghost"
            size="xs"
            disabled={disabled}
            onClick={() =>
              onChange({
                botToken: "",
                botTokenConfigured: false,
                clearBotToken: true,
              })
            }
          >
            清除已保存 Token
          </Button>
        </div>
      ) : null}
      <label className="field">
        <span>Chat ID</span>
        <div className="input-shell">
          <IconBrandTelegram size={15} aria-hidden="true" />
          <Input
            value={channel.chatId}
            disabled={disabled}
            onChange={(event) => onChange({ chatId: event.target.value })}
            placeholder="-1001234567890"
            spellCheck={false}
          />
        </div>
      </label>
    </>
  );
}

export const TelegramChannelEditor = memo(TelegramChannelEditorComponent);
