import type { ReactNode } from "react";
import { Modal } from "@mantine/core";

import { SETTINGS_OVERLAY_Z_INDEX } from "./overlay.constants";

type SettingsModalShellProps = {
  afterClose?: () => void;
  children: ReactNode;
  container?: HTMLElement | null;
  header?: ReactNode;
  onCancel: () => void;
  title?: ReactNode;
  visible: boolean;
};

export function CodeyBrandMark() {
  return (
    <svg
      className="block size-[38px] rounded-[10px] text-[#007aff] shadow-[0_1px_2px_rgba(0,122,255,0.12),0_4px_12px_rgba(0,122,255,0.14)] max-[760px]:size-8"
      viewBox="0 0 350 350"
      aria-hidden="true"
      focusable="false"
    >
      <defs>
        <linearGradient
          id="codey-brand-mark-gradient"
          x1="0"
          x2="1"
          y1="0"
          y2="1"
        >
          <stop offset="0%" stopColor="#ffffff" />
          <stop offset="100%" stopColor="#e3efff" />
        </linearGradient>
      </defs>
      <rect
        x="0"
        y="0"
        width="350"
        height="350"
        rx="34"
        fill="url(#codey-brand-mark-gradient)"
      />
      <path
        d="M70 301c-16 0-24-18-13-30l73-77c8-8 8-20 0-28L65 101C50 86 57 61 78 57c9-2 18 1 25 8l91 91c18 18 18 46 0 64l-66 66c-6 6-2 15 7 15h183"
        fill="none"
        stroke="currentColor"
        strokeLinecap="round"
        strokeLinejoin="round"
        strokeWidth="22"
      />
    </svg>
  );
}

export function SettingsModalShell({
  afterClose,
  children,
  container,
  header,
  onCancel,
  title,
  visible,
}: SettingsModalShellProps) {
  return (
    <Modal
      centered
      classNames={{
        body: "m-0 flex min-h-0 flex-1 flex-col overflow-hidden! p-0",
        content:
          "flex! h-[min(860px,calc(100dvh_-_24px))]! max-h-[calc(100dvh_-_24px)]! max-w-[calc(100vw_-_24px)] flex-col overflow-hidden! p-0 max-[760px]:h-[calc(100dvh_-_12px)]! max-[760px]:max-h-[calc(100dvh_-_12px)]! max-[760px]:max-w-[calc(100vw_-_12px)]",
        header:
          "m-0 min-h-0! flex-none border-b border-gray-200 px-5! py-2.5! max-[760px]:px-3.5! max-[760px]:py-2!",
        inner: "p-3! max-[760px]:p-1.5!",
        overlay: "bg-black/25 backdrop-blur-[20px]",
        root: "[-webkit-app-region:no-drag]",
        title: "min-w-0 flex-1",
      }}
      closeButtonProps={{ "aria-label": "关闭配置" }}
      closeOnClickOutside={false}
      closeOnEscape={false}
      onClose={onCancel}
      onExitTransitionEnd={afterClose}
      opened={visible}
      overlayProps={{ backgroundOpacity: 0.25, blur: 20 }}
      padding={0}
      lockScroll={false}
      portalProps={container ? { target: container } : undefined}
      size={1040}
      title={header ?? title}
      withCloseButton={header === undefined}
      withinPortal={Boolean(container)}
      zIndex={SETTINGS_OVERLAY_Z_INDEX}
    >
      {children}
    </Modal>
  );
}
