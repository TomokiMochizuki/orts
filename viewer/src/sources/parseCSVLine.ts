/**
 * Pure functions for parsing CSV orbit data lines.
 *
 * Shared between the main thread (orbit.ts) and the CSV parse Worker.
 * No DOM or React dependencies.
 */

import { TORQUE_CHART_MODELS } from "../chartMetrics.js";
import type { CSVMetadata, OrbitPoint } from "../orbit.js";

/**
 * Try to parse a CSV comment line as metadata.
 * Returns the key-value pair if the line matches `# key = value`, else null.
 */
export function parseMetadataLine(line: string, metadata: CSVMetadata): boolean {
  const match = line.match(/^#\s*(\w+)\s*=\s*(.+)/);
  if (!match) return false;

  const [, key, value] = match;
  switch (key) {
    case "epoch_jd":
      metadata.epochJd = Number(value.trim());
      break;
    case "mu":
      metadata.mu = Number(value.trim().split(/\s/)[0]);
      break;
    case "central_body":
      metadata.centralBody = value.trim();
      break;
    case "central_body_radius":
      metadata.centralBodyRadius = Number(value.trim().split(/\s/)[0]);
      break;
    case "satellite": {
      const trimmed = value.trim();
      if (trimmed) metadata.satelliteName = trimmed;
      break;
    }
    case "satellites": {
      metadata.satellites = value
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      break;
    }
    default:
      return false;
  }
  return true;
}

/** The name each column of `orts run --format csv` is written under.
 *
 * The units are part of the names, so they are matched whole rather than
 * normalised away: a name this table does not list is a column this viewer
 * does not read, which is a different thing from a name it mis-parsed.
 */
const BASE_COLUMNS: Record<string, keyof OrbitPoint> = {
  "t[s]": "t",
  "x[km]": "x",
  "y[km]": "y",
  "z[km]": "z",
  "vx[km/s]": "vx",
  "vy[km/s]": "vy",
  "vz[km/s]": "vz",
  "a[km]": "a",
  "e[-]": "e",
  "i[rad]": "inc",
  "raan[rad]": "raan",
  "omega[rad]": "omega",
  "nu[rad]": "nu",
  wx: "wx",
  wy: "wy",
  wz: "wz",
  qw: "qw",
  qx: "qx",
  qy: "qy",
  qz: "qz",
};

/** Columns a row is not a state vector without. */
const REQUIRED_COLUMNS = ["t", "x", "y", "z", "vx", "vy", "vz"] as const;

/** Where each column the viewer reads sits in a data line. */
export interface CSVColumns {
  /** Index of the satellite id, when the file carries one. */
  satelliteId?: number;
  /** `OrbitPoint` field to column index. */
  fields: Map<string, number>;
}

/**
 * Read a comment line as the column header, or return null if it is not one.
 *
 * The header is written as a comment, so it arrives among the metadata lines.
 * It is recognised by carrying the time column, which every file has.
 */
export function parseHeaderLine(line: string): CSVColumns | null {
  if (!line.startsWith("#")) return null;
  const cells = line
    .replace(/^#\s*/, "")
    .split(",")
    .map((cell) => cell.trim());
  if (!cells.includes("t[s]")) return null;

  const columns: CSVColumns = { fields: new Map() };
  cells.forEach((cell, index) => {
    if (cell === "satellite_id") {
      columns.satelliteId = index;
      return;
    }
    const field = BASE_COLUMNS[cell];
    if (field) {
      columns.fields.set(field, index);
      return;
    }
    // `<model>.torque_body_<axis>_Nm`, for the models the charts know. Other
    // models are recorded too and are read the day the charts know them.
    const torque = cell.match(/^(.+)\.torque_body_([xyz])_Nm$/);
    if (torque && (TORQUE_CHART_MODELS as readonly string[]).includes(torque[1])) {
      columns.fields.set(`torque_${torque[1]}_${torque[2]}`, index);
    }
  });
  return columns;
}

/** How the writer spells a non-finite `f64`, measured from its own output.
 *
 * Values go through `{:.10}`, so these are Rust's `Display` forms. A recorded
 * non-finite sample is a measurement the record layer keeps on purpose, and
 * `Number` reads none of these spellings: `Number("inf")` is NaN, which would
 * otherwise be indistinguishable from a malformed cell.
 */
const NON_FINITE = new Map<string, number>([
  ["NaN", Number.NaN],
  ["inf", Number.POSITIVE_INFINITY],
  ["-inf", Number.NEGATIVE_INFINITY],
]);

/** One cell as a number, or `undefined` where the file left it empty.
 *
 * A satellite that has none of a model has empty cells in that model's
 * columns, and `Number("")` is 0 — which would read as a torque measured to
 * be zero rather than a model that is not there.
 */
function cell(cells: string[], index: number | undefined): number | undefined {
  if (index === undefined) return undefined;
  const raw = cells[index];
  if (raw === undefined || raw === "") return undefined;
  // A `Map`, not an object: `"toString" in {...}` is true and yields a
  // function, which would leave a cell spelled `toString` passing as a number.
  const nonFinite = NON_FINITE.get(raw);
  if (nonFinite !== undefined) return nonFinite;
  const value = Number(raw);
  return Number.isNaN(value) ? undefined : value;
}

/**
 * Parse a data line against a header, or return null if it is not a state.
 *
 * Every column the viewer reads is found by name, so a file that grows a
 * column in the middle — as the recorder's did — still parses.
 */
export function parseDataLineWithColumns(line: string, columns: CSVColumns): OrbitPoint | null {
  const cells = line.split(",").map((s) => s.trim());

  const read: Record<string, number | string | undefined> = {};
  for (const [field, index] of columns.fields) {
    const value = cell(cells, index);
    if (value !== undefined) read[field] = value;
  }
  for (const required of REQUIRED_COLUMNS) {
    if (read[required] === undefined) return null;
  }
  if (columns.satelliteId !== undefined) {
    const id = cells[columns.satelliteId];
    if (id) read.entityPath = id;
  }

  // `OrbitPoint` declares the six orbital elements as numbers, and its readers
  // — the DuckDB row, the chart row — use them as such. So an element the
  // header names but the row leaves empty is NaN, which the charts draw as a
  // gap, rather than 0, which would state a circular orbit for a row that
  // recorded nothing. An element the header does not name at all keeps the
  // zero the positional parsing gave a short line.
  const element = (name: string): number => {
    const value = read[name];
    if (typeof value === "number") return value;
    return columns.fields.has(name) ? Number.NaN : 0;
  };
  const required = (name: string): number => read[name] as number;

  // Named rather than cast: `OrbitPoint` declares the elements as numbers, so
  // the fields it requires are spelled out here and the optional ones —
  // attitude, torque — are spread in as they were read.
  return {
    ...read,
    t: required("t"),
    x: required("x"),
    y: required("y"),
    z: required("z"),
    vx: required("vx"),
    vy: required("vy"),
    vz: required("vz"),
    a: element("a"),
    e: element("e"),
    inc: element("inc"),
    raan: element("raan"),
    omega: element("omega"),
    nu: element("nu"),
    entityPath: typeof read.entityPath === "string" ? read.entityPath : undefined,
  } as OrbitPoint;
}

/**
 * Parse a single CSV data line into an OrbitPoint, or return null if invalid.
 *
 * @param line - CSV data line
 * @param multiSat - If true, first field is satellite_id (string), rest are numeric.
 *   The presence of `# satellites = ...` header implies the satellite_id column exists,
 *   even for single-satellite files. This matches `orts run` output format where
 *   multi-sat CSV always includes the id column regardless of satellite count.
 *
 * Single-sat format: `t,x,y,z,vx,vy,vz[,a,e,inc,raan,omega,nu]`
 * Multi-sat format:  `satellite_id,t,x,y,z,vx,vy,vz[,a,e,inc,raan,omega,nu]`
 * Minimum 7 numeric fields required.
 */
export function parseDataLine(line: string, multiSat = false): OrbitPoint | null {
  const parts = line.split(",").map((s) => s.trim());

  let entityPath: string | undefined;
  let numericParts: string[];

  if (multiSat) {
    if (parts.length < 8) return null; // id + 7 numeric
    entityPath = parts[0];
    numericParts = parts.slice(1);
  } else {
    if (parts.length < 7) return null;
    numericParts = parts;
  }

  const nums = numericParts.map(Number);
  if (nums.some(Number.isNaN)) return null;

  return {
    t: nums[0],
    x: nums[1],
    y: nums[2],
    z: nums[3],
    vx: nums[4],
    vy: nums[5],
    vz: nums[6],
    a: nums[7] ?? 0,
    e: nums[8] ?? 0,
    inc: nums[9] ?? 0,
    raan: nums[10] ?? 0,
    omega: nums[11] ?? 0,
    nu: nums[12] ?? 0,
    entityPath,
    accel_gravity: 0,
    accel_drag: 0,
    accel_srp: 0,
    accel_third_body_sun: 0,
    accel_third_body_moon: 0,
  };
}

/**
 * Create a fresh CSVMetadata object with all fields null.
 */
export function emptyMetadata(): CSVMetadata {
  return {
    epochJd: null,
    mu: null,
    centralBody: null,
    centralBodyRadius: null,
    satelliteName: null,
    satellites: null,
  };
}
