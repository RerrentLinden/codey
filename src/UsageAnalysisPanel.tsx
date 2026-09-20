import { useEffect, useRef, useState } from "react";
import { IconArrowLeft, IconChartDonut, IconRefresh } from "@tabler/icons-react";
import { Spinner } from "@heroui/react";
import { Button, Select, Badge } from "./components/ui";
import { invoke } from "./api";
import { withTimeout } from "./appUtils";
import { formatUsageNumber, observeUsageRequest, usageCoverage, usageQuery, usageSlices, usageTrend, type LogAnalytics, type UsageLoadState, type UsageRange } from "./usageAnalysis";
import styles from "./styles.usage-analysis.css?inline";
import { UsageHeatmap } from "./UsageHeatmap";

const colors = ["#007aff", "#635bdb", "#1a9c86", "#d98916", "#e06785", "#699ad4", "#8b71ba", "#71a54d", "#b77757", "#c883c7", "#428394", "#afb13d", "#93969e"];
const utc = (timestamp: number) => new Date(timestamp).toLocaleString("zh-CN", { timeZone: "UTC", year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });

export function UsageAnalysisPanel({ onBack }: { onBack: () => void }) {
  const [range, setRange] = useState<UsageRange>("7d");
  const [revision, setRevision] = useState(0);
  const [chart, setChart] = useState<"heatmap" | "bars">("heatmap");
  const [state, setState] = useState<UsageLoadState>({ loading: true, data: null, error: "" });
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => { heading.current?.focus(); }, []);
  useEffect(() => observeUsageRequest(
    () => withTimeout(invoke<LogAnalytics>("query_route_request_log_stats", usageQuery(range)), 30_000, "统计请求超时，请稍后重试。"),
    setState,
  ), [range, revision]);
  const reload = () => { setState({ loading: true, data: null, error: "" }); setRevision((value) => value + 1); };
  const stats = state.data;
  const health = stats?.recordingHealth;
  const dropped = health ? health.droppedFull + health.droppedClosed + health.writeDropped : 0;
  const partial = health && (health.sampleRatePerMillion < 1_000_000 || health.sampledOut > 0 || dropped > 0 || health.writeFailures > 0 || health.observerPanics > 0 || health.writerPanics > 0 || health.shutdownTimeouts > 0);
  const slices = stats?.queryable ? usageSlices(stats) : [];
  const totalTokens = stats?.totalTokensSum;
  let angle = 0;
  const gradient = totalTokens != null && totalTokens > 0 ? slices.map((slice, index) => {
    const start = angle;
    angle += (slice.tokens ?? 0) / totalTokens * 360;
    return `${colors[index]} ${start}deg ${angle}deg`;
  }).join(", ") : "";
  const trend = stats?.queryable ? usageTrend(stats) : { peak: null, bars: [] };
  // 未上报用量的分桶画成低矮虚线柱，避免与零用量一样显示为空白。
  const unknownBarHeight = 3;
  const percent = (tokens: number | null) => tokens != null && totalTokens != null && totalTokens > 0 ? tokens / totalTokens * 100 : null;

  return <section className="usage-analysis" aria-labelledby="usage-analysis-title">
    <style>{styles}</style>
    <Button variant="link" className="usage-back" onClick={onBack}><IconArrowLeft size={15} aria-hidden="true" />返回线路与模型</Button>
    <div className="usage-heading">
      <div className="section-heading"><span className="section-icon" aria-hidden="true"><IconChartDonut size={18} /></span><div>
        <h1 ref={heading} tabIndex={-1} id="usage-analysis-title">用量分析</h1>
        <p>了解模型消耗分布与用量变化</p>
      </div></div>
      <Button variant="outline" size="sm" loading={state.loading} onClick={reload}><IconRefresh size={14} aria-hidden="true" />刷新</Button>
    </div>
    <div className="usage-toolbar">
      <div className="usage-controls">
        <Select aria-label="用量时间范围" value={range} optionList={[{ label: "近 7 天", value: "7d" }, { label: "近 30 天", value: "30d" }, { label: "全部历史", value: "all" }]} onChange={(value) => { if (value !== range) { setState({ loading: true, data: null, error: "" }); setRange(value as UsageRange); } }} />
      </div>
      <Badge variant="secondary">仅保留的日志</Badge>
    </div>
    <p className="usage-scope">仅统计经过 Codey 路由且仍保留的请求日志，不包含官方直连。清理日志会影响历史统计。</p>
    <div aria-live="polite" aria-busy={state.loading}>
      {state.loading ? <div className="usage-state" role="status"><Spinner size="sm" /><strong>正在统计用量…</strong><span>正在读取所选范围的请求日志</span></div>
        : state.error ? <div className="usage-state usage-error" role="alert"><strong>用量分析加载失败</strong><p>{state.error}</p><Button variant="outline" size="sm" onClick={reload}>重试</Button></div>
        : stats?.queryable === false ? <div className="usage-state"><strong>当前日志暂不可统计</strong><p>{stats.reason === "ndjson_not_queryable" ? "当前日志格式不支持在线统计。返回线路与模型，重新开启日志记录后重试。" : "请检查日志记录状态后重试。"}</p><Button variant="outline" size="sm" onClick={reload}>重试</Button></div>
        : stats ? <>
          {health && !health.active && <p className="usage-notice">{health.enabled ? "日志记录已停止，请检查记录状态；" : "日志记录未开启；"}仍可查看已保留的历史用量。</p>}
          {partial && <p className="usage-notice">当前记录周期存在采样、丢弃或写入异常，统计仅反映已保存记录，不能代表全部请求。</p>}
          {health && health.pendingEntries > 0 && <p className="usage-scope">有 {formatUsageNumber(health.pendingEntries)} 条日志等待写入，可稍后刷新。</p>}
          {stats.total === 0 ? <div className="usage-state"><IconChartDonut size={32} aria-hidden="true" /><strong>所选范围暂无请求日志</strong><span>可切换到全部历史，或开启日志记录后发起请求。</span></div> : <>
            <div className="usage-overview">
              <div className="usage-total"><span>总 Token</span><strong>{formatUsageNumber(totalTokens)}</strong><small>所选范围 · 已知总量合计</small>
                <div className="usage-quality"><span>用量已知覆盖率 <b>{usageCoverage(stats.total, stats.totalTokensKnownCount)}</b></span><span>{formatUsageNumber(stats.totalTokensKnownCount)} / {formatUsageNumber(stats.total)} 条总量已知</span></div>
              </div>
              <div className="usage-metrics">
              {[
                ["输入 Token", formatUsageNumber(stats.inputTokensSum), "包含缓存输入"],
                ["输出 Token", formatUsageNumber(stats.outputTokensSum), "上游已上报用量"],
                ["缓存 Token", formatUsageNumber(stats.cachedTokensSum), "已包含于输入，不重复相加"],
                ["请求数", formatUsageNumber(stats.total), "所选范围内保留的记录"],
              ].map(([title, value, detail]) => <div className="usage-metric" key={title}><span>{title}</span><strong>{value}</strong><small>{detail}</small></div>)}
              </div>
            </div>
            {stats.totalTokensKnownCount < stats.total && <p className="usage-notice">部分请求未上报总 Token，用量与占比仅基于已知数据；“—”表示未上报。</p>}
            <div className="usage-card">
              <div className="usage-card-heading"><div><h2>模型用量占比</h2><p>按已知 Token 排序，快速定位主要消耗</p></div><Badge variant="secondary">前 12 项</Badge></div>
              <div className="usage-distribution">
                <div className="usage-donut-wrap"><div className="usage-donut" role="img" aria-label={totalTokens == null ? "尚无已知 Token 用量，占比不可计算" : `已知总 Token ${formatUsageNumber(totalTokens)}，详细占比见图例`} style={{ background: gradient ? `conic-gradient(${gradient})` : "var(--codey-surface-sunken, #e9eaee)" }}><div><span>已知 Token</span><strong>{formatUsageNumber(totalTokens)}</strong></div></div><p>模型消耗分布</p></div>
                <ul className="usage-legend">
                  {slices.map((slice, index) => <li key={`${slice.other ? "other" : "group"}:${slice.key}`}><span className="usage-rank" aria-hidden="true">{slice.other ? "·" : String(index + 1).padStart(2, "0")}</span><div className="usage-model"><strong><i style={{ background: colors[index] }} aria-hidden="true" />{slice.other ? "其他" : slice.key || "未知模型"}</strong><small>{formatUsageNumber(slice.requests)} 次请求</small><span className="usage-share-track" aria-hidden="true"><span style={{ width: `${percent(slice.tokens) ?? 0}%`, background: colors[index] }} /></span></div><div className="usage-legend-value"><strong>{formatUsageNumber(slice.tokens)}</strong><small>{percent(slice.tokens)?.toFixed(1) ?? "—"}{percent(slice.tokens) == null ? "" : "%"}</small></div></li>)}
                </ul>
              </div>
              {(stats.groups.length > 12 || stats.groupsTruncated) && <p className="usage-scope">“其他”包含前 12 项以外的所有分组{stats.groupsTruncated ? "，包括未单独返回的分组" : ""}。</p>}
            </div>
            <div className="usage-card">
              <div className="usage-card-heading"><div><h2>Token 时间趋势</h2><p>观察所选范围内的消耗节奏</p></div><div className="usage-chart-switch" role="group" aria-label="趋势图类型"><Button size="xs" variant={chart === "heatmap" ? "light" : "ghost"} aria-pressed={chart === "heatmap"} onPress={() => setChart("heatmap")}>热力图</Button><Button size="xs" variant={chart === "bars" ? "light" : "ghost"} aria-pressed={chart === "bars"} onPress={() => setChart("bars")}>直方图</Button></div></div>
              <p className="usage-scope">{utc(stats.fromUnixMs)} — {utc(stats.toUnixMs)} UTC（不含结束时间）</p>
              {chart === "heatmap" && <UsageHeatmap key={range} stats={stats} />}
              {stats.trend.length ? <>
                {chart === "bars" && <><div className="usage-trend-unit">Token · UTC · 每 {formatUsageNumber(stats.bucketMs / 3_600_000)} 小时一组</div>
                <div className="usage-trend-chart"><div className="usage-trend-scale"><span>{formatUsageNumber(trend.peak)}</span><span>{formatUsageNumber(trend.peak == null ? null : trend.peak / 2)}</span><span>0</span></div><div className="usage-trend-plot">
                  <svg className="usage-trend" viewBox="0 0 800 140" preserveAspectRatio="none" role="img" aria-label="UTC 时间分桶的已知 Token 趋势，展开下方明细可读取每组数据">
                    {[11, 75, 139].map((y) => <line className="usage-gridline" key={y} x1="0" y1={y} x2="800" y2={y} strokeDasharray={y === 139 ? undefined : "4 5"} />)}
                    {trend.bars.map((bucket) => <rect className="usage-trend-bar" key={bucket.timestampUnixMs} x={bucket.x} y={139 - (bucket.tokensUnknown ? unknownBarHeight : bucket.height)} width={bucket.width} height={bucket.tokensUnknown ? unknownBarHeight : bucket.height} rx="2" style={bucket.tokensUnknown ? { fill: "none", stroke: "var(--codey-blue, #007aff)", strokeDasharray: "2 2", strokeWidth: 1.5, opacity: .75, vectorEffect: "non-scaling-stroke" } : undefined}><title>{utc(bucket.timestampUnixMs)} UTC：{bucket.tokensUnknown ? "用量未上报" : `${formatUsageNumber(bucket.totalTokensSum)} Token`}，{formatUsageNumber(bucket.total)} 次请求</title></rect>)}
                  </svg>
                  <div className="usage-trend-labels"><span>{utc(stats.fromUnixMs)}</span><span className="usage-trend-midpoint">{utc(stats.fromUnixMs + (stats.toUnixMs - stats.fromUnixMs) / 2)}</span><span>{utc(stats.toUnixMs)}</span></div>
                </div></div>
                <p className="usage-scope">虚线柱表示该时段有请求但用量未上报；空白表示没有请求，以明细为准。</p></>}
                <details className="usage-trend-details"><summary>查看趋势明细</summary><div className="usage-table-scroll"><table><thead><tr><th>分桶起点（UTC）</th><th>请求数</th><th>Token</th></tr></thead><tbody>{stats.trend.map((bucket) => <tr key={bucket.timestampUnixMs}><td>{utc(bucket.timestampUnixMs)}</td><td>{formatUsageNumber(bucket.total)}</td><td>{formatUsageNumber(bucket.totalTokensSum)}</td></tr>)}</tbody></table></div></details>
              </> : <p className="usage-scope">暂无趋势数据</p>}
            </div>
          </>}
        </> : null}
    </div>
  </section>;
}
