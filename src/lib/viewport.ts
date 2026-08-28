// Pure screen ↔ time conversion helpers, for DISPLAY purposes only (ARCHITECTURE.md §3:
// any authoritative position decision belongs to the backend).

export interface Viewport {
  /** First visible sample. */
  start: number;
  /** Samples per CSS pixel. */
  spp: number;
}

export function sampleToX(sample: number, vp: Viewport): number {
  return (sample - vp.start) / vp.spp;
}

export function xToSample(x: number, vp: Viewport): number {
  return vp.start + x * vp.spp;
}

export function visibleSpan(widthPx: number, vp: Viewport): number {
  return widthPx * vp.spp;
}

/** Clamp the viewport so it stays inside the file. */
export function clampViewport(
  vp: Viewport,
  widthPx: number,
  totalSamples: number,
  minSpp: number
): Viewport {
  const maxSpp = Math.max(totalSamples / widthPx, minSpp);
  const spp = Math.min(Math.max(vp.spp, minSpp), maxSpp);
  const span = widthPx * spp;
  let start = vp.start;
  if (start + span > totalSamples) start = totalSamples - span;
  if (start < 0) start = 0;
  return { start, spp };
}

/** Zoom by `factor` keeping the sample under `anchorX` stationary. */
export function zoomAt(
  vp: Viewport,
  anchorX: number,
  factor: number,
  widthPx: number,
  totalSamples: number,
  minSpp: number
): Viewport {
  const anchorSample = xToSample(anchorX, vp);
  const spp = vp.spp * factor;
  const next = { start: anchorSample - anchorX * spp, spp };
  return clampViewport(next, widthPx, totalSamples, minSpp);
}

/** Signed auto-scroll velocity (px/frame) when a drag sits within `zone`
 * px of either edge — 0 in the middle, proportional inside the zone,
 * clamped to ±`max` (positions beyond the edges saturate). */
export function edgeScrollVelocity(
  x: number,
  width: number,
  zone: number,
  max: number
): number {
  if (width <= 2 * zone) return 0;
  if (x < zone) {
    const depth = Math.min(1, (zone - x) / zone);
    return -depth * max;
  }
  if (x > width - zone) {
    const depth = Math.min(1, (x - (width - zone)) / zone);
    return depth * max;
  }
  return 0;
}
