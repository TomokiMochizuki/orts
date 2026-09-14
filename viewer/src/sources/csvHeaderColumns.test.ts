import { describe, expect, it } from "vitest";

import { parseCSVChunked } from "./csvParseLogic.js";
import { parseDataLineWithColumns, parseHeaderLine } from "./parseCSVLine.js";

/** The columns `orts run --format csv` writes, in the order it writes them.
 *
 * Measured from a two-satellite run with panels on one of them: the attitude
 * columns sit after the torque columns, which is why reading by position and
 * stopping at `nu` dropped both.
 */
const MODELS = [
  "gravity_gradient",
  "panel_drag",
  "panel_srp",
  "third_body_moon",
  "third_body_sun",
  "zonal_gravity",
];

const HEADER = [
  "# satellite_id",
  "t[s]",
  "x[km]",
  "y[km]",
  "z[km]",
  "vx[km/s]",
  "vy[km/s]",
  "vz[km/s]",
  "a[km]",
  "e[-]",
  "i[rad]",
  "raan[rad]",
  "omega[rad]",
  "nu[rad]",
  "wx",
  "wy",
  "wz",
  ...MODELS.flatMap((m) => ["x", "y", "z"].map((axis) => `${m}.torque_body_${axis}_Nm`)),
  "qw",
  "qx",
  "qy",
  "qz",
].join(",");

/** One row. `torques` names the models this satellite has; the rest are the
 * empty cells the writer leaves for a satellite without them. */
function row(id: string, t: number, torques: Record<string, [number, number, number]>) {
  const cells = [id, t.toFixed(3), "6778.137", "0", "0", "0", "7.6686", "0"];
  cells.push("6778.137", "0", "0", "0", "0", "0"); // a, e, i, raan, omega, nu
  cells.push("0.01", "0.02", "0.03"); // wx, wy, wz
  for (const model of MODELS) {
    const triple = torques[model];
    cells.push(...(triple ? triple.map(String) : ["", "", ""]));
  }
  cells.push("0.9239", "0", "0.3827", "0"); // qw..qz
  return cells.join(",");
}

describe("parseHeaderLine", () => {
  it("finds every column the viewer reads, by name", () => {
    const columns = parseHeaderLine(HEADER);

    expect(columns?.satelliteId).toBe(0);
    expect(columns?.fields.get("t")).toBe(1);
    expect(columns?.fields.get("inc")).toBe(10);
    // Past where the positional parsing stopped.
    expect(columns?.fields.get("wx")).toBe(14);
    expect(columns?.fields.get("torque_gravity_gradient_y")).toBe(18);
    expect(columns?.fields.get("qw")).toBe(35);
  });

  // Other models get torque columns too — `third_body_sun`, `zonal_gravity` —
  // and are read the day a chart knows them.
  it("takes only the torque columns a chart knows", () => {
    const columns = parseHeaderLine(HEADER);

    expect(columns?.fields.has("torque_panel_srp_x")).toBe(true);
    expect(columns?.fields.has("torque_zonal_gravity_x")).toBe(false);
  });

  it("is not a header without the time column", () => {
    expect(parseHeaderLine("# mu = 398600.4418 km^3/s^2")).toBeNull();
    expect(parseHeaderLine("sat-a,0.000,6778.137")).toBeNull();
  });
});

