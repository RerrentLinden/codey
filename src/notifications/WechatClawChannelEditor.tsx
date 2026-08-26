import { memo, useEffect, useRef, useState } from "react";
import {
  IconBrandWechat,
  IconLoader2 as LoaderCircle,
  IconQrcode,
} from "@tabler/icons-react";

import { invoke } from "../api";
import { errorText } from "../appUtils";
import { Button, Input } from "../components/mantine";
import { inputShellClass, insetInputClass } from "../uiClasses";
import type { NotificationChannel, NotificationChannelEditorProps } from "./types";

type WechatClawLoginStartResult = {
  loginId: string;
  status: "wait";
  qrCode?: string;
  qrCodeImageUrl?: string;
};

type WechatClawLoginPollResult = {
  status: "wait" | "scanned" | "confirmed" | "expired" | "failed";
  message?: string;
  baseUrl?: string;
  botToken?: string;
  recipientId?: string;
};

type ActiveLogin = {
  loginId: string;
  qrCodeImageUrl?: string;
  phase: "waiting" | "scanned" | "confirmed" | "expired" | "failed";
  message: string;
};

function WechatClawChannelEditorComponent({
  channel,
  disabled,
  onChange,
}: NotificationChannelEditorProps) {
  const [login, setLogin] = useState<ActiveLogin | null>(null);
  const [isStarting, setIsStarting] = useState(false);
  const mounted = useRef(true);
  const channelRef = useRef<NotificationChannel>(channel);
  const onChangeRef = useRef(onChange);
  channelRef.current = channel;
  onChangeRef.current = onChange;

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    const loginId = login?.loginId;
    if (!loginId) return;

    let cancelled = false;
    let timer: number | undefined;
    const scheduleNext = () => {
      // iLink can long-poll this request; the small delay only protects against
      // an immediate `wait` response and keeps the temporary binding flow quiet.
      timer = window.setTimeout(() => void poll(), 1_200);
    };
    const poll = async () => {
      try {
        const result = await invoke<WechatClawLoginPollResult>(
          "poll_wechat_claw_login",
          { loginId },
        );
        if (cancelled) return;

        if (result.status === "confirmed") {
          const token = result.botToken?.trim();
          const baseUrl = result.baseUrl?.trim();
          if (!token || !baseUrl) {
            setLogin((current) => current?.loginId === loginId
              ? { ...current, phase: "failed", message: "微信 ClawBot 没有返回完整的绑定凭据，请重新扫码" }
              : current);
            return;
          }
          onChangeRef.current({
            url: baseUrl,
            urlConfigured: true,
            clearUrl: false,
            botToken: token,
            botTokenConfigured: true,
            clearBotToken: false,
            chatId: result.recipientId?.trim() || channelRef.current.chatId,
          });
          setLogin((current) => current?.loginId === loginId
            ? { ...current, phase: "confirmed", message: "绑定成功。确认接收人 ID 后保存即可。" }
            : current);
          return;
        }

        if (result.status === "expired" || result.status === "failed") {
          setLogin((current) => current?.loginId === loginId
            ? {
              ...current,
              phase: result.status === "expired" ? "expired" : "failed",
              message: result.message || "微信 ClawBot 登录没有完成，请重新扫码",
            }
            : current);
          return;
        }

        setLogin((current) => current?.loginId === loginId
          ? {
            ...current,
            phase: result.status === "scanned" ? "scanned" : "waiting",
            message: result.message || (result.status === "scanned"
              ? "已扫码，请在微信中确认授权。"
              : "请使用微信扫描二维码。"),
          }
          : current);
        scheduleNext();
      } catch (error) {
        if (!cancelled) {
          setLogin((current) => current?.loginId === loginId
            ? { ...current, phase: "failed", message: errorText(error) }
            : current);
        }
      }
    };

    void poll();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [login?.loginId]);

  async function startLogin() {
    if (disabled || isStarting) return;
    setLogin(null);
    setIsStarting(true);
    try {
      const result = await invoke<WechatClawLoginStartResult>(
        "start_wechat_claw_login",
      );
      if (!mounted.current) return;
      setLogin({
        loginId: result.loginId,
        qrCodeImageUrl: result.qrCodeImageUrl,
        phase: "waiting",
        message: "请使用微信扫描二维码，并在手机上确认授权。",
      });
    } catch (error) {
      if (mounted.current) {
        setLogin({
          loginId: "",
          phase: "failed",
          message: errorText(error),
        });
      }
    } finally {
      if (mounted.current) setIsStarting(false);
    }
  }

  const isBound = Boolean(channel.botToken.trim() || channel.botTokenConfigured);
  const loginMessage = login?.message || (isBound
    ? "当前已绑定。重新扫码会替换已保存的登录凭据。"
    : "扫描后会自动填写登录凭据和当前微信的接收人 ID。二维码 10 分钟内有效。");

  return (
    <>
      <div className="rounded-[10px] border border-[#07c160]/25 bg-[#f2fff5] p-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <span className="grid size-7 place-items-center rounded-full bg-[#07c160]/12 text-[#07a854]">
              <IconQrcode size={17} aria-hidden="true" />
            </span>
            <div>
              <strong className="block text-xs text-[#1d1d1f]">微信 ClawBot 绑定</strong>
              <span className="block text-[11px] text-[#5d6b61]">无需企业微信机器人或常驻转发服务</span>
            </div>
          </div>
          <Button
            variant="secondary"
            size="xs"
            disabled={disabled || isStarting}
            onClick={() => void startLogin()}
          >
            {isStarting ? <LoaderCircle className="animate-spin" aria-hidden="true" /> : <IconBrandWechat aria-hidden="true" />}
            {isStarting ? "正在生成" : isBound ? "重新扫码" : "扫码绑定"}
          </Button>
        </div>
        <p className="mt-2 text-[11px] leading-5 text-[#526158]" role="status" aria-live="polite">
          {loginMessage}
        </p>
        {login?.qrCodeImageUrl && (login.phase === "waiting" || login.phase === "scanned") ? (
          <div className="mt-3 flex justify-center rounded-lg bg-white p-2">
            <img
              className="size-48 rounded-md object-contain"
              src={login.qrCodeImageUrl}
              alt="微信 ClawBot 登录二维码"
              referrerPolicy="no-referrer"
              onError={() => {
                setLogin((current) => current?.loginId === login.loginId
                  ? { ...current, phase: "failed", message: "二维码加载失败，请重新生成后重试。" }
                  : current);
              }}
            />
          </div>
        ) : null}
      </div>

      <label className="field">
        <span>接收通知的 iLink 用户 ID</span>
        <div className={inputShellClass}>
          <IconBrandWechat size={15} aria-hidden="true" />
          <Input
            className={insetInputClass}
            value={channel.chatId}
            disabled={disabled}
            onChange={(event) => onChange({ chatId: event.target.value })}
            placeholder="扫码后自动填入，可按需修改"
            spellCheck={false}
          />
        </div>
      </label>

      {isBound ? (
        <div className="-mt-[7px] flex justify-end">
          <Button
            className="text-[#8e8e93] hover:text-[#d70015]"
            variant="ghost"
            size="xs"
            disabled={disabled}
            onClick={() => {
              setLogin(null);
              onChange({
                url: "",
                urlConfigured: false,
                clearUrl: false,
                botToken: "",
                botTokenConfigured: false,
                clearBotToken: true,
              });
            }}
          >
            解除已保存绑定
          </Button>
        </div>
      ) : null}
    </>
  );
}

export const WechatClawChannelEditor = memo(WechatClawChannelEditorComponent);
