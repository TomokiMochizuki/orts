import { describe, expect, it } from "vitest";

import { TORQUE_AXES, TORQUE_CHART_METRICS, TORQUE_CHART_MODELS } from "../chartMetrics.js";
import { buildTorqueChartData, isTorqueChartActive, TORQUE_CHART_DEFS } from "./torqueCharts.js";

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
  // The metric names and the parser are generated from `TORQUE_CHART_MODELS`,
  // so a chart for a model outside that list would name columns nobody writes.
  it("charts exactly the models the store knows", () => {
    expect(TORQUE_CHART_DEFS.map((d) => d.model)).toEqual([...TORQUE_CHART_MODELS]);
    for (const def of TORQUE_CHART_DEFS) {
      expect(def.title, `"${def.model}" needs a title`).toBeTruthy();
    }
  });

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

describe("buildTorqueChartData", () => {
  const def = TORQUE_CHART_DEFS[1]; // panel_srp
  const [xMetric, yMetric, zMetric] = def.series.map((s) => s.metric);
  const t = new Float64Array([0, 1, 2]);

  function axis(...values: number[]) {
    return new Float64Array(values);
  }

  it("plots the three axes of one satellite as three series", () => {
    const data = buildTorqueChartData(
      def,
      {
        t,
        [xMetric]: axis(1, 2, 3),
        [yMetric]: axis(4, 5, 6),
        [zMetric]: axis(7, 8, 9),
      },
      null,
    );

    expect(data?.series.map((s) => s.label)).toEqual(["x", "y", "z"]);
    expect(data?.values[2]?.[0]).toBe(7);
    expect(data?.t).toBe(t);
  });

  // Two of three axes read as a direction they are not, so the chart is not
  // drawn at all.
  it("draws nothing when an axis is missing", () => {
    const data = buildTorqueChartData(
      def,
      { t, [xMetric]: axis(1, 2, 3), [yMetric]: axis(4, 5, 6) },
      null,
    );

    expect(data).toBeNull();
  });

  it("draws nothing when there is no data at all", () => {
    expect(buildTorqueChartData(def, null, null)).toBeNull();
    expect(buildTorqueChartData(def, undefined, {})).toBeNull();
  });

  describe("in a fleet", () => {
    function multi(axes: Record<string, { t: Float64Array; sats: string[]; color?: string }>) {
      const map: Record<string, ReturnType<typeof series> | null> = {};
      for (const [metric, { t: axisT, sats, color }] of Object.entries(axes)) {
        map[metric] = series(axisT, sats, color);
      }
      return map;
    }

    function series(axisT: Float64Array, sats: string[], color = "#ffffff") {
      return {
        t: axisT,
        values: sats.map((_, i) => new Float64Array(axisT.length).fill(i + 1)),
        series: sats.map((label) => ({ label, color })),
      };
    }

    // One satellite's three components belong together: reading them every
    // third entry down the legend is what the axis-major order forced.
    it("groups a satellite's axes together, labelled with its name", () => {
      const data = buildTorqueChartData(
        def,
        null,
        multi({
          [xMetric]: { t, sats: ["sat-a", "sat-b"] },
          [yMetric]: { t, sats: ["sat-a", "sat-b"] },
          [zMetric]: { t, sats: ["sat-a", "sat-b"] },
        }),
      );

      expect(data?.series.map((s) => s.label)).toEqual([
        "sat-a x",
        "sat-a y",
        "sat-a z",
        "sat-b x",
        "sat-b y",
        "sat-b z",
      ]);
    });

    // The hue is the satellite's, so the axes are told apart by lightness —
    // the difference that survives every kind of colour vision. Three lines in
    // one satellite's single colour would be indistinguishable.
    it("separates a satellite's axes by lightness within its own colour", () => {
      const data = buildTorqueChartData(
        def,
        null,
        multi({
          [xMetric]: { t, sats: ["sat-a"], color: "#00ff88" },
          [yMetric]: { t, sats: ["sat-a"], color: "#00ff88" },
          [zMetric]: { t, sats: ["sat-a"], color: "#00ff88" },
        }),
      );

      const colors = data?.series.map((s) => s.color) ?? [];
      expect(new Set(colors).size).toBe(3);
      const luminance = colors.map((c) => {
        const hex = c.replace("#", "");
        const [r, g, b] = [0, 2, 4].map((i) => Number.parseInt(hex.slice(i, i + 2), 16));
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
      });
      // x is the satellite's own colour, y lighter, z darker.
      expect(luminance[1]).toBeGreaterThan(luminance[0]);
      expect(luminance[2]).toBeLessThan(luminance[0]);
    });

    // Equal sample counts do not make the times equal, and the type carries
    // no promise that they are, so the values decide.
    it("leaves out an axis sampled at different times", () => {
      const data = buildTorqueChartData(
        def,
        null,
        multi({
          [xMetric]: { t, sats: ["sat-a"] },
          [yMetric]: { t: new Float64Array([0, 1, 9]), sats: ["sat-a"] },
          [zMetric]: { t, sats: ["sat-a"] },
        }),
      );

      expect(data?.series.map((s) => s.label)).toEqual(["sat-a x", "sat-a z"]);
    });

    // A chart has one time axis. An axis aligned on a different one would be
    // drawn against the wrong times, so it is left out instead.
    it("leaves out an axis with a different number of samples", () => {
      const data = buildTorqueChartData(
        def,
        null,
        multi({
          [xMetric]: { t, sats: ["sat-a"] },
          [yMetric]: { t: new Float64Array([0, 1]), sats: ["sat-a"] },
          [zMetric]: { t, sats: ["sat-a"] },
        }),
      );

      expect(data?.series.map((s) => s.label)).toEqual(["sat-a x", "sat-a z"]);
      expect(data?.t.length).toBe(3);
    });

    it("draws nothing when the fleet carries none of the axes", () => {
      expect(buildTorqueChartData(def, { t }, {})).toBeNull();
    });
  });
});
