import { describe, expect, it } from "vitest";

import { upsertSatellite } from "./eventDispatcher.js";
import type { SatelliteInfo, SimInfo } from "./types.js";

function satellite(id: string, perturbations: string[] = []): SatelliteInfo {
  return {
    id,
    name: id,
    altitude: 400,
    period: 5544,
    perturbations,
    shape: null,
  } as SatelliteInfo;
}

function info(satellites: SatelliteInfo[]): SimInfo {
  return {
    mu: 398600.4418,
    dt: 1,
    output_interval: 10,
    stream_interval: 10,
    central_body: "earth",
    central_body_radius: 6378.137,
    satellites,
  } as SimInfo;
}

describe("upsertSatellite", () => {
  it("adds a satellite the snapshot does not have", () => {
    const merged = upsertSatellite(info([satellite("sat-a")]), satellite("sat-b", ["panel_srp"]));

    expect(merged?.satellites.map((s) => s.id)).toEqual(["sat-a", "sat-b"]);
    expect(merged?.satellites[1].perturbations).toEqual(["panel_srp"]);
  });

  // Replacing in place rather than appending keeps the order stable, so a
  // repeated announcement does not reorder what the viewer lists.
  it("replaces the entry with the same id, in place", () => {
    const merged = upsertSatellite(
      info([satellite("sat-a"), satellite("sat-b"), satellite("sat-c")]),
      satellite("sat-b", ["gravity_gradient"]),
    );

    expect(merged?.satellites.map((s) => s.id)).toEqual(["sat-a", "sat-b", "sat-c"]);
    expect(merged?.satellites[1].perturbations).toEqual(["gravity_gradient"]);
  });

  it("leaves the other entries alone", () => {
    const before = info([satellite("sat-a", ["zonal_gravity"])]);
    const merged = upsertSatellite(before, satellite("sat-b"));

    expect(merged?.satellites[0]).toBe(before.satellites[0]);
    expect(merged).not.toBe(before);
  });

  // The server sends `info` on connect, before any announcement, so there is
  // nothing to merge into until it arrives.
  it("does nothing without a snapshot", () => {
    expect(upsertSatellite(null, satellite("sat-a"))).toBeNull();
  });
});
