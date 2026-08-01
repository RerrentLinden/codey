import { memo } from "react";
import {
  IconAlertCircle as CircleAlert,
  IconDatabase as Database,
  IconFileDescription as FileText,
  IconListDetails as ListDetails,
  IconLoader2 as LoaderCircle,
  IconRefresh as RefreshCw,
  IconTrash as Trash2,
} from "@tabler/icons-react";

import { Badge, Button, Card } from "./components/semi";

export type TraceLogStats = {
  pending: boolean;
  capturedAt: number;
  databasesFound: number;
  databasesScanned: number;
  databaseBytes: number;
  rowCount: number;
  estimatedLogBytes: number;
  oldestTimestamp?: number;
  newestTimestamp?: number;
  errors: string[];
};

type TraceLogModuleProps = {
  stats?: TraceLogStats;
  snapshotStale: boolean;
  protectionEnabled: boolean;
  clearBusy: boolean;
  refreshing: boolean;
  disabled: boolean;
  onClear: () => void;
  onRefresh: () => void;
};

const countFormatter = new Intl.NumberFormat("zh-CN");
const snapshotTimeFormatter = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});
const rangeDateFormatter = new Intl.DateTimeFormat("zh-CN", {
  month: "2-digit",
  day: "2-digit",
});

export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / (1024 ** index);
  return `${value >= 10 || index === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[index]}`;
}

function formatCount(value: number): string {
  return countFormatter.format(Number.isFinite(value) ? value : 0);
}

function formatSnapshotTime(timestamp: number): string {
  if (!timestamp) return "本次启动";
  return snapshotTimeFormatter.format(new Date(timestamp * 1000));
}

function formatRange(stats: TraceLogStats): string {
  if (!stats.oldestTimestamp || !stats.newestTimestamp) return "暂无日志时间范围";
  return `${rangeDateFormatter.format(new Date(stats.oldestTimestamp * 1000))} - ${rangeDateFormatter.format(new Date(stats.newestTimestamp * 1000))}`;
}

function TraceLogModuleComponent({
  stats,
  snapshotStale,
  protectionEnabled,
  clearBusy,
  refreshing,
  disabled,
  onClear,
  onRefresh,
}: TraceLogModuleProps) {
  const loading = refreshing || Boolean(stats?.pending);
  const snapshot = stats && stats.capturedAt > 0 && !stats.pending ? stats : undefined;

  return (
    <section className="trace-section" aria-labelledby="trace-title">
      <div className="section-title compact trace-section-title">
        <div>
          <span className="section-kicker">Diagnostics</span>
          <h2 id="trace-title">Trace 日志分析</h2>
          <p>按需快照 · 日志诊断</p>
        </div>
        <div className="trace-module-actions">
          <Badge variant={protectionEnabled ? "success" : "secondary"}>
            {protectionEnabled ? "写盘保护已开启" : "写盘保护关闭"}
          </Badge>
          <Button
            className="trace-refresh-button"
            variant="outline"
            size="sm"
            disabled={disabled}
            onClick={onRefresh}
          >
            <RefreshCw className={loading ? "spinner" : ""} aria-hidden="true" />
            刷新统计
          </Button>
          <Button
            className="trace-clear-button"
            variant="destructive"
            size="sm"
            disabled={disabled}
            onClick={onClear}
          >
            {clearBusy
              ? <LoaderCircle className="spinner" aria-hidden="true" />
              : <Trash2 aria-hidden="true" />}
            清理日志库
          </Button>
        </div>
      </div>

      <Card className={`trace-card${snapshot ? "" : " trace-card-empty"}`} aria-busy={loading}>
        {!snapshot ? (
          <div className="trace-empty-container">
            <div className="trace-empty" role="status" aria-live="polite">
              <div className="trace-empty-badge">
                <span className="trace-empty-icon">
                  {loading
                    ? <LoaderCircle className="spinner" size={28} aria-hidden="true" />
                    : <RefreshCw size={26} aria-hidden="true" />}
                </span>
              </div>
              <div className="trace-empty-copy">
                <h3>{loading ? "正在统计 Trace 日志" : "未获取 Diagnostic/Trace 诊断快照"}</h3>
                <p>
                  {loading
                    ? "正在扫描本地 Codex 日志库及数据库索引，请稍候…"
                    : "一键扫描本地 logs_*.sqlite 日志数据库，统计日志总量、磁盘占用与内容字节。"}
                </p>
              </div>
              <div className="trace-empty-action">
                <Button
                  variant="default"
                  size="default"
                  disabled={disabled}
                  onClick={onRefresh}
                  className="trace-start-btn"
                >
                  {loading ? (
                    <>
                      <LoaderCircle className="spinner" aria-hidden="true" />
                      扫描分析中…
                    </>
                  ) : (
                    <>
                      <RefreshCw aria-hidden="true" />
                      立即生成诊断快照
                    </>
                  )}
                </Button>
              </div>
            </div>
          </div>
        ) : (
          <>
            <div className="trace-snapshot-row">
              <div className="trace-snapshot-info">
                <span className={`trace-status-dot ${protectionEnabled ? "active" : ""}`} />
                <strong>{protectionEnabled ? "保护状态正常" : "写盘保护未开启"}</strong>
                <span>{snapshot.databasesScanned}/{snapshot.databasesFound} 个日志数据库已完成扫描</span>
              </div>
              <Badge variant={snapshot.errors.length || snapshotStale ? "warning" : "secondary"}>
                {snapshotStale ? "清理前快照 · " : ""}{formatSnapshotTime(snapshot.capturedAt)}
              </Badge>
            </div>

            <div className="trace-metrics-grid">
              <div className="trace-metric-card trace-metric-card-rows">
                <span className="trace-metric-icon" aria-hidden="true">
                  <ListDetails size={20} />
                </span>
                <div className="trace-metric-content">
                  <span>日志总条数</span>
                  <strong>{formatCount(snapshot.rowCount)}</strong>
                  <small>{formatRange(snapshot)}</small>
                </div>
              </div>
              <div className="trace-metric-card trace-metric-card-storage">
                <span className="trace-metric-icon" aria-hidden="true">
                  <Database size={20} />
                </span>
                <div className="trace-metric-content">
                  <span>磁盘占用空间</span>
                  <strong>{formatBytes(snapshot.databaseBytes)}</strong>
                  <small>主数据库及 WAL/SHM</small>
                </div>
              </div>
              <div className="trace-metric-card trace-metric-card-content">
                <span className="trace-metric-icon" aria-hidden="true">
                  <FileText size={20} />
                </span>
                <div className="trace-metric-content">
                  <span>内容字节估算</span>
                  <strong>{formatBytes(snapshot.estimatedLogBytes)}</strong>
                  <small>按 estimated_bytes 汇总</small>
                </div>
              </div>
            </div>

            {snapshot.errors.length > 0 && (
              <div className="trace-warning" title={snapshot.errors.join("\n")}>
                <CircleAlert size={15} />
                <span>{snapshot.errors.length} 个日志库统计异常，已保留其余快照数据</span>
              </div>
            )}
          </>
        )}
      </Card>
    </section>
  );
}

export const TraceLogModule = memo(TraceLogModuleComponent);
