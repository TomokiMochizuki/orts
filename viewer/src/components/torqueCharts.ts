/** Torque chart metadata and the rule for when each one is shown.
 *
 * One chart per model with its three body-frame components overlaid, because
 * what a torque model gets wrong is the direction: a magnitude reads the same
 * whether the spacecraft is being turned the right way or the wrong one.
 *
 * Kept apart from the component so the rule can be tested on its own, as the
 * acceleration charts' is.
 */

import { TORQUE_AXES } from "../chartMetrics.js";

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
  model: string;
  title: string;
  /** The three axes, in plotting order. */
  series: TorqueSeries[];
}

/** Colours for the three axes, held apart so every torque chart agrees. */
export const TORQUE_AXIS_COLORS = ["#f66", "#6f6", "#6af"];

export const TORQUE_CHART_DEFS: TorqueChartDef[] = [
  { model: "gravity_gradient", title: "Gravity-gradient Torque" },
  { model: "panel_srp", title: "SRP Torque" },
  { model: "panel_drag", title: "Aerodynamic Torque" },
].map(({ model, title }) => ({
  model,
  title,
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
