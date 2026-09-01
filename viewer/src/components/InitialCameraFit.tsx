import { useThree } from "@react-three/fiber";
import { useEffect, useRef } from "react";
import { cameraDistanceForSpan, NOMINAL_SPACECRAFT_SPAN } from "../spacecraftScale.js";

/**
 * Pull the camera back once, if the viewport turns out to be narrower than the
 * default framing assumed.
 *
 * The `camera` prop is read at mount, before the canvas has a size, so a portrait
 * embedding would otherwise clip the axes sideways. Applied on the first sizing
 * only: reframing on every resize would undo a zoom the viewer had chosen, and
 * this is a starting view, not a constraint.
 */
export function InitialCameraFit({ fov }: { fov: number }) {
  const camera = useThree((s) => s.camera);
  const size = useThree((s) => s.size);
  const applied = useRef(false);
  useEffect(() => {
    if (applied.current || size.width === 0 || size.height === 0) return;
    applied.current = true;
    const needed = cameraDistanceForSpan(NOMINAL_SPACECRAFT_SPAN, fov, size.width / size.height);
    const current = camera.position.length();
    if (current > 0 && needed > current) camera.position.multiplyScalar(needed / current);
  }, [camera, size, fov]);
  return null;
}
