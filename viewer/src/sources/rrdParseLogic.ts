/**
 * RRD parse worker message types.
 *
 * Shared between the Worker and the main thread (RrdFileAdapter).
 */

import { TORQUE_AXES, TORQUE_CHART_MODELS } from "../chartMetrics.js";
import { type OrbitPoint, torqueComponent } from "../orbit.js";

import type { RrdMetadata } from "../wasm/rrdWasmInit.js";

/** Messages from main thread → Worker */
export type RrdWorkerInput = {
  type: "parse";
  buffer: ArrayBuffer;
};

/** Messages from Worker → main thread */
export type RrdWorkerMessage =
  | { type: "metadata"; metadata: RrdMetadata }
  | { type: "chunk"; points: RrdPointOut[]; done: boolean }
  | { type: "error"; message: string };

/** A single point output from the Worker (raw state vector, no Keplerian). */
export interface RrdPointOut {
  t: number;
  x: number;
  y: number;
  z: number;
  vx: number;
  vy: number;
  vz: number;
  entityPath: string | null;
  qw?: number;
  qx?: number;
  qy?: number;
  qz?: number;
  wx?: number;
  wy?: number;
  wz?: number;
  /** `torque_<model>_<axis>` for the models the charts know. */
  [torqueColumn: `torque_${string}`]: number | undefined;
}

/** One decoded row as rrd-wasm hands it over. */
export interface RrdRowIn {
  t: number;
  x: number;
  y: number;
  z: number;
  vx: number;
  vy: number;
  vz: number;
  entity_path: string | null;
  /** Four components or nothing — see {@link rowToPoint}. */
  quaternion?: readonly [number, number, number, number] | null;
  angular_velocity?: readonly [number, number, number] | null;
  /** One triple per model, or nothing — the decoder reports a torque only
   * where all three axes are present. */
  torque_gravity_gradient?: readonly [number, number, number] | null;
  torque_panel_srp?: readonly [number, number, number] | null;
  torque_panel_drag?: readonly [number, number, number] | null;
}

/**
 * One decoded row as a point the viewer can draw.
 *
 * Lives here rather than in the worker so the boundary can be tested: what a
 * malformed row turns into is the whole question, and a worker's message handler
 * is not reachable from a unit test.
 */
export function rowToPoint(row: RrdRowIn): RrdPointOut {
  const point: RrdPointOut = {
    t: row.t,
    x: row.x,
    y: row.y,
    z: row.z,
    vx: row.vx,
    vy: row.vy,
    vz: row.vz,
    entityPath: row.entity_path,
  };

  // Attitude is optional, and arrives whole or not at all: the decoder builds
  // `Some([qw?, qx?, qy?, qz?])`, so a row missing any one component yields
  // `None` rather than a short list (`rrd-wasm/src/lib.rs`). A complete tuple can
  // still carry `NaN` — a diverged simulation writes one — and that reaches the
  // display frame as an attitude to refuse, which is the behaviour wanted.
  if (row.quaternion) {
    point.qw = row.quaternion[0];
    point.qx = row.quaternion[1];
    point.qy = row.quaternion[2];
    point.qz = row.quaternion[3];
  }
  if (row.angular_velocity) {
    point.wx = row.angular_velocity[0];
    point.wy = row.angular_velocity[1];
    point.wz = row.angular_velocity[2];
  }

  // Whole or not at all, as the attitude is: the decoder leaves a model's
  // torque out entirely unless all three axes were logged.
  for (const model of TORQUE_CHART_MODELS) {
    const triple = row[`torque_${model}` as keyof RrdRowIn] as
      | readonly [number, number, number]
      | null
      | undefined;
    if (!triple) continue;
    TORQUE_AXES.forEach((axis, i) => {
      point[`torque_${model}_${axis}` as `torque_${string}`] = triple[i];
    });
  }
  return point;
}

/** Models whose whole torque triple appears in these points, per entity.
 *
 * A recording's columns are the union over its satellites, so the presence of
 * a column says nothing about a given satellite: what counts is a triple
 * actually decoded for it. A model reporting `[0, 0, 0]` is a model that was
 * there, and is counted.
 */
export function torqueModelsOf(
  points: readonly OrbitPoint[],
  into: Map<string, Set<string>> = new Map(),
): Map<string, Set<string>> {
  for (const point of points) {
    const entity = point.entityPath ?? "default";
    for (const model of TORQUE_CHART_MODELS) {
      const complete = TORQUE_AXES.every(
        (axis) => torqueComponent(point, `torque_${model}_${axis}`) !== undefined,
      );
      if (!complete) continue;
      let models = into.get(entity);
      if (!models) {
        models = new Set();
        into.set(entity, models);
      }
      models.add(model);
    }
  }
  return into;
}
