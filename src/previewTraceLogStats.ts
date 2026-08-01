import type { TraceLogStats } from "./TraceLogModule";

const capturedAt = Math.floor(Date.now() / 1000);

export const previewTraceLogStats: TraceLogStats = {
  pending: false,
  capturedAt,
  databasesFound: 2,
  databasesScanned: 2,
  databaseBytes: 903634944,
  rowCount: 318757,
  estimatedLogBytes: 219676672,
  oldestTimestamp: capturedAt - 37 * 86400,
  newestTimestamp: capturedAt - 45,
  errors: [],
};
