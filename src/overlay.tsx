import ReactDOM from "react-dom/client";
import { MantineProvider } from "@mantine/core";
import mantineStyles from "@mantine/core/styles.css?inline";
import { App } from "./App";
import coreStyles from "./styles.css?inline";
import operationsStyles from "./styles.operations.css?inline";
import modelStyles from "./styles.models.css?inline";
import featureStyles from "./styles.features.css?inline";
import diagnosticStyles from "./styles.diagnostics.css?inline";
import responsiveStyles from "./styles.responsive.css?inline";
import { codeyApiPath } from "./api";
import { SETTINGS_OVERLAY_Z_INDEX_CSS } from "./overlay.constants";
import { SETTINGS_OPENED_EVENT } from "./useRuntimeStatus";
import { codeyMantineTheme } from "./mantine";
import tailwindStyles from "./tailwind.css?inline";

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
    __codeySettingsOverlay?: OverlayController;
  }
}

function isElement(target: EventTarget): target is HTMLElement {
  return target instanceof HTMLElement;
}

function canScrollVertically(element: HTMLElement, deltaY: number) {
  const maxScrollTop = element.scrollHeight - element.clientHeight;
  if (maxScrollTop <= 0 || deltaY === 0) return false;

  const { overflowY } = window.getComputedStyle(element);
  if (!/(auto|scroll|overlay)/.test(overflowY)) return false;

  return deltaY < 0
    ? element.scrollTop > 0
    : element.scrollTop < maxScrollTop;
}

function getOverlayMountTarget() {
  return document.body ?? document.documentElement;
}

window.__codeyInvokeApi = async (command, args) => {
  if (typeof window.__codexSessionDeleteBridge !== "function") {
    throw new Error("Codey bridge 尚未就绪");
  }
  return window.__codexSessionDeleteBridge(codeyApiPath(command), args);
};

if (!window.__codeySettingsOverlay) {
  const host = document.createElement("div");
  host.id = "codey-settings-overlay-host";
  host.style.display = "none";
  host.style.setProperty("inset", "0", "important");
  host.style.setProperty("position", "fixed", "important");
  host.style.setProperty(
    "--codey-settings-overlay-z-index",
    SETTINGS_OVERLAY_Z_INDEX_CSS,
  );
  host.style.setProperty(
    "z-index",
    SETTINGS_OVERLAY_Z_INDEX_CSS,
    "important",
  );
  host.style.setProperty("background", "transparent", "important");
  host.setAttribute("data-mantine-color-scheme", "light");
  host.setAttribute("aria-hidden", "true");
  const shadow = host.attachShadow({ mode: "open" });
  const style = document.createElement("style");
  style.textContent = [
    mantineStyles,
    tailwindStyles,
    coreStyles,
    operationsStyles,
    modelStyles,
    featureStyles,
    diagnosticStyles,
    responsiveStyles,
  ].join("\n");
  const rootElement = document.createElement("div");
  rootElement.id = "codey-overlay-root";
  rootElement.style.inset = "0";
  rootElement.style.pointerEvents = "none";
  rootElement.style.position = "fixed";
  rootElement.style.width = "100%";
  rootElement.setAttribute("data-mantine-color-scheme", "light");
  const modalContainer = document.createElement("div");
  modalContainer.id = "codey-overlay-modal-container";
  modalContainer.style.inset = "0";
  modalContainer.style.position = "fixed";
  modalContainer.style.width = "100%";
  modalContainer.setAttribute("data-mantine-color-scheme", "light");
  shadow.append(style, rootElement, modalContainer);
  getOverlayMountTarget().appendChild(host);

  let hideTimer: number | undefined;
  let visible = false;

  const handleOverlayWheel = (event: WheelEvent) => {
    if (!visible) return;

    const path = event.composedPath();
    const nestedScrollable = path.find(
      (target) =>
        isElement(target) &&
        !target.classList.contains("page-scroll") &&
        canScrollVertically(target, event.deltaY),
    );
    if (nestedScrollable) {
      event.stopPropagation();
      return;
    }

    const scrollContainer =
      path.find(
        (target): target is HTMLElement =>
          isElement(target) && target.classList.contains("page-scroll"),
      ) ?? modalContainer.querySelector<HTMLElement>(".page-scroll");

    event.stopPropagation();

    if (!scrollContainer) return;

    const maxScrollTop =
      scrollContainer.scrollHeight - scrollContainer.clientHeight;
    if (maxScrollTop <= 0) return;

    event.preventDefault();
    scrollContainer.scrollTop = Math.min(
      maxScrollTop,
      Math.max(0, scrollContainer.scrollTop + event.deltaY),
    );
  };
  modalContainer.addEventListener("wheel", handleOverlayWheel, {
    capture: true,
    passive: false,
  });

  const hide = () => {
    window.clearTimeout(hideTimer);
    hideTimer = undefined;
    host.style.display = "none";
    host.setAttribute("aria-hidden", "true");
  };
  const reactRoot = ReactDOM.createRoot(rootElement);
  const render = (visible: boolean) => {
    reactRoot.render(
      <MantineProvider
        cssVariablesSelector=":host"
        forceColorScheme="light"
        getRootElement={() => host}
        theme={codeyMantineTheme}
      >
        <App
          embedded
          modalContainer={modalContainer}
          modalVisible={visible}
          onAfterClose={hide}
          onClose={close}
        />
      </MantineProvider>,
    );
  };
  const close = () => {
    if (!visible) return;
    visible = false;
    render(false);
    window.clearTimeout(hideTimer);
    hideTimer = window.setTimeout(hide, 250);
  };
  const open = () => {
    if (visible) return;
    visible = true;
    window.clearTimeout(hideTimer);
    hideTimer = undefined;
    getOverlayMountTarget().appendChild(host);
    host.style.display = "block";
    host.setAttribute("aria-hidden", "false");
    render(true);
    window.dispatchEvent(new CustomEvent(SETTINGS_OPENED_EVENT));
  };
  const isOpen = () => visible;

  render(false);
  window.__codeySettingsOverlay = {
    open,
    close,
    isOpen,
    toggle: open,
  };
}
