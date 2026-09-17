import { memo } from "react";

import { Input } from "../components/ui";
import { inputShellClass, insetInputClass } from "../uiClasses";
import type { NotificationChannelEditorProps } from "./types";

function NtfyChannelEditorComponent({
  channel,
  disabled,
  onChange,
}: NotificationChannelEditorProps) {
  return (
    <>
      <label className="field notification-field-row">
        <span>服务器地址</span>
        <div className={inputShellClass}>
          <Input
            className={insetInputClass}
            type="text"
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
                : "https://ntfy.sh"
            }
            autoComplete="off"
            spellCheck={false}
          />
        </div>
      </label>
      <label className="field notification-field-row">
        <span>主题</span>
        <div className={inputShellClass}>
          <Input
            className={insetInputClass}
            value={channel.chatId}
            disabled={disabled}
            onChange={(event) => onChange({ chatId: event.target.value })}
            placeholder="自定义主题，例如 codey-notify"
            spellCheck={false}
          />
        </div>
      </label>
      <label className="field notification-field-row">
        <span>访问令牌</span>
        <div className={inputShellClass}>
          <Input
            className={insetInputClass}
            type="text"
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
                ? "已保存；输入新令牌可替换"
                : "公开主题可留空"
            }
            autoComplete="off"
            spellCheck={false}
          />
        </div>
      </label>
    </>
  );
}

export const NtfyChannelEditor = memo(NtfyChannelEditorComponent);
