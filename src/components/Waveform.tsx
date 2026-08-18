import { useCallback, useEffect, useRef } from "react";
import type { ProjectView } from "../types/ProjectView";
import type { PeakSlice } from "../types/PeakSlice";
import type { RegionSpan } from "../types/RegionSpan";
import type { RegionEdge } from "../types/RegionEdge";
import { api } from "../api";
import { formatTimecode, formatDuration } from "../lib/format";
import type { Viewport } from "../lib/viewport";
import { sampleToX, xToSample, zoomAt, clampViewport } from "../lib/viewport";

const RULER_H = 26;
const EDGE_HIT_PX = 14; // half of the 28 px grab target
const FLAG_W = 26;
const FLAG_H = 20;

interface Props {
  view: ProjectView;
  viewport: Viewport;
  playheadSample: number;
  /** "mix" = summed waveform; "layers" = one lane per layer. */
  waveMode: "mix" | "layers";
  proposals: RegionSpan[] | null;
  /// Auto-split candidates rejected by the minimum-length filter (faint).
  ignoredProposals?: RegionSpan[] | null;
  selection: RegionSpan | null;
  pendingStart: number | null;
  selectedTrack: number | null;
  onWidthChange: (w: number) => void;
  onViewportChange: (vp: Viewport) => void;
  onSeek: (sample: number) => void;
  onSelectionChange: (sel: RegionSpan | null) => void;
  onAddRegion: (start: number, end: number) => void;
  onMoveEdge: (id: number, edge: RegionEdge, sample: number) => void;
  onSelectTrack: (id: number | null) => void;
  onRemoveRegion: (id: number) => void;
}

interface EdgeRef {
  id: number;
  edge: RegionEdge;
  sample: number;
}

type DragState =
  | { type: "edge"; id: number; edge: RegionEdge; pos: number; moved: boolean }
  | { type: "select"; anchor: number; moved: boolean };

/** Read a design token so canvas drawing follows the active theme. */
function css(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** Pick a "nice" ruler step (in seconds) giving labels every ≥ minPx. */
export function rulerStep(spp: number, sampleRate: number, minPx = 110): number {
  const steps = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];
  const minSeconds = (minPx * spp) / sampleRate;
  return steps.find((s) => s >= minSeconds) ?? 7200;
}

