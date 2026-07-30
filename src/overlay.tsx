import ReactDOM from "react-dom/client";
import "../node_modules/@douyinfe/semi-ui/lib/es/_base/base.css";
import { App } from "./App";
import appStyles from "./styles.css?inline";
import overlayStyles from "./overlay.css?inline";
import { codeyApiPath } from "./api";

const SETTINGS_OPENED_EVENT = "codey-settings-opened";

type OverlayController = {
  open: () => void;
  close: () => void;
  toggle: () => void;
  isOpen: () => boolean;
};

declare global {
  interface Window {
    __codexSessionDeleteBridge?: (
      path: string,
      payload: unknown,
    ) => Promise<unknown>;
    __codeyComponentStyles?: string;
    __codeySettingsOverlay?: OverlayController;
  }
}

window.__codeyInvokeApi = async (command, args) => {
  if (typeof window.__codexSessionDeleteBridge !== "function") {
    throw new Error("Codey bridge 尚未就绪");
  }
  return window.__codexSessionDeleteBridge(codeyApiPath(command), args);
};

if (!window.__codeySettingsOverlay) {
  const componentStyles = window.__codeyComponentStyles ?? "";
  delete window.__codeyComponentStyles;
  const host = document.createElement("div");
  host.id = "codey-settings-overlay-host";
  host.style.display = "none";
  host.setAttribute("aria-hidden", "true");
  const shadow = host.attachShadow({ mode: "open" });
  const style = document.createElement("style");
  style.textContent = `${componentStyles}\n${overlayStyles}\n${appStyles}`;
  const backdrop = document.createElement("div");
  backdrop.className = "codey-overlay-backdrop";
  const dialog = document.createElement("section");
  dialog.className = "codey-overlay-dialog";
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  dialog.setAttribute("aria-label", "Codey 配置");
  dialog.tabIndex = -1;
  const rootElement = document.createElement("div");
  rootElement.id = "codey-overlay-root";
  dialog.appendChild(rootElement);
  backdrop.appendChild(dialog);
  shadow.append(style, backdrop);
  document.documentElement.appendChild(host);

  const close = () => {
    host.style.display = "none";
    host.setAttribute("aria-hidden", "true");
  };
  const open = () => {
    host.style.display = "block";
    host.setAttribute("aria-hidden", "false");
    window.dispatchEvent(new CustomEvent(SETTINGS_OPENED_EVENT));
    requestAnimationFrame(() => dialog.focus({ preventScroll: true }));
  };
  const isOpen = () => host.style.display !== "none";

  ReactDOM.createRoot(rootElement).render(<App embedded onClose={close} />);
  window.__codeySettingsOverlay = {
    open,
    close,
    isOpen,
    toggle: open,
  };
}
