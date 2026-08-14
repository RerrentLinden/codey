import {
  type Dispatch,
  type SetStateAction,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";

import { invoke } from "./api";
import type {
  Confirmation,
  InlineResult,
  Notice,
  UpdateCheck,
  UpdateDownload,
} from "./App.types";
import { errorText, withTimeout } from "./appUtils";
import { formatBytes } from "./formatters";

const UPDATE_AVAILABLE_EVENT = "codey-update-availability-changed";
const AUTO_UPDATE_CHECK_INTERVAL_MS = 30 * 60 * 1000;
const UPDATE_CHECK_TIMEOUT_MS = 12_000;

declare global {
  interface Window {
    __codeyUpdateAvailability?: UpdateCheck | null;
  }
}

type UseAppUpdatesOptions = {
  embedded: boolean;
  configLoaded: boolean;
  isBusy: boolean;
  setBusy: Dispatch<SetStateAction<string | null>>;
  setNotice: Dispatch<SetStateAction<Notice>>;
  setConfirmation: Dispatch<SetStateAction<Confirmation | null>>;
  beforeInstall: () => Promise<void>;
};

const updateAvailable = (
  check: UpdateCheck | null | undefined,
): check is UpdateCheck => check?.updateAvailable === true;

function updateCheckText(result: UpdateCheck) {
  return result.updateAvailable
    ? result.selectedAsset
      ? `发现 v${result.latestVersion} 更新（当前 v${result.currentVersion}）`
      : `发现 v${result.latestVersion} 更新，但当前系统暂无可安装包`
    : `当前已是最新版本 v${result.currentVersion}`;
}

function updateResultTone(result: UpdateCheck): InlineResult["tone"] {
  return result.updateAvailable && !result.selectedAsset
    ? "error"
    : "success";
}

function publishUpdateAvailability(result: UpdateCheck | null) {
  window.__codeyUpdateAvailability = updateAvailable(result) ? result : null;
  window.dispatchEvent(
    new CustomEvent(UPDATE_AVAILABLE_EVENT, {
      detail: window.__codeyUpdateAvailability,
    }),
  );
}

export function useAppUpdates({
  embedded,
  configLoaded,
  isBusy,
  setBusy,
  setNotice,
  setConfirmation,
  beforeInstall,
}: UseAppUpdatesOptions) {
  const [updateResult, setUpdateResult] = useState<InlineResult>({
    tone: "idle",
    text: "",
  });
  const [updateCheck, setUpdateCheck] = useState<UpdateCheck | null>(null);
  const [downloadedUpdate, setDownloadedUpdate] =
    useState<UpdateDownload | null>(null);
  const updateCheckRef = useRef<UpdateCheck | null>(null);
  const updateCheckInFlightRef = useRef<Promise<UpdateCheck> | null>(null);
  const requestUpdateCheck = useCallback(() => {
    const current = updateCheckInFlightRef.current;
    if (current) return current;
    const request = withTimeout(
      invoke<UpdateCheck>("check_for_updates"),
      UPDATE_CHECK_TIMEOUT_MS,
      "检查更新超时，请检查网络",
    ).finally(() => {
      if (updateCheckInFlightRef.current === request) {
        updateCheckInFlightRef.current = null;
      }
    });
    updateCheckInFlightRef.current = request;
    return request;
  }, []);

  useEffect(() => {
    updateCheckRef.current = updateCheck;
  }, [updateCheck]);

  useEffect(() => {
    const applyDetectedUpdate = (
      result: UpdateCheck | null | undefined,
    ) => {
      if (!updateAvailable(result)) return;
      setUpdateCheck(result);
      setDownloadedUpdate(null);
      setUpdateResult({
        tone: updateResultTone(result),
        text: updateCheckText(result),
      });
    };

    applyDetectedUpdate(window.__codeyUpdateAvailability);
    const handleUpdateAvailabilityChanged = (event: Event) => {
      applyDetectedUpdate((event as CustomEvent<UpdateCheck | null>).detail);
    };
    window.addEventListener(
      UPDATE_AVAILABLE_EVENT,
      handleUpdateAvailabilityChanged,
    );
    return () => {
      window.removeEventListener(
        UPDATE_AVAILABLE_EVENT,
        handleUpdateAvailabilityChanged,
      );
    };
  }, []);

  useEffect(() => {
    if (embedded || !configLoaded) return;
    let cancelled = false;
    let timer = 0;

    const shouldPause = () =>
      updateAvailable(updateCheckRef.current) ||
      updateAvailable(window.__codeyUpdateAvailability);

    const schedule = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        timer = 0;
        void checkForUpdatesSilently();
      }, AUTO_UPDATE_CHECK_INTERVAL_MS);
    };

    const checkForUpdatesSilently = async () => {
      if (cancelled || shouldPause()) return;
      setUpdateResult({ tone: "pending", text: "正在检查更新…" });
      try {
        const result = await requestUpdateCheck();
        if (cancelled) return;
        if (result.updateAvailable) {
          setUpdateCheck(result);
          setDownloadedUpdate(null);
          setUpdateResult({
            tone: updateResultTone(result),
            text: updateCheckText(result),
          });
          publishUpdateAvailability(result);
          return;
        }
        setUpdateResult({
          tone: "success",
          text: updateCheckText(result),
        });
      } catch {
        if (!cancelled) setUpdateResult({ tone: "idle", text: "" });
        // 更新地址不可达或检查超时时直接跳过；手动检查仍会展示具体错误。
      } finally {
        if (!cancelled && !shouldPause()) schedule();
      }
    };

    void checkForUpdatesSilently();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [configLoaded, embedded, requestUpdateCheck]);

  async function checkForUpdates() {
    if (!configLoaded || isBusy) return;
    setBusy("check-update");
    setUpdateResult({ tone: "pending", text: "正在检查更新…" });
    setUpdateCheck(null);
    setDownloadedUpdate(null);
    try {
      const result = await requestUpdateCheck();
      setUpdateCheck(result);
      publishUpdateAvailability(result);
      const text = updateCheckText(result);
      setUpdateResult({
        tone: updateResultTone(result),
        text,
      });
      setNotice({
        tone:
          result.updateAvailable && result.selectedAsset
            ? "info"
            : result.updateAvailable
              ? "error"
              : "success",
        text,
      });
    } catch (error) {
      const text = errorText(error);
      setUpdateResult({ tone: "error", text });
      setNotice({ tone: "error", text });
    } finally {
      setBusy(null);
    }
  }

  async function downloadUpdate() {
    if (
      !configLoaded ||
      isBusy ||
      !updateCheck?.updateAvailable ||
      !updateCheck.selectedAsset
    )
      return;
    setBusy("download-update");
    setDownloadedUpdate(null);
    setUpdateResult({ tone: "pending", text: "正在下载并校验更新…" });
    try {
      const result = await withTimeout(
        invoke<UpdateDownload>("download_update"),
        300_000,
        "下载更新超时，请稍后重试",
      );
      setDownloadedUpdate(result);
      const text = `已下载 ${result.fileName}（${formatBytes(result.size)}），校验通过`;
      setUpdateResult({ tone: "success", text });
      setNotice({ tone: "success", text });
    } catch (error) {
      const text = errorText(error);
      setUpdateResult({ tone: "error", text });
      setNotice({ tone: "error", text });
    } finally {
      setBusy(null);
    }
  }

  function askInstallDownloadedUpdate() {
    if (!downloadedUpdate || isBusy) return;
    setConfirmation({
      action: "install-update",
      title: "安装更新",
      description: `Codey 会先保存未保存的设置，再退出当前实例，安装 ${downloadedUpdate.fileName}，然后尝试启动新版。`,
      confirmLabel: "安装并重启",
      run: () => void installDownloadedUpdate(),
    });
  }

  async function installDownloadedUpdate() {
    if (!downloadedUpdate || isBusy) return;
    setBusy("install-update");
    setUpdateResult({ tone: "pending", text: "正在启动安装器…" });
    try {
      await beforeInstall();
      await invoke("install_downloaded_update", {
        filePath: downloadedUpdate.filePath,
      });
      const text = "正在退出 Codey 并启动安装器…";
      setUpdateResult({ tone: "pending", text });
      setNotice({ tone: "info", text });
    } catch (error) {
      const text = errorText(error);
      setUpdateResult({ tone: "error", text });
      setNotice({ tone: "error", text });
      setBusy(null);
    }
  }

  return {
    updateResult,
    updateCheck,
    downloadedUpdate,
    checkForUpdates,
    downloadUpdate,
    askInstallDownloadedUpdate,
  };
}
