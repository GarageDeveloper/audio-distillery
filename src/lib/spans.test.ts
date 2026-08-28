import { describe, expect, it } from "vitest";
import { clampSpanToFreeHole, clampEdgeToNeighbors, snapToClipBoundary } from "./spans";

const tracks = [
  { id: 1, start: 100, end: 200 },
  { id: 2, start: 400, end: 500 },
];

describe("clampSpanToFreeHole", () => {
  it("butts a growing selection against the neighbouring tracks", () => {
    // Anchor in the hole between the two tracks.
    expect(clampSpanToFreeHole(300, 50, tracks)).toEqual({ start: 200, end: 300 });
    expect(clampSpanToFreeHole(300, 470, tracks)).toEqual({ start: 300, end: 400 });
    expect(clampSpanToFreeHole(300, 350, tracks)).toEqual({ start: 300, end: 350 });
  });
  it("is open-ended before the first and after the last track", () => {
    expect(clampSpanToFreeHole(50, 0, tracks)).toEqual({ start: 0, end: 50 });
    expect(clampSpanToFreeHole(600, 9999, tracks)).toEqual({ start: 600, end: 9999 });
    expect(clampSpanToFreeHole(600, 450, tracks)).toEqual({ start: 500, end: 600 });
  });
  it("keeps legacy behaviour when the anchor sits inside a track", () => {
    expect(clampSpanToFreeHole(150, 450, tracks)).toEqual({ start: 150, end: 450 });
  });
});

describe("clampEdgeToNeighbors", () => {
  it("clamps an edge against neighbours and its own minimum length", () => {
    expect(clampEdgeToNeighbors(2, "start", 150, tracks, 10)).toBe(200);
    expect(clampEdgeToNeighbors(1, "end", 450, tracks, 10)).toBe(400);
    expect(clampEdgeToNeighbors(1, "start", 195, tracks, 10)).toBe(190);
    expect(clampEdgeToNeighbors(1, "start", 120, tracks, 10)).toBe(120);
  });
});

describe("snapToClipBoundary", () => {
  it("magnetizes within tolerance only", () => {
    expect(snapToClipBoundary(103, [100, 500], 8)).toEqual({ pos: 100, snapped: true });
    expect(snapToClipBoundary(120, [100, 500], 8)).toEqual({ pos: 120, snapped: false });
    expect(snapToClipBoundary(100, [100], 8)).toEqual({ pos: 100, snapped: true });
  });
});
