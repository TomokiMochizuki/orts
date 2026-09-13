import { describe, expect, it } from "vitest";

import { TORQUE_AXES, TORQUE_CHART_METRICS } from "../chartMetrics.js";
import { isTorqueChartActive, TORQUE_CHART_DEFS } from "./torqueCharts.js";

describe("isTorqueChartActive", () => {
  it("shows nothing when no model is active", () => {
    expect(isTorqueChartActive("panel_srp", [])).toBe(false);
    expect(isTorqueChartActive("panel_srp", undefined)).toBe(false);
  });

  it("matches a model by its own name", () => {
    expect(isTorqueChartActive("panel_srp", ["gravity", "panel_srp"])).toBe(true);
    expect(isTorqueChartActive("gravity_gradient", ["gravity_gradient"])).toBe(true);
    expect(isTorqueChartActive("panel_drag", ["gravity", "panel_srp"])).toBe(false);
  });

  // The acceleration charts match `panel_srp` against a chart named `srp`,
  // because a panel model reports its acceleration under the force's name. The
  // torque wire keeps the model's name, so no such renaming applies here — and
  // a torque chart must not appear for the cannonball model, which has no
  // torque to report.
  it("does not match the cannonball model of the same force", () => {
    expect(isTorqueChartActive("panel_srp", ["gravity", "srp"])).toBe(false);
    expect(isTorqueChartActive("panel_drag", ["gravity", "drag"])).toBe(false);
  });
});

describe("TORQUE_CHART_DEFS", () => {
  it("charts one model per def, with the three body axes as its series", () => {
    expect(TORQUE_CHART_DEFS.map((d) => d.model)).toEqual([
      "gravity_gradient",
      "panel_srp",
      "panel_drag",
    ]);
    for (const def of TORQUE_CHART_DEFS) {
      expect(def.series.map((s) => s.label)).toEqual(TORQUE_AXES);
    }
  });

  // A def naming a column no query selects draws an empty chart, so the names
  // have to be the ones `METRIC_NAMES` carries into the store.
  it("names only metrics the store queries", () => {
    for (const def of TORQUE_CHART_DEFS) {
      for (const series of def.series) {
        expect(TORQUE_CHART_METRICS, `missing "${series.metric}"`).toContain(series.metric);
      }
    }
  });

  it("gives each axis its own colour, shared across the models", () => {
    const colors = TORQUE_CHART_DEFS.map((d) => d.series.map((s) => s.color));
    expect(new Set(colors[0]).size).toBe(3);
    for (const perModel of colors) {
      expect(perModel).toEqual(colors[0]);
    }
  });
});
