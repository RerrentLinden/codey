import { memo } from "react";
import {
  IconCheck as Check,
  IconDownload as Download,
  IconLoader2 as LoaderCircle,
  IconRefresh as RefreshCw,
} from "@tabler/icons-react";

import type { InlineResult, RuntimeStatus, UpdateCheck, UpdateDownload } from "./App.types";
import { Badge, Button, Card } from "./components/semi";

type AppUpdateCardProps = {
  status: RuntimeStatus;
  updateResult: InlineResult;
  updateCheck: UpdateCheck | null;
  downloadedUpdate: UpdateDownload | null;
  busy: string | null;
  isBusy: boolean;
  onCheckUpdates: () => void;
  onDownloadUpdate: () => void;
  onInstallUpdate: () => void;
};

function AppUpdateCardComponent({
  status,
  updateResult,
  updateCheck,
  downloadedUpdate,
  busy,
  isBusy,
  onCheckUpdates,
  onDownloadUpdate,
  onInstallUpdate,
}: AppUpdateCardProps) {
  const hasUpdate = updateCheck?.updateAvailable === true;
  return (
    <section className="secondary-section" aria-labelledby="update-title">
      <div className="section-title compact">
        <div>
          <h2 id="update-title">应用更新</h2>
          <p>检查软件版本与在线更新</p>
        </div>
      </div>
      <Card className="secondary-card update-card">
        <div className="update-card-header">
          <div className="update-card-title">
            <span
              className={`column-icon update-card-icon ${
                hasUpdate ? "has-update" : ""
              }`}
            >
              <RefreshCw size={16} />
              {hasUpdate && (
                <span
                  className="update-notification-dot"
                  aria-hidden="true"
                />
              )}
            </span>
            <div>
              <strong>应用更新</strong>
              <small>
                当前版本{" "}
                {status.appVersion ? `v${status.appVersion}` : "读取中"}
              </small>
            </div>
          </div>
          <Badge variant={hasUpdate ? "warning" : "secondary"}>
            {hasUpdate ? "发现新版本" : "已是最新"}
          </Badge>
        </div>

        <div className="update-card-content">
          <div className="update-status-msg">
            <span
              className={`inline-result ${updateResult.tone}`}
              aria-live="polite"
            >
              {updateResult.text || "从公开更新源检查最新稳定版本。"}
            </span>
          </div>
          <div className="update-actions-row">
            {downloadedUpdate ? (
              <Button
                variant="default"
                size="sm"
                disabled={isBusy}
                onClick={onInstallUpdate}
              >
                {busy === "install-update" ? (
                  <LoaderCircle className="spinner" aria-hidden="true" />
                ) : (
                  <Check aria-hidden="true" />
                )}
                安装并重启
              </Button>
            ) : updateCheck?.updateAvailable && updateCheck.selectedAsset ? (
              <Button
                variant="default"
                size="xs"
                disabled={isBusy}
                onClick={onDownloadUpdate}
              >
                {busy === "download-update" ? (
                  <LoaderCircle className="spinner" aria-hidden="true" />
                ) : (
                  <Download aria-hidden="true" />
                )}
                下载更新
              </Button>
            ) : null}
            <Button
              variant="secondary"
              size="sm"
              disabled={isBusy}
              onClick={onCheckUpdates}
            >
              {busy === "check-update" ? (
                <LoaderCircle className="spinner" aria-hidden="true" />
              ) : (
                <RefreshCw aria-hidden="true" />
              )}
              检查更新
            </Button>
          </div>
        </div>
      </Card>
    </section>
  );
}

export const AppUpdateCard = memo(AppUpdateCardComponent);
