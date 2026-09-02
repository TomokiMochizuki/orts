/**
 * Where the X / Y / Z letters sit on an axis triad, and how large they are.
 *
 * Pure so the placement is testable without a renderer: a label that lands on
 * the axis line, or inside the spacecraft, is the failure mode worth pinning.
 */

/** Letter position along its axis, as a multiple of the axis length. */
const TIP_OVERSHOOT = 1.16;

/** Letter height, as a fraction of the axis length. */
const LABEL_SCALE = 0.3;

/** Letter positions in the triad's own frame, in X, Y, Z order. */
export function axisLabelPositions(length: number): [number, number, number][] {
  const d = length * TIP_OVERSHOOT;
  return [
    [d, 0, 0],
    [0, d, 0],
    [0, 0, d],
  ];
}

/** Letter height in scene units, so labels scale with the triad they name. */
export function axisLabelScale(length: number): number {
  return length * LABEL_SCALE;
}
