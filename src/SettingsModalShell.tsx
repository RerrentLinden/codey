import type { ReactNode } from "react";
import SemiModal from "@douyinfe/semi-ui/lib/es/modal";

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
      className="config-brand-mark"
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
  const headingProps = header === undefined ? { title } : { header };
  return (
    <SemiModal
      {...headingProps}
      afterClose={afterClose}
      centered
      className="codey-settings-modal-layer"
      closeOnEsc={false}
      closable={header === undefined}
      footer={null}
      getPopupContainer={container ? () => container : undefined}
      mask
      maskClosable={false}
      modalContentClass="codey-settings-modal-content"
      onCancel={onCancel}
      visible={visible}
      width={1040}
      zIndex={SETTINGS_OVERLAY_Z_INDEX}
    >
      {children}
    </SemiModal>
  );
}
