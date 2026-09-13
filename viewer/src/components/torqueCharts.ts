/** Torque chart metadata and the rule for when each one is shown.
 *
 * One chart per model with its three body-frame components overlaid, because
 * what a torque model gets wrong is the direction: a magnitude reads the same
 * whether the spacecraft is being turned the right way or the wrong one.
 *
 * Kept apart from the component so the rule can be tested on its own, as the
 * acceleration charts' is.
 */

import type { ChartDataMap } from "@sksat/uneri";
import { TORQUE_AXES, TORQUE_CHART_MODELS, type TorqueChartModel } from "../chartMetrics.js";
import type { MultiChartDataMap, MultiSeriesData } from "../hooks/buildMultiChartData.js";

/** One axis of one model's torque, as a chart series. */
export interface TorqueSeries {
  /** Chart metric name, matching the DuckDB column. */
  metric: string;
  /** Series label: the axis, or the satellite and the axis in a fleet. */
  label: string;
  color: string;
}

/** One chart: a model, and its three components as series. */
export interface TorqueChartDef {
  /** `Model::name`, as the wire reports it. */
  model: TorqueChartModel;
  title: string;
  /** Y-axis unit label. */
  yLabel: string;
  /** The three axes, in plotting order. */
  series: TorqueSeries[];
}

/** Colours for the three axes, held apart so every torque chart agrees. */
export const TORQUE_AXIS_COLORS = ["#f66", "#6f6", "#6af"];

/** The chart title for each model. Typed over `TORQUE_CHART_MODELS`, so a
 * model added there without a title here is a compile error rather than a
 * chart that never appears — the metric names and the parser are generated
 * from that same list. */
const TORQUE_CHART_TITLES: Record<TorqueChartModel, string> = {
  gravity_gradient: "Gravity-gradient Torque",
  panel_srp: "SRP Torque",
  panel_drag: "Aerodynamic Torque",
};

export const TORQUE_CHART_DEFS: TorqueChartDef[] = TORQUE_CHART_MODELS.map((model) => ({
  model,
  title: TORQUE_CHART_TITLES[model],
  yLabel: "N\u00B7m",
  series: TORQUE_AXES.map((axis, i) => ({
    metric: `torque_${model}_${axis}`,
    label: axis,
    color: TORQUE_AXIS_COLORS[i],
  })),
}));

/** Whether a model's torque chart should be shown for the active models.
 *
 * The wire reports a torque per model, so the chart appears when the run
 * carries that model. A panel model reports under its own name here, where the
 * acceleration channels rename it to the force it computes — `panel_srp` is
 * both the model and the chart.
 */
export function isTorqueChartActive(
  model: string,
  activePerturbations: string[] | undefined,
): boolean {
  if (!activePerturbations || activePerturbations.length === 0) return false;
  return activePerturbations.includes(model);
}

/** Whether two aligned axes carry the same times. */
function sameTimes(a: Float64Array, b: Float64Array): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return false;
  }
  return true;
}

/** Assemble one model's chart: its three axes as series on one time axis.
 *
 * In a fleet the series dimension is already the satellites, so each
 * satellite's axes carry its own name and one satellite is read by isolating
 * its series in the legend. `multiChartData` takes precedence for the same
 * reason the other charts prefer it: it is what the multi-satellite view has.
 */
export function buildTorqueChartData(
  def: TorqueChartDef,
  chartData: ChartDataMap | null | undefined,
  multiChartData: MultiChartDataMap | null | undefined,
): MultiSeriesData | null {
  if (multiChartData) {
    const perAxis = def.series.map((axis) => multiChartData[axis.metric]);
    const t = perAxis.find((d) => d)?.t;
    if (!t) return null;

    const values: Float64Array[] = [];
    const series: { label: string; color: string }[] = [];
    perAxis.forEach((axisData, i) => {
      // One chart, one time axis: an axis aligned on different times would be
      // drawn against these ones. Equal sample counts do not make the times
      // equal, so the values are compared.
      if (!axisData || !sameTimes(axisData.t, t)) return;
      axisData.values.forEach((satValues, sat) => {
        values.push(satValues);
        series.push({
          label: `${axisData.series[sat]?.label ?? "sat"} ${def.series[i].label}`,
          color: axisData.series[sat]?.color ?? def.series[i].color,
        });
      });
    });
    return values.length > 0 ? { t, values, series } : null;
  }

  if (!chartData) return null;
  // All three axes or none: two of them would read as a direction they are not.
  const values = def.series.map((axis) => chartData[axis.metric]).filter((v) => v);
  if (values.length !== def.series.length) return null;
  return {
    t: chartData.t,
    values,
    series: def.series.map((axis) => ({ label: axis.label, color: axis.color })),
  };
}
