import { describe, expect, it } from "vitest";
import { TORQUE_CHART_METRICS } from "../chartMetrics.js";
import { createOrbitSchema } from "./orbitSchema.js";

describe("createOrbitSchema", () => {
  const schema = createOrbitSchema();

  it("has 25 base columns (13 orbital + 5 acceleration + 7 attitude) plus a torque column per model and axis", () => {
    expect(schema.columns).toHaveLength(25 + TORQUE_CHART_METRICS.length);
    const names = schema.columns.map((c) => c.name);
    expect(names).toEqual([
      "t",
      "x",
      "y",
      "z",
      "vx",
      "vy",
      "vz",
      "a",
      "e",
      "inc",
      "raan",
      "omega",
      "nu",
      "accel_gravity",
      "accel_drag",
      "accel_srp",
      "accel_third_body_sun",
      "accel_third_body_moon",
      "qw",
      "qx",
      "qy",
      "qz",
      "wx",
      "wy",
      "wz",
      ...TORQUE_CHART_METRICS,
    ]);
  });

  // `buildDerivedQuery` SELECTs only the derived columns, so a column with no
  // pass-through here never reaches a chart however the inserts are written.
  it("exposes every torque column to the query", () => {
    const derived = schema.derived.map((d) => d.name);
    for (const metric of TORQUE_CHART_METRICS) {
      expect(derived, `"${metric}" is not queryable`).toContain(metric);
    }
  });

  it("toRow returns one value per column", () => {
    const point = {
      t: 0,
      x: 6778,
      y: 0,
      z: 0,
      vx: 0,
      vy: 7.669,
      vz: 0,
      a: 6778,
      e: 0.001,
      inc: 0.9,
      raan: 3.14,
      omega: 1.0,
      nu: 0.5,
    };
    const row = schema.toRow(point);
    expect(row).toHaveLength(schema.columns.length);
    expect(row.every((v) => typeof v === "number" || v === null)).toBe(true);
  });

  it("toRow does not produce undefined for any field", () => {
    // Simulate what happens when WebSocket data is ingested
    const point = {
      t: 10,
      x: 6778,
      y: 100,
      z: 0,
      vx: -0.1,
      vy: 7.669,
      vz: 0,
      a: 6780,
      e: 0.001,
      inc: 0.9,
      raan: 3.14,
      omega: 1.0,
      nu: 0.5,
    };
    const row = schema.toRow(point);
    for (let i = 0; i < row.length; i++) {
      expect(
        row[i],
        `column ${schema.columns[i].name} should not be undefined`,
      ).not.toBeUndefined();
    }
  });

  it("derived columns include semi-major axis (a) for charting", () => {
    // buildDerivedQuery only SELECTs derived columns, not base columns.
    // To chart a base column, it must also appear in the derived list.
    const derivedNames = schema.derived.map((d) => d.name);
    expect(derivedNames).toContain("a");
  });

  it("derived columns include eccentricity (e) for charting", () => {
    const derivedNames = schema.derived.map((d) => d.name);
    expect(derivedNames).toContain("e");
  });

  it("derived columns include all expected chart columns", () => {
    const derivedNames = schema.derived.map((d) => d.name);
    const expectedChartColumns = [
      "altitude",
      "energy",
      "angular_momentum",
      "velocity",
      "inc_deg",
      "raan_deg",
      "omega_deg",
      "nu_deg",
      "a",
      "e",
      "accel_gravity",
      "accel_drag",
      "accel_srp",
      "accel_third_body_sun",
      "accel_third_body_moon",
      "accel_perturbation_total",
    ];
    for (const col of expectedChartColumns) {
      expect(derivedNames, `missing derived column: ${col}`).toContain(col);
    }
  });
});