describe("parseDataLineWithColumns", () => {
  const columns = parseHeaderLine(HEADER);
  if (!columns) throw new Error("the fixture header must parse");

  it("reads the attitude and torque columns the position-based parsing dropped", () => {
    const point = parseDataLineWithColumns(
      row("sat-a", 10, { gravity_gradient: [1e-5, -2e-5, 3e-5], panel_srp: [4e-7, 5e-7, 6e-7] }),
      columns,
    );

    expect(point?.entityPath).toBe("sat-a");
    expect(point?.wx).toBe(0.01);
    expect(point?.qw).toBe(0.9239);
    expect(point?.torque_gravity_gradient_y).toBe(-2e-5);
    expect(point?.torque_panel_srp_z).toBe(6e-7);
  });

  // The writer leaves a satellite's cells empty for a model it does not have,
  // and `Number("")` is 0 — which would read as a torque measured to be zero.
  it("leaves an empty cell unset rather than zero", () => {
    const point = parseDataLineWithColumns(
      row("sat-b", 10, { gravity_gradient: [1e-6, 0, 0] }),
      columns,
    );

    expect(point?.torque_gravity_gradient_x).toBe(1e-6);
    expect(point?.torque_panel_srp_x).toBeUndefined();
    expect(point?.torque_panel_drag_x).toBeUndefined();
  });

  it("keeps a torque recorded as zero", () => {
    const point = parseDataLineWithColumns(row("sat-a", 10, { panel_drag: [0, 0, 0] }), columns);

    expect(point?.torque_panel_drag_x).toBe(0);
  });

  // The writer formats an f64 with `{:.10}`, so a non-finite sample is written
  // as Rust prints it. Measured from its output: `NaN`, `inf`, `-inf`. The
  // record layer keeps such a sample on purpose, so reading it as absent
  // would lose a measurement.
  it("reads the non-finite spellings the writer uses", () => {
    const cells = row("sat-a", 10, { gravity_gradient: [1, 2, 3] }).split(",");
    const columns2 = parseHeaderLine(HEADER);
    if (!columns2) throw new Error("header");
    cells[columns2.fields.get("torque_gravity_gradient_x") as number] = "NaN";
    cells[columns2.fields.get("torque_gravity_gradient_y") as number] = "inf";
    cells[columns2.fields.get("torque_gravity_gradient_z") as number] = "-inf";

    const point = parseDataLineWithColumns(cells.join(","), columns);

    expect(point?.torque_gravity_gradient_x).toBeNaN();
    expect(point?.torque_gravity_gradient_y).toBe(Number.POSITIVE_INFINITY);
    expect(point?.torque_gravity_gradient_z).toBe(Number.NEGATIVE_INFINITY);
  });

  // Filling it would state a circular orbit for a row that recorded nothing.
  it("leaves an orbital element the header names but the row omits unset", () => {
    const cells = row("sat-a", 10, {}).split(",");
    cells[columns.fields.get("e") as number] = "";

    const point = parseDataLineWithColumns(cells.join(","), columns);

    expect(point?.e).toBeUndefined();
  });

  // `"toString" in {...}` is true, and an object lookup would hand back a
  // function where a number is declared.
  it("does not read an inherited property name as a number", () => {
    const cells = row("sat-a", 10, {}).split(",");
    cells[columns.fields.get("t") as number] = "toString";

    expect(parseDataLineWithColumns(cells.join(","), columns)).toBeNull();
  });

  it("refuses a row without the state vector", () => {
    const cells = row("sat-a", 10, {}).split(",");
    cells[2] = ""; // x
    expect(parseDataLineWithColumns(cells.join(","), columns)).toBeNull();
  });
});

describe("parseCSVChunked with a header", () => {
  const csv = [
    "# mu = 398600.4418 km^3/s^2",
    "# epoch_jd = 2461102.5",
    "# satellites = sat-a, sat-b",
    HEADER,
    row("sat-a", 0, { gravity_gradient: [1e-5, 0, 0], panel_srp: [1e-7, 0, 0] }),
    row("sat-b", 0, { gravity_gradient: [2e-5, 0, 0] }),
    row("sat-a", 10, { gravity_gradient: [1e-5, 0, 0], panel_srp: [1e-7, 0, 0] }),
  ].join("\n");

  it("reports the models each satellite carries, not the file's columns", () => {
    let torqueModels: Record<string, string[]> | undefined;
    const points: unknown[] = [];
    parseCSVChunked(csv, 10, (msg) => {
      if (msg.type === "chunk") points.push(...msg.points);
      if (msg.type === "complete") torqueModels = msg.torqueModels;
    });

    expect(points).toHaveLength(3);
    expect(torqueModels?.["sat-a"]?.sort()).toEqual(["gravity_gradient", "panel_srp"]);
    expect(torqueModels?.["sat-b"]).toEqual(["gravity_gradient"]);
  });

  // A file written before the header existed is still read the old way.
  it("falls back to positional parsing without a header", () => {
    const headerless = [
      "# mu = 398600.4418 km^3/s^2",
      "0.000,6778.137,0,0,0,7.6686,0",
      "10.000,6778.0,76.6,0,-0.08,7.668,0",
    ].join("\n");

    const points: { t: number }[] = [];
    parseCSVChunked(headerless, 10, (msg) => {
      if (msg.type === "chunk") points.push(...(msg.points as { t: number }[]));
    });

    expect(points.map((p) => p.t)).toEqual([0, 10]);
  });
});
