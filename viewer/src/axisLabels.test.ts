import { describe, expect, it } from "vitest";
import { axisLabelPositions, axisLabelScale } from "./axisLabels.js";
import { axisLengthForSpan, frameAxisLengthForSpan } from "./spacecraftScale.js";

describe("axis label placement", () => {
  it("puts each letter on its own axis and nowhere else", () => {
    const [x, y, z] = axisLabelPositions(2);
    expect(x[1]).toBe(0);
    expect(x[2]).toBe(0);
    expect(y[0]).toBe(0);
    expect(y[2]).toBe(0);
    expect(z[0]).toBe(0);
    expect(z[1]).toBe(0);
    expect(x[0]).toBeGreaterThan(0);
    expect(y[1]).toBeGreaterThan(0);
    expect(z[2]).toBeGreaterThan(0);
  });

  it("sits past the tip, clear of the line's end", () => {
    // On the tip the letter overlaps the line it names; too far and it reads as a
    // separate object.
    for (const length of [0.03, 0.75, 2]) {
      const [x] = axisLabelPositions(length);
      expect(x[0]).toBeGreaterThan(length);
      expect(x[0]).toBeLessThan(length * 1.5);
    }
  });

  it("scales with the axis, and stays shorter than it", () => {
    for (const length of [0.03, 0.75, 2]) {
      expect(axisLabelScale(length)).toBeCloseTo(axisLabelScale(1) * length, 12);
      expect(axisLabelScale(length)).toBeLessThan(length);
    }
  });

  it("keeps the body letters clear of the spacecraft, inside the frame letters", () => {
    // The attitude view draws both triads about one spacecraft, so the two sets of
    // letters must not land on top of each other.
    const span = 1;
    const [bodyX] = axisLabelPositions(axisLengthForSpan(span));
    const [frameX] = axisLabelPositions(frameAxisLengthForSpan(span));
    expect(bodyX[0]).toBeGreaterThan(span / 2);
    expect(frameX[0]).toBeGreaterThan(bodyX[0] + axisLabelScale(axisLengthForSpan(span)));
  });
});
