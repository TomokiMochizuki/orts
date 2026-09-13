import { type ChartDataMap, type TimeRange, TimeSeriesChart } from "@sksat/uneri";
import { memo, useMemo, useState } from "react";
import type { MultiChartDataMap, MultiSeriesData } from "../hooks/buildMultiChartData.js";
import { ACCEL_CHART_DEFS, isAccelChartActive } from "./accelCharts.js";
import styles from "./GraphPanel.module.css";
import { buildTorqueChartData, isTorqueChartActive, TORQUE_CHART_DEFS } from "./torqueCharts.js";

const TIME_RANGE_OPTIONS: { label: string; value: TimeRange }[] = [
  { label: "All", value: null },
  { label: "5 min", value: 300 },
  { label: "30 min", value: 1800 },
  { label: "1 h", value: 3600 },
];

/** Chart definitions: metric name → display config. */
const CHART_DEFS: { metric: string; title: string; yLabel: string; color: string }[] = [
  { metric: "altitude", title: "Altitude", yLabel: "km", color: "#4af" },
  { metric: "energy", title: "Specific Orbital Energy", yLabel: "km\u00B2/s\u00B2", color: "#f84" },
  { metric: "angular_momentum", title: "Angular Momentum", yLabel: "km\u00B2/s", color: "#8f4" },
  { metric: "velocity", title: "Velocity", yLabel: "km/s", color: "#f4f" },
  { metric: "a", title: "Semi-major Axis", yLabel: "km", color: "#4ff" },
  { metric: "e", title: "Eccentricity", yLabel: "-", color: "#ff4" },
  { metric: "inc_deg", title: "Inclination", yLabel: "deg", color: "#f48" },
  { metric: "raan_deg", title: "RAAN", yLabel: "deg", color: "#84f" },
];

/** Acceleration chart definitions. Shown conditionally based on active perturbations. */

interface GraphPanelProps {
  /** Single-satellite chart data (replay mode / single sat). */
  chartData?: ChartDataMap | null;
  /** Multi-satellite chart data (comparison mode). */
  multiChartData?: MultiChartDataMap | null;
  isLoading: boolean;
  timeRange: TimeRange;
  onTimeRangeChange: (range: TimeRange) => void;
  /** Called when the user drag-zooms into a time range on any chart. */
  onZoom?: (tMin: number, tMax: number) => void;
  /** Active perturbation names from SimInfo (union across all satellites). */
  activePerturbations?: string[];
}

export const GraphPanel = memo(function GraphPanel({
  chartData,
  multiChartData,
  isLoading,
  timeRange,
  onTimeRangeChange,
  onZoom,
  activePerturbations,
}: GraphPanelProps) {
  const [collapsed, setCollapsed] = useState(false);

  // Acceleration charts appear once any perturbation is active: gravity and the
  // total then show alongside whichever individual perturbations are on.
  const visibleAccelDefs = useMemo(
    () => ACCEL_CHART_DEFS.filter((def) => isAccelChartActive(def.pertKey, activePerturbations)),
    [activePerturbations],
  );

  const allDefs = useMemo(
    () => [...CHART_DEFS, ...visibleAccelDefs.map((d) => ({ ...d, yLabel: "km/s\u00B2" }))],
    [visibleAccelDefs],
  );

  // A torque chart appears when the run carries that model, and carries its
  // three body-frame components as series: what a torque model gets wrong is
  // the direction, which a magnitude cannot show.
  const visibleTorqueDefs = useMemo(
    () => TORQUE_CHART_DEFS.filter((def) => isTorqueChartActive(def.model, activePerturbations)),
    [activePerturbations],
  );

  // One `MultiSeriesData` per model. In a fleet the series dimension is
  // already the satellites, so each satellite's axes are labelled with its own
  // name and the legend's isolation (click a series) is how one is read alone.
  const torqueData = useMemo(() => {
    const result: Record<string, MultiSeriesData | null> = {};
    for (const def of visibleTorqueDefs) {
      result[def.model] = buildTorqueChartData(def, chartData, multiChartData);
    }
    return result;
  }, [visibleTorqueDefs, chartData, multiChartData]);

  // Single-series data extraction (for backward compat / single satellite)
  const singleSeriesData = useMemo(() => {
    if (!chartData) return null;
    const result: Record<string, [Float64Array, Float64Array] | null> = {};
    for (const def of allDefs) {
      result[def.metric] = chartData[def.metric]
        ? ([chartData.t, chartData[def.metric]] as [Float64Array, Float64Array])
        : null;
    }
    return result;
  }, [chartData, allDefs]);

  return (
    <div className={`${styles.graphPanel} ${collapsed ? styles.collapsed : ""}`}>
      <button className={styles.toggle} onClick={() => setCollapsed((c) => !c)}>
        {collapsed ? "\u25C0 Graphs" : "\u25B6"}
      </button>
      {!collapsed && (
        <div className={styles.content}>
          {isLoading && <div className={styles.loading}>Loading DuckDB...</div>}
          <div className={styles.timeRangeSelector}>
            {TIME_RANGE_OPTIONS.map((opt) => (
              <button
                key={opt.label}
                className={`${styles.timeRangeBtn} ${timeRange === opt.value ? styles.active : ""}`}
                onClick={() => onTimeRangeChange(opt.value)}
              >
                {opt.label}
              </button>
            ))}
          </div>
          {allDefs.map((def) => (
            <TimeSeriesChart
              key={def.metric}
              title={def.title}
              yLabel={def.yLabel}
              data={multiChartData ? null : (singleSeriesData?.[def.metric] ?? null)}
              multiData={multiChartData?.[def.metric]}
              color={def.color}
              onZoom={onZoom}
            />
          ))}
          {visibleTorqueDefs.map((def) => (
            <TimeSeriesChart
              key={def.model}
              title={def.title}
              yLabel={def.yLabel}
              multiData={torqueData[def.model]}
              onZoom={onZoom}
              // A gap here is a sample with no such model, not a sample that
              // landed between another satellite's: drawing across it would
              // state a torque that was never computed.
              spanGaps={false}
            />
          ))}
        </div>
      )}
    </div>
  );
});
