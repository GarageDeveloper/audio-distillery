import { describe, expect, it } from "vitest";
import { clampViewport, sampleToX, xToSample, zoomAt } from "./viewport";

const WIDTH = 1000;
const TOTAL = 1_000_000;

describe("coordinate conversion", () => {
  it("round-trips sample ↔ x", () => {
    const vp = { start: 5000, spp: 20 };
    expect(sampleToX(5000, vp)).toBe(0);
    expect(sampleToX(25000, vp)).toBe(1000);
    expect(xToSample(sampleToX(12345, vp), vp)).toBeCloseTo(12345);
  });
});

describe("clampViewport", () => {
  it("keeps the viewport inside the file", () => {
    const vp = clampViewport({ start: -500, spp: 10 }, WIDTH, TOTAL, 1);
    expect(vp.start).toBe(0);
    const vp2 = clampViewport({ start: TOTAL, spp: 10 }, WIDTH, TOTAL, 1);
    expect(vp2.start + WIDTH * vp2.spp).toBeLessThanOrEqual(TOTAL);
  });
  it("caps zoom-out at the whole file", () => {
    const vp = clampViewport({ start: 0, spp: 1e9 }, WIDTH, TOTAL, 1);
    expect(vp.spp).toBe(TOTAL / WIDTH);
  });
});

describe("zoomAt", () => {
  it("keeps the sample under the anchor stationary", () => {
    const vp = { start: 100_000, spp: 100 };
    const anchorX = 400;
    const before = xToSample(anchorX, vp);
    const zoomed = zoomAt(vp, anchorX, 0.5, WIDTH, TOTAL, 1);
    expect(xToSample(anchorX, zoomed)).toBeCloseTo(before, 3);
    expect(zoomed.spp).toBe(50);
  });
});

import { edgeScrollVelocity } from "./viewport";

describe("edgeScrollVelocity", () => {
  it("is zero in the middle and ramps in the zones", () => {
    expect(edgeScrollVelocity(500, 1000, 40, 18)).toBe(0);
    expect(edgeScrollVelocity(40, 1000, 40, 18) === 0).toBe(true);
    expect(edgeScrollVelocity(0, 1000, 40, 18)).toBe(-18);
    expect(edgeScrollVelocity(20, 1000, 40, 18)).toBeCloseTo(-9);
    expect(edgeScrollVelocity(1000, 1000, 40, 18)).toBe(18);
    expect(edgeScrollVelocity(980, 1000, 40, 18)).toBeCloseTo(9);
  });
  it("saturates beyond the edges and disables on tiny widths", () => {
    expect(edgeScrollVelocity(-50, 1000, 40, 18)).toBe(-18);
    expect(edgeScrollVelocity(1100, 1000, 40, 18)).toBe(18);
    expect(edgeScrollVelocity(10, 60, 40, 18)).toBe(0);
  });
});
