import { useMemo } from "react";
import * as THREE from "three";
import { axisLabelPositions, axisLabelScale } from "../axisLabels.js";

/**
 * Letters in `AxesHelper`'s X, Y, Z order, in colours that match the axes it
 * draws — brightened a little, since a letter is a thinner mark than a line and
 * the pure axis colours read darker at that size.
 */
const AXES = [
  { letter: "X", color: "#ff6b6b" },
  { letter: "Y", color: "#6bff6b" },
  { letter: "Z", color: "#7aa9ff" },
] as const;

/**
 * One canvas texture per letter, made on first use and kept.
 *
 * Lazily, not at module scope: the module is imported by tests running without a
 * DOM, and a canvas at import time would throw there.
 */
const textureCache = new Map<string, THREE.Texture>();

/** Above the scene's meshes, which use the default 0. */
const LABEL_RENDER_ORDER = 10;

function letterTexture(letter: string, color: string): THREE.Texture {
  const key = `${letter}:${color}`;
  const cached = textureCache.get(key);
  if (cached) return cached;

  const size = 128;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (ctx) {
    ctx.font = `bold ${size * 0.76}px ui-sans-serif, system-ui, sans-serif`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    // Outline first: a letter over a bright 3D model needs its own contrast, and
    // the scene's background cannot be relied on behind it.
    ctx.lineWidth = size * 0.1;
    ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
    ctx.strokeText(letter, size / 2, size * 0.54);
    ctx.fillStyle = color;
    ctx.fillText(letter, size / 2, size * 0.54);
  }

  const texture = new THREE.CanvasTexture(canvas);
  texture.colorSpace = THREE.SRGBColorSpace;
  textureCache.set(key, texture);
  return texture;
}

interface AxisLabelsProps {
  /** Axis length in scene units — the labels sit just past each tip. */
  length: number;
  /** Matches the axes' own opacity, so a dimmed triad gets dimmed letters. */
  opacity?: number;
}

/**
 * X / Y / Z at the tips of an axis triad.
 *
 * Sprites rather than 3D text: they face the camera from every angle, need no
 * font asset (the letters are drawn with the browser's own canvas text), and
 * carry no dependency. Placed inside the triad's group, so a label follows the
 * axis it names when the group rotates — a sprite ignores inherited *rotation*
 * but not inherited position.
 *
 * Both triads in the attitude view are RGB, and the axes alone leave a reader to
 * infer which line is which from the colour convention. The letters say it.
 */
export function AxisLabels({ length, opacity = 1 }: AxisLabelsProps) {
  const labels = useMemo(
    () =>
      AXES.map((axis, i) => ({
        ...axis,
        texture: letterTexture(axis.letter, axis.color),
        position: axisLabelPositions(length)[i],
      })),
    [length],
  );
  const scale = axisLabelScale(length);

  return (
    <>
      {labels.map((label) => (
        <sprite
          key={label.letter}
          position={label.position}
          scale={[scale, scale, scale]}
          // Drawn last and without a depth test, so a letter on an axis pointing
          // away from the camera stays readable instead of hiding inside the
          // spacecraft. The axis *line* is still depth-tested, so it disappearing
          // into the body is what tells the reader that axis points away.
          renderOrder={LABEL_RENDER_ORDER}
        >
          <spriteMaterial
            map={label.texture}
            transparent
            opacity={opacity}
            depthTest={false}
            depthWrite={false}
            toneMapped={false}
          />
        </sprite>
      ))}
    </>
  );
}
