import ReactDOM from "react-dom/client";
import "../node_modules/@douyinfe/semi-ui/lib/es/_base/base.css";
import { App } from "./App";
import appStyles from "./styles.css?inline";
import overlayStyles from "./overlay.css?inline";
import { codeyApiPath } from "./api";
import { SETTINGS_OPENED_EVENT } from "./useRuntimeStatus";

const SETTINGS_OVERLAY_Z_INDEX = "2147483647";

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
  host.style.setProperty("inset", "0", "important");
  host.style.setProperty("position", "fixed", "important");
  host.style.setProperty("z-index", SETTINGS_OVERLAY_Z_INDEX, "important");
  host.setAttribute("aria-hidden", "true");
  const shadow = host.attachShadow({ mode: "open" });
  const style = document.createElement("style");
  style.textContent = `${componentStyles}\n${overlayStyles}\n${appStyles}`;
  const rootElement = document.createElement("div");
  rootElement.id = "codey-overlay-root";
  const modalContainer = document.createElement("div");
  modalContainer.id = "codey-overlay-modal-container";
  shadow.append(style, rootElement, modalContainer);
  document.documentElement.appendChild(host);

  let hideTimer: number | undefined;
  const hide = () => {
    window.clearTimeout(hideTimer);
    hideTimer = undefined;
    host.style.display = "none";
    host.setAttribute("aria-hidden", "true");
  };
  const reactRoot = ReactDOM.createRoot(rootElement);
  const render = (visible: boolean) => {
    reactRoot.render(
      <App
        embedded
        modalContainer={modalContainer}
        modalVisible={visible}
        onAfterClose={hide}
        onClose={close}
      />,
    );
  };
  const close = () => {
    render(false);
    window.clearTimeout(hideTimer);
    hideTimer = window.setTimeout(hide, 250);
  };
  const open = () => {
    window.clearTimeout(hideTimer);
    hideTimer = undefined;
    document.documentElement.appendChild(host);
    host.style.display = "block";
    host.setAttribute("aria-hidden", "false");
    render(true);
    window.dispatchEvent(new CustomEvent(SETTINGS_OPENED_EVENT));
  };
  const isOpen = () => host.style.display !== "none";

  render(false);
  window.__codeySettingsOverlay = {
    open,
    close,
    isOpen,
    toggle: open,
  };
}