export function Waveform(p: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const sizeRef = useRef({ w: 0, h: 0 });
  const sliceRef = useRef<PeakSlice | null>(null);
  const lanesRef = useRef<PeakSlice[] | null>(null);
  const fetchSeq = useRef(0);
  const hoverEdge = useRef<EdgeRef | null>(null);
  const drag = useRef<DragState | null>(null);

  const propsRef = useRef(p);
  propsRef.current = p;

  /** Region edges with live drag position applied. */
  const edges = useCallback((): EdgeRef[] => {
    const { view } = propsRef.current;
    const d = drag.current;
    const out: EdgeRef[] = [];
    for (const t of view.tracks) {
      for (const edge of ["start", "end"] as RegionEdge[]) {
        let sample = edge === "start" ? t.start_sample : t.end_sample;
        if (d?.type === "edge" && d.id === t.id && d.edge === edge) {
          sample = d.pos;
        }
        out.push({ id: t.id, edge, sample });
      }
    }
    return out;
  }, []);

  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const { view, viewport, playheadSample, proposals, selection, pendingStart, selectedTrack } =
      propsRef.current;
    const { w, h } = sizeRef.current;
    if (w === 0 || h === 0) return;
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

    const sr = view.audio.sample_rate;
    const vp = viewport;
    const d = drag.current;

    // Live region spans (with a dragged edge applied).
    const regions = view.tracks.map((t) => {
      let start = t.start_sample;
      let end = t.end_sample;
      if (d?.type === "edge" && d.id === t.id) {
        if (d.edge === "start") start = d.pos;
        else end = d.pos;
      }
      return { ...t, start_sample: start, end_sample: end };
    });

    // Background + ruler strip.
    ctx.fillStyle = css("--wave-bg");
    ctx.fillRect(0, 0, w, h);
    ctx.fillStyle = css("--bg-deep");
    ctx.fillRect(0, 0, w, RULER_H);

    // Time grid + ruler labels.
    const step = rulerStep(vp.spp, sr);
    const gridColor = css("--grid-time");
    const textColor = css("--text-3");
    const firstTick = Math.floor(xToSample(0, vp) / sr / step) * step;
    const lastSec = xToSample(w, vp) / sr;
    ctx.font = "10px ui-monospace, Menlo, Consolas, monospace";
    ctx.textBaseline = "middle";
    for (let s = firstTick; s <= lastSec + step; s += step) {
      const x = Math.round(sampleToX(s * sr, vp)) + 0.5;
      if (x < -50 || x > w + 50) continue;
      ctx.strokeStyle = gridColor;
      ctx.beginPath();
      ctx.moveTo(x, RULER_H);
      ctx.lineTo(x, h);
      ctx.stroke();
      ctx.fillStyle = textColor;
      ctx.fillText(formatDuration(s), x + 4, RULER_H / 2);
    }
    ctx.strokeStyle = css("--line");
    ctx.beginPath();
    ctx.moveTo(0, RULER_H + 0.5);
    ctx.lineTo(w, RULER_H + 0.5);
    ctx.stroke();

    // Waveform: summed mix (per audio channel) or one lane per layer.
    const area = h - RULER_H;
    const peakColors = [css("--wave-l-peak"), css("--wave-r-peak")];
    const rmsColors = [css("--wave-l-rms"), css("--wave-r-rms")];
    const centerColor = css("--wave-center");
    const waveMode = propsRef.current.waveMode;

    const drawLane = (
      data: PeakSlice,
      chIdx: number | "mono",
      cy: number,
      amp: number,
      colorIdx: number,
      dim: boolean
    ) => {
      const spb = data.samples_per_bucket;
      const grad = ctx.createLinearGradient(0, cy - amp, 0, cy + amp);
      const lo = css("--copper-lo");
      grad.addColorStop(0, lo);
      grad.addColorStop(0.5, rmsColors[Math.min(colorIdx, 1)]);
      grad.addColorStop(1, lo);
      const buckets = data.channels[0]?.length ?? 0;
      const sampleAt = (b: number, off: 0 | 1) => {
        if (chIdx === "mono") {
          // Merge all channels of this slice into one lane.
          let v = off === 0 ? 127 : -127;
          for (const ch of data.channels) {
            const x = ch[b * 2 + off];
            v = off === 0 ? Math.min(v, x) : Math.max(v, x);
          }
          return v;
        }
        return data.channels[chIdx][b * 2 + off];
      };
      if (dim) ctx.globalAlpha = 0.35;
      ctx.fillStyle = peakColors[Math.min(colorIdx, 1)];
      for (let b = 0; b < buckets / 2; b++) {
        const s0 = data.start_sample + b * spb;
        const x = sampleToX(s0, vp);
        const bw = Math.max(spb / vp.spp, 1);
        if (x + bw < 0 || x > w) continue;
        const mn = (sampleAt(b, 0) / 127) * amp;
        const mx = (sampleAt(b, 1) / 127) * amp;
        ctx.fillRect(x, cy - Math.max(mx, 0), bw, Math.max(mx - mn, 1));
      }
      ctx.fillStyle = grad;
      for (let b = 0; b < buckets / 2; b++) {
        const s0 = data.start_sample + b * spb;
        const x = sampleToX(s0, vp);
        const bw = Math.max(spb / vp.spp, 1);
        if (x + bw < 0 || x > w) continue;
        const mn = (sampleAt(b, 0) / 127) * amp * 0.45;
        const mx = (sampleAt(b, 1) / 127) * amp * 0.45;
        ctx.fillRect(x, cy - Math.max(mx, 0), bw, Math.max(mx - mn, 1));
      }
      ctx.globalAlpha = 1;
    };

    if (waveMode === "layers" && lanesRef.current) {
      const lanes = lanesRef.current;
      const layerViews = view.layers;
      const n = Math.max(lanes.length, 1);
      const laneH = area / n;
      const amp = laneH * 0.42;
      ctx.font = "600 10px ui-monospace, Menlo, Consolas, monospace";
      void amp;
      for (let li = 0; li < lanes.length; li++) {
        if (li > 0) {
          ctx.strokeStyle = css("--line-soft");
          ctx.beginPath();
          ctx.moveTo(0, Math.round(RULER_H + laneH * li) + 0.5);
          ctx.lineTo(w, Math.round(RULER_H + laneH * li) + 0.5);
          ctx.stroke();
        }
        const muted = layerViews[li]?.muted ?? false;
        // Each layer shows its REAL channels: stereo files get two sub-lanes
        // (L/R, usual channel colors), mono files a single one.
        const chCount = Math.max(lanes[li].channels.length, 1);
        const subH = laneH / chCount;
        for (let c = 0; c < chCount; c++) {
          const scy = RULER_H + laneH * li + subH * (c + 0.5);
          ctx.strokeStyle = centerColor;
          ctx.beginPath();
          ctx.moveTo(0, Math.round(scy) + 0.5);
          ctx.lineTo(w, Math.round(scy) + 0.5);
          ctx.stroke();
          drawLane(lanes[li], c, scy, subH * 0.42, c, muted);
        }
        // Lane label: layer name + channel layout (+ muted flag).
        const name = layerViews[li]?.name ?? `Layer ${li + 1}`;
        const layout = chCount === 1 ? "mono" : chCount === 2 ? "stereo" : `${chCount} ch`;
        const label = `${name} · ${layout}${muted ? " · muted" : ""}`;
        ctx.fillStyle = css("--panel-2");
        const tw = ctx.measureText(label).width;
        ctx.beginPath();
        ctx.roundRect(6, RULER_H + laneH * li + 6, tw + 12, 16, 4);
        ctx.fill();
        ctx.fillStyle = css("--text-2");
        ctx.textBaseline = "middle";
        ctx.fillText(label, 12, RULER_H + laneH * li + 14);
      }
    } else {
      const slice = sliceRef.current;
      const chCount = slice?.channels.length ?? view.audio.channels;
      const laneH = area / chCount;
      const amp = laneH * 0.42;
      for (let c = 0; c < chCount; c++) {
        const cy = RULER_H + laneH * (c + 0.5);
        ctx.strokeStyle = centerColor;
        ctx.beginPath();
        ctx.moveTo(0, Math.round(cy) + 0.5);
        ctx.lineTo(w, Math.round(cy) + 0.5);
        ctx.stroke();
        if (slice?.channels[c]) {
          drawLane(slice, c, cy, amp, c, false);
        }
      }
    }

    // Dim everything outside track regions: that audio is ignored at export.
    const sorted = [...regions].sort((a, b) => a.start_sample - b.start_sample);
    ctx.fillStyle = css("--wave-bg");
    ctx.globalAlpha = 0.62;
    let cursor = 0;
    for (const r of sorted) {
      const x0 = sampleToX(cursor, vp);
      const x1 = sampleToX(r.start_sample, vp);
      if (x1 > 0 && x1 > x0) ctx.fillRect(Math.max(x0, 0), RULER_H, Math.min(x1, w) - Math.max(x0, 0), area);
      cursor = Math.max(cursor, r.end_sample);
    }
    const xEnd = sampleToX(cursor, vp);
    if (xEnd < w) ctx.fillRect(Math.max(xEnd, 0), RULER_H, w - Math.max(xEnd, 0), area);
    ctx.globalAlpha = 1;

    // Region tint + border + chip.
    ctx.font = "700 10px ui-monospace, Menlo, Consolas, monospace";
    const current = regions.find(
      (t) => playheadSample >= t.start_sample && playheadSample < t.end_sample
    );
    for (const t of regions) {
      const x0 = sampleToX(t.start_sample, vp);
      const x1 = sampleToX(t.end_sample, vp);
      if (x1 < 0 || x0 > w) continue;
      const isSelected = selectedTrack === t.id;
      ctx.fillStyle = css("--selection");
      ctx.fillRect(Math.max(x0, 0), RULER_H, Math.min(x1, w) - Math.max(x0, 0), area);
      if (isSelected) {
        ctx.fillStyle = css("--copper-dim");
        ctx.fillRect(Math.max(x0, 0), RULER_H, Math.min(x1, w) - Math.max(x0, 0), area);
      }

      // Chip: always "number · title"; fall back to the number alone when
      // the region is too narrow on screen for the full label.
      const highlighted = current?.id === t.id || isSelected;
      const visible = Math.min(x1, w) - Math.max(x0, 0);
      let label = `${t.number} · ${t.title}`;
      let tw = ctx.measureText(label).width;
      if (tw + 14 > visible - 6) {
        label = String(t.number);
        tw = ctx.measureText(label).width;
      }
      const cx = (Math.max(x0, 0) + Math.min(x1, w)) / 2;
      const bx = cx - tw / 2 - 7;
      const bw2 = tw + 14;
      if (bw2 <= visible - 6) {
        const by = RULER_H + 10;
        ctx.beginPath();
        ctx.roundRect(bx, by, bw2, 18, 9);
        ctx.fillStyle = highlighted ? css("--copper") : css("--panel-2");
        ctx.fill();
        ctx.fillStyle = highlighted ? css("--text-on-accent") : css("--text-2");
        ctx.textBaseline = "middle";
        ctx.fillText(label, bx + 7, by + 10);
      }
    }

    // Clips: each source file gets a vivid 2 px frame (theme token --clip)
    // and a solid name badge at the bottom, so the timeline composition is
    // obvious at a glance.
    if (view.audio.clips.length > 1) {
      const clipColor = css("--clip");
      ctx.save();
      for (const clip of view.audio.clips) {
        const x0 = sampleToX(clip.start_sample, vp);
        const x1 = sampleToX(clip.start_sample + clip.duration_samples, vp);
        if (x1 < 0 || x0 > w) continue;
        ctx.strokeStyle = clipColor;
        ctx.lineWidth = 2;
        ctx.globalAlpha = 0.9;
        ctx.strokeRect(x0 + 1, RULER_H + 1, x1 - x0 - 2, h - RULER_H - 2);
        ctx.globalAlpha = 1;
      }
      ctx.font = "700 11px ui-monospace, Menlo, Consolas, monospace";
      ctx.textBaseline = "middle";
      for (const clip of view.audio.clips) {
        const x0 = sampleToX(clip.start_sample, vp);
        const x1 = sampleToX(clip.start_sample + clip.duration_samples, vp);
        if (x1 < 0 || x0 > w) continue;
        const label = clip.name;
        const tw = ctx.measureText(label).width;
        const bx = Math.max(x0, 0) + 5;
        const bh = 20;
        const by = h - bh - 5;
        const bw = Math.min(tw + 16, Math.min(x1, w) - bx - 4);
        if (bw < 30) continue;
        ctx.fillStyle = clipColor;
        ctx.beginPath();
        ctx.roundRect(bx, by, bw, bh, 5);
        ctx.fill();
        ctx.fillStyle = css("--wave-bg");
        ctx.save();
        ctx.beginPath();
        ctx.roundRect(bx, by, bw, bh, 5);
        ctx.clip();
        ctx.fillText(label, bx + 8, by + bh / 2 + 1);
        ctx.restore();
      }
      ctx.restore();
    }

    // Silence-detection proposals (ghost regions). Candidates rejected by
    // the minimum-length filter stay barely visible so raising/lowering the
    // threshold gives immediate feedback.
    const { ignoredProposals } = propsRef.current;
    if (proposals) {
      ctx.save();
      ctx.strokeStyle = css("--copper");
      ctx.setLineDash([5, 4]);
      for (const r of proposals) {
        const x0 = sampleToX(r.start, vp);
        const x1 = sampleToX(r.end, vp);
        if (x1 < 0 || x0 > w) continue;
        ctx.globalAlpha = 0.18;
        ctx.fillStyle = css("--copper");
        ctx.fillRect(Math.max(x0, 0), RULER_H, Math.min(x1, w) - Math.max(x0, 0), area);
        ctx.globalAlpha = 0.6;
        ctx.strokeRect(Math.max(x0, 0) + 0.5, RULER_H + 1.5, Math.min(x1, w) - Math.max(x0, 0) - 1, area - 3);
      }
      ctx.restore();
    }
    if (ignoredProposals && ignoredProposals.length > 0) {
      ctx.save();
      ctx.strokeStyle = css("--text-3");
      ctx.setLineDash([2, 4]);
      ctx.globalAlpha = 0.35;
      for (const r of ignoredProposals) {
        const x0 = sampleToX(r.start, vp);
        const x1 = sampleToX(r.end, vp);
        if (x1 < 0 || x0 > w) continue;
        ctx.strokeRect(Math.max(x0, 0) + 0.5, RULER_H + 1.5, Math.min(x1, w) - Math.max(x0, 0) - 1, area - 3);
      }
      ctx.restore();
    }

    // Live selection (before it becomes a track).
    if (selection) {
      const x0 = sampleToX(selection.start, vp);
      const x1 = sampleToX(selection.end, vp);
      ctx.save();
      ctx.fillStyle = css("--copper-dim");
      ctx.fillRect(Math.max(x0, 0), RULER_H, Math.min(x1, w) - Math.max(x0, 0), area);
      ctx.strokeStyle = css("--copper-hi");
      ctx.setLineDash([5, 4]);
      for (const x of [x0, x1]) {
        if (x < 0 || x > w) continue;
        ctx.beginPath();
        ctx.moveTo(Math.round(x) + 0.5, RULER_H);
        ctx.lineTo(Math.round(x) + 0.5, h);
        ctx.stroke();
      }
      ctx.restore();
    }

    // Pending start marker (first M of the pair).
    if (pendingStart != null) {
      const x = Math.round(sampleToX(pendingStart, vp)) + 0.5;
      if (x >= -FLAG_W && x <= w + FLAG_W) {
        ctx.save();
        ctx.strokeStyle = css("--copper-hi");
        ctx.setLineDash([3, 3]);
        ctx.beginPath();
        ctx.moveTo(x, RULER_H);
        ctx.lineTo(x, h);
        ctx.stroke();
        ctx.restore();
        ctx.fillStyle = css("--copper-hi");
        ctx.font = "700 10px ui-monospace, Menlo, Consolas, monospace";
        ctx.textAlign = "left";
        ctx.fillText("start?", x + 5, RULER_H + FLAG_H / 2);
      }
    }

    // Region edge markers ("barrel label" flags: start opens →, end closes ←).
    const markerColor = css("--marker");
    for (const t of regions) {
      for (const edge of ["start", "end"] as RegionEdge[]) {
        const sample = edge === "start" ? t.start_sample : t.end_sample;
        const x = Math.round(sampleToX(sample, vp)) + 0.5;
        if (x < -FLAG_W || x > w + FLAG_W) continue;
        const isDragged = d?.type === "edge" && d.id === t.id && d.edge === edge;
        const isHover =
          hoverEdge.current?.id === t.id && hoverEdge.current?.edge === edge;
        const emph = isDragged || isHover || selectedTrack === t.id;

        ctx.save();
        ctx.shadowColor = css("--marker-glow");
        ctx.shadowBlur = emph ? 10 : 5;
        ctx.strokeStyle = markerColor;
        ctx.lineWidth = emph ? 3 : 1;
        ctx.beginPath();
        ctx.moveTo(x, RULER_H);
        ctx.lineTo(x, h);
        ctx.stroke();
        ctx.restore();

        // Dovetail flag on the inside of the region.
        const dir = edge === "start" ? 1 : -1;
        const fy = RULER_H;
        const grad = ctx.createLinearGradient(0, fy, 0, fy + FLAG_H);
        grad.addColorStop(0, css("--marker-flag-a"));
        grad.addColorStop(1, css("--marker-flag-b"));
        ctx.fillStyle = grad;
        ctx.beginPath();
        ctx.moveTo(x, fy);
        ctx.lineTo(x + dir * FLAG_W, fy);
        ctx.lineTo(x + dir * (FLAG_W - 6), fy + FLAG_H / 2);
        ctx.lineTo(x + dir * FLAG_W, fy + FLAG_H);
        ctx.lineTo(x, fy + FLAG_H);
        ctx.closePath();
        ctx.fill();
        ctx.fillStyle = css("--text-on-accent");
        ctx.font = "700 10px ui-monospace, Menlo, Consolas, monospace";
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(
          edge === "start" ? String(t.number) : "▏",
          x + dir * (FLAG_W / 2 - 2),
          fy + FLAG_H / 2 + 1
        );
        ctx.textAlign = "left";

        if (isDragged) {
          const tc = formatTimecode(sample / sr);
          const tw = ctx.measureText(tc).width;
          ctx.fillStyle = css("--panel");
          ctx.beginPath();
          ctx.roundRect(x - tw / 2 - 6, fy + FLAG_H + 6, tw + 12, 20, 4);
          ctx.fill();
          ctx.strokeStyle = css("--line");
          ctx.stroke();
          ctx.fillStyle = css("--copper-hi");
          ctx.textAlign = "center";
          ctx.fillText(tc, x, fy + FLAG_H + 16);
          ctx.textAlign = "left";
        }
      }
    }

    // Playhead (always on top).
    const px = Math.round(sampleToX(playheadSample, vp)) + 0.5;
    if (px >= 0 && px <= w) {
      ctx.save();
      ctx.shadowColor = css("--playhead-glow");
      ctx.shadowBlur = 6;
      ctx.strokeStyle = css("--playhead");
      ctx.beginPath();
      ctx.moveTo(px, RULER_H);
      ctx.lineTo(px, h);
      ctx.stroke();
      ctx.fillStyle = css("--playhead");
      ctx.beginPath();
      ctx.moveTo(px - 6, RULER_H);
      ctx.lineTo(px + 6, RULER_H);
      ctx.lineTo(px, RULER_H + 8);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
  }, []);

  // Resize observer.
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      sizeRef.current = { w: r.width, h: r.height };
      propsRef.current.onWidthChange(r.width);
      draw();
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [draw]);

  // Fetch peaks whenever the visible window, the layer mix, the per-track
  // overrides or the view mode change (the backend applies all gains).
  const mixKey =
    p.view.layers.map((l) => `${l.id}:${l.gain_db}:${l.muted}`).join(",") +
    "|" +
    p.view.tracks
      .map((t) => `${t.id}@${t.start_sample}-${t.end_sample}:${JSON.stringify(t.gain_overrides)}`)
      .join(",");
  useEffect(() => {
    const w = sizeRef.current.w || 1000;
    const start = Math.floor(p.viewport.start);
    const end = Math.ceil(p.viewport.start + w * p.viewport.spp);
    const seq = ++fetchSeq.current;
    const buckets = Math.max(Math.round(w), 100);
    if (p.waveMode === "layers") {
      api
        .getPeaksSplit(start, end, buckets)
        .then((lanes) => {
          if (seq === fetchSeq.current) {
            lanesRef.current = lanes;
            draw();
          }
        })
        .catch(() => {});
    } else {
      api
        .getPeaks(start, end, buckets)
        .then((slice) => {
          if (seq === fetchSeq.current) {
            sliceRef.current = slice;
            draw();
          }
        })
        .catch(() => {});
    }
  }, [p.viewport.start, p.viewport.spp, p.view.audio.path, p.view.audio.duration_samples, mixKey, p.waveMode, draw]);

  // Redraw on any relevant prop change.
  useEffect(() => {
    draw();
  }, [p.view, p.viewport, p.playheadSample, p.waveMode, p.proposals, p.ignoredProposals, p.selection, p.pendingStart, p.selectedTrack, draw]);

  // Auto-follow the playhead past the right edge.
  useEffect(() => {
    const w = sizeRef.current.w;
    if (!w) return;
    const span = w * p.viewport.spp;
    if (p.playheadSample > p.viewport.start + span) {
      p.onViewportChange({ start: p.playheadSample - span * 0.1, spp: p.viewport.spp });
    }
  }, [p.playheadSample]); // eslint-disable-line react-hooks/exhaustive-deps

  const edgeAt = useCallback(
    (x: number): EdgeRef | null => {
      const { viewport } = propsRef.current;
      let best: EdgeRef | null = null;
      let bestDist = Infinity;
      for (const e of edges()) {
        const ex = sampleToX(e.sample, viewport);
        const dist = Math.abs(x - ex);
        if (dist <= EDGE_HIT_PX && dist < bestDist) {
          best = e;
          bestDist = dist;
        }
      }
      return best;
    },
    [edges]
  );

  const trackAt = useCallback((x: number): number | null => {
    const { view, viewport } = propsRef.current;
    const s = xToSample(x, viewport);
    const t = view.tracks.find((t) => s >= t.start_sample && s < t.end_sample);
    return t?.id ?? null;
  }, []);

  // Wheel: zoom centered on cursor; horizontal delta pans (non-passive).
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { view, viewport, onViewportChange } = propsRef.current;
      const rect = el.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const w = sizeRef.current.w;
      const total = view.audio.duration_samples;
      if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        onViewportChange(
          clampViewport(
            { start: viewport.start + e.deltaX * viewport.spp, spp: viewport.spp },
            w,
            total,
            1
          )
        );
      } else {
        const factor = Math.exp(e.deltaY * 0.0022);
        onViewportChange(zoomAt(viewport, x, factor, w, total, 1));
      }
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const toLocal = (e: React.PointerEvent | React.MouseEvent) => {
    const rect = wrapRef.current!.getBoundingClientRect();
    return { x: e.clientX - rect.left, y: e.clientY - rect.top };
  };

  const clampSample = (s: number) =>
    Math.min(Math.max(s, 0), propsRef.current.view.audio.duration_samples);

  return (
    <div
      ref={wrapRef}
      className="waveform-wrap"
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        const { x } = toLocal(e);
        const edge = edgeAt(x);
        if (edge) {
          drag.current = { type: "edge", id: edge.id, edge: edge.edge, pos: edge.sample, moved: false };
          propsRef.current.onSelectTrack(edge.id);
        } else {
          const anchor = clampSample(xToSample(x, propsRef.current.viewport));
          drag.current = { type: "select", anchor, moved: false };
        }
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        draw();
      }}
      onPointerMove={(e) => {
        const { x } = toLocal(e);
        const d = drag.current;
        if (d?.type === "edge") {
          d.pos = clampSample(xToSample(x, propsRef.current.viewport));
          d.moved = true;
          draw();
          return;
        }
        if (d?.type === "select") {
          const cur = clampSample(xToSample(x, propsRef.current.viewport));
          if (Math.abs(cur - d.anchor) > 3 * propsRef.current.viewport.spp) {
            d.moved = true;
            propsRef.current.onSelectionChange({
              start: Math.min(d.anchor, cur),
              end: Math.max(d.anchor, cur),
            });
          }
          return;
        }
        const edge = edgeAt(x);
        const prev = hoverEdge.current;
        if (edge?.id !== prev?.id || edge?.edge !== prev?.edge) {
          hoverEdge.current = edge;
          draw();
        }
        wrapRef.current!.style.cursor = edge ? "ew-resize" : "default";
      }}
      onPointerUp={(e) => {
        const { x } = toLocal(e);
        const d = drag.current;
        drag.current = null;
        if (d?.type === "edge") {
          if (d.moved) {
            propsRef.current.onMoveEdge(d.id, d.edge, Math.round(d.pos));
          }
          draw();
          return;
        }
        if (d?.type === "select") {
          if (!d.moved) {
            // Simple click: seek, select the clicked track (if any), clear selection.
            const sample = clampSample(xToSample(x, propsRef.current.viewport));
            propsRef.current.onSelectionChange(null);
            propsRef.current.onSelectTrack(trackAt(x));
            propsRef.current.onSeek(Math.round(sample));
          }
          draw();
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        const { x } = toLocal(e);
        const edge = edgeAt(x);
        const id = edge?.id ?? trackAt(x);
        if (id != null) propsRef.current.onRemoveRegion(id);
      }}
      onPointerLeave={() => {
        if (hoverEdge.current) {
          hoverEdge.current = null;
          draw();
        }
      }}
    >
      <canvas ref={canvasRef} className="waveform-canvas" />
    </div>
  );
}
