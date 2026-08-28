/** Pure span math for selection/edge interactions: tracks may never
 * overlap, so every gesture is clamped ("en butée") against the
 * existing regions on the DISPLAY side too — the backend stays the
 * authority, this only makes the gesture honest while it happens. */

export interface Span {
  start: number;
  end: number;
}

/** Clamp a growing selection so it stays inside the free hole around
 * its anchor: the moving end butts against the neighbouring tracks.
 * An anchor sitting INSIDE a track keeps the legacy behaviour (the
 * backend trims at add time). */
export function clampSpanToFreeHole(
  anchor: number,
  sample: number,
  tracks: Span[]
): Span {
  const inside = tracks.some((t) => anchor > t.start && anchor < t.end);
  let lo = 0;
  let hi = Infinity;
  if (!inside) {
    for (const t of tracks) {
      if (t.end <= anchor) lo = Math.max(lo, t.end);
      if (t.start >= anchor) hi = Math.min(hi, t.start);
    }
  }
  const s = Math.min(Math.max(sample, lo), hi);
  return { start: Math.min(anchor, s), end: Math.max(anchor, s) };
}

/** Clamp a region-edge drag against its neighbours and its own minimum
 * length — mirrors the backend's move_edge clamping so the visual flag
 * never promises a position the backend will refuse. */
export function clampEdgeToNeighbors(
  id: number,
  edge: "start" | "end",
  pos: number,
  tracks: (Span & { id: number })[],
  minLen: number
): number {
  const own = tracks.find((t) => t.id === id);
  if (!own) return pos;
  let prevEnd = 0;
  let nextStart = Infinity;
  for (const t of tracks) {
    if (t.id === id) continue;
    if (t.end <= own.start) prevEnd = Math.max(prevEnd, t.end);
    if (t.start >= own.end) nextStart = Math.min(nextStart, t.start);
  }
  if (edge === "start") {
    return Math.min(Math.max(pos, prevEnd), own.end - minLen);
  }
  return Math.min(Math.max(pos, own.start + minLen), nextStart);
}

/** Magnetic snap to the nearest clip boundary within `tol` samples —
 * the "feel" when a track edge reaches a file frontier. */
export function snapToClipBoundary(
  pos: number,
  boundaries: number[],
  tol: number
): { pos: number; snapped: boolean } {
  let best = pos;
  let bestDist = tol;
  let found = false;
  for (const b of boundaries) {
    const d = Math.abs(pos - b);
    if (d <= bestDist) {
      best = b;
      bestDist = d;
      found = true;
    }
  }
  return { pos: best, snapped: found };
}
