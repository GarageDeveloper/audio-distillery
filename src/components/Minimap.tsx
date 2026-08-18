import { useCallback, useEffect, useRef } from "react";
import type { ProjectView } from "../types/ProjectView";
import type { PeakSlice } from "../types/PeakSlice";
import { api } from "../api";
import type { Viewport } from "../lib/viewport";
import { clampViewport } from "../lib/viewport";

interface Props {
  view: ProjectView;
  viewport: Viewport;
  width: number;
  playheadSample: number;
  onViewportChange: (vp: Viewport) => void;
}

function css(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** Global overview strip: whole file, viewport rectangle, markers, playhead. */
export function Minimap(p: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sliceRef = useRef<PeakSlice | null>(null);
  const sizeRef = useRef({ w: 0, h: 64 });
  const dragRef = useRef<{ grabOffset: number } | null>(null);
  const propsRef = useRef(p);
  propsRef.current = p;

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { view, viewport, playheadSample } = propsRef.current;
    const { w, h } = sizeRef.current;
    if (!w) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    const total = view.audio.duration_samples;
    const toX = (sample: number) => (sample / total) * w;

    ctx.fillStyle = css("--bg-deep");
    ctx.fillRect(0, 0, w, h);

    // Mono waveform (all channels merged).
    const slice = sliceRef.current;
    if (slice) {
      const cy = h / 2;
      const amp = h * 0.4;
      ctx.fillStyle = css("--minimap-wave");
      ctx.globalAlpha = 0.75;
      const buckets = slice.channels[0].length / 2;
      for (let b = 0; b < buckets; b++) {
        let mn = 0;
        let mx = 0;
        for (const ch of slice.channels) {
          mn = Math.min(mn, ch[b * 2] / 127);
          mx = Math.max(mx, ch[b * 2 + 1] / 127);
        }
        const x = toX(slice.start_sample + b * slice.samples_per_bucket);
        const bw = Math.max(slice.samples_per_bucket / (total / w), 1);
        ctx.fillRect(x, cy - mx * amp, bw, Math.max((mx - mn) * amp, 1));
      }
      ctx.globalAlpha = 1;
    }

    // Clip boundaries (same vivid color as the waveform clip frames).
    if (view.audio.clips.length > 1) {
      ctx.strokeStyle = css("--clip");
      ctx.lineWidth = 2;
      for (const c of view.audio.clips.slice(1)) {
        const x = Math.round(toX(c.start_sample));
        ctx.beginPath();
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
        ctx.stroke();
      }
      ctx.lineWidth = 1;
    }

    // Track regions (audio outside them is ignored at export).
    ctx.fillStyle = css("--copper");
    ctx.globalAlpha = 0.18;
    for (const t of view.tracks) {
      const x0 = toX(t.start_sample);
      const x1 = toX(t.end_sample);
      ctx.fillRect(x0, 0, x1 - x0, h);
    }
    ctx.globalAlpha = 0.55;
    ctx.strokeStyle = css("--copper");
    for (const t of view.tracks) {
      for (const s of [t.start_sample, t.end_sample]) {
        const x = Math.round(toX(s)) + 0.5;
        ctx.beginPath();
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
        ctx.stroke();
      }
    }
    ctx.globalAlpha = 1;

    // Viewport rectangle.
    const span = propsRef.current.width * viewport.spp;
    const x0 = toX(viewport.start);
    const x1 = toX(Math.min(viewport.start + span, total));
    ctx.fillStyle = css("--copper-dim");
    ctx.strokeStyle = css("--copper");
    ctx.beginPath();
    ctx.roundRect(x0, 1, Math.max(x1 - x0, 4), h - 2, 3);
    ctx.fill();
    ctx.stroke();

    // Playhead.
    const px = Math.round(toX(playheadSample)) + 0.5;
    ctx.strokeStyle = css("--playhead");
    ctx.beginPath();
    ctx.moveTo(px, 0);
    ctx.lineTo(px, h);
    ctx.stroke();
  }, []);

  // Fetch whole-file peaks once per file / width bucket.
  useEffect(() => {
    const w = Math.max(Math.round(sizeRef.current.w || p.width), 200);
    api
      .getPeaks(0, p.view.audio.duration_samples, w)
      .then((slice) => {
        sliceRef.current = slice;
        draw();
      })
      .catch(() => {});
  }, [p.view.audio.path, p.view.audio.duration_samples, p.width, draw]);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      sizeRef.current = { w: r.width, h: r.height };
      draw();
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [draw]);

  useEffect(() => {
    draw();
  }, [p.viewport, p.playheadSample, p.view, draw]);

  const sampleAt = (clientX: number) => {
    const rect = wrapRef.current!.getBoundingClientRect();
    const x = clientX - rect.left;
    return (x / rect.width) * propsRef.current.view.audio.duration_samples;
  };

  const moveViewportTo = (centerSample: number, grabOffset?: number) => {
    const { viewport, width, view, onViewportChange } = propsRef.current;
    const span = width * viewport.spp;
    const start =
      grabOffset !== undefined ? centerSample - grabOffset : centerSample - span / 2;
    onViewportChange(
      clampViewport({ start, spp: viewport.spp }, width, view.audio.duration_samples, 1)
    );
  };

  return (
    <div
      ref={wrapRef}
      className="minimap-wrap"
      style={{ cursor: dragRef.current ? "grabbing" : "grab" }}
      onPointerDown={(e) => {
        const s = sampleAt(e.clientX);
        const { viewport, width } = propsRef.current;
        const span = width * viewport.spp;
        if (s >= viewport.start && s <= viewport.start + span) {
          dragRef.current = { grabOffset: s - viewport.start };
        } else {
          dragRef.current = { grabOffset: span / 2 };
          moveViewportTo(s);
        }
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        if (!dragRef.current) return;
        moveViewportTo(sampleAt(e.clientX), dragRef.current.grabOffset);
      }}
      onPointerUp={() => {
        dragRef.current = null;
      }}
    >
      <canvas ref={canvasRef} className="minimap-canvas" />
    </div>
  );
}
