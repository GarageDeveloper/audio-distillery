import { useCallback, useEffect, useRef, useState } from "react";
import type { ProjectView } from "../types/ProjectView";
import type { PeakSlice } from "../types/PeakSlice";
import type { RegionSpan } from "../types/RegionSpan";
import type { RegionEdge } from "../types/RegionEdge";
import { api } from "../api";
import { formatTimecode, formatDuration } from "../lib/format";
import type { Viewport } from "../lib/viewport";
import { sampleToX, xToSample, zoomAt, clampViewport, edgeScrollVelocity } from "../lib/viewport";
import { clampSpanToFreeHole, clampEdgeToNeighbors, snapToClipBoundary } from "../lib/spans";

const RULER_H = 26;
const MIN_LANE_H = 90; // comfortable minimum per expanded layer lane
const COLLAPSED_H = 22; // collapsed layer strip
const SCROLLBAR_W = 6;
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
  /// Proposals the user explicitly excluded during review: stay fully
  /// materialized but marked in the error tint.
  excludedProposals?: RegionSpan[] | null;
  selection: RegionSpan | null;
  pendingStart: number | null;
  selectedTrack: number | null;
  /** Index into view.audio.clips of the selected clip (null = none). */
  selectedClip: number | null;
  onWidthChange: (w: number) => void;
  onViewportChange: (vp: Viewport) => void;
  onSeek: (sample: number) => void;
  onSelectionChange: (sel: RegionSpan | null) => void;
  onAddRegion: (start: number, end: number) => void;
  onBeginEdgeDrag: () => void;
  onMoveEdge: (id: number, edge: RegionEdge, sample: number) => void;
  onSelectTrack: (id: number | null) => void;
  onSelectClip: (index: number | null) => void;
  /** The clip's ⋯ menu chip was clicked: open the menu anchored at
   * (x, y) — coordinates local to the waveform wrap. */
  onOpenClipMenu: (index: number, x: number, y: number) => void;
  /** Shift-click: complete a selection from the App-held anchor. */
  onShiftClick: (sample: number) => void;
  onRemoveRegion: (id: number) => void;
  onToggleLayerCollapsed: (id: number, collapsed: boolean) => void;
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
  // Vertical navigation of the Layers view.
  const scrollY = useRef(0);
  const maxScroll = useRef(0);
  const contentH = useRef(0);
  const labelRects = useRef<{ x: number; y: number; w: number; h: number; id: number; collapsed: boolean }[]>([]);
  const scrollbarDrag = useRef<{ startY: number; startScroll: number } | null>(null);
  const dragSendAt = useRef(0);
  const clipRects = useRef<{ x: number; y: number; w: number; h: number; index: number }[]>([]);
  /** Sample of the clip boundary an edge drag is currently snapped to. */
  const snappedAt = useRef<number | null>(null);
  const clipMenuRects = useRef<{ x: number; y: number; w: number; h: number; index: number }[]>([]);
  // Drag auto-scroll at the viewport edges.
  const autoScrollRaf = useRef<number | null>(null);
  const lastPointerX = useRef(0);

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
      // Layout: collapsed layers take a thin strip; expanded ones share the
      // remaining space with a comfortable minimum, scrolling vertically
      // (⌥+wheel / scrollbar) when it no longer fits.
      const flags = lanes.map((_, li) => layerViews[li]?.collapsed ?? false);
      const nCollapsed = flags.filter(Boolean).length;
      const nExpanded = flags.length - nCollapsed;
      const expandedH =
        nExpanded > 0
          ? Math.max(MIN_LANE_H, (area - nCollapsed * COLLAPSED_H) / nExpanded)
          : 0;
      contentH.current = nCollapsed * COLLAPSED_H + nExpanded * expandedH;
      maxScroll.current = Math.max(0, contentH.current - area);
      scrollY.current = Math.min(Math.max(scrollY.current, 0), maxScroll.current);

      ctx.save();
      ctx.beginPath();
      ctx.rect(0, RULER_H, w, area);
      ctx.clip();
      ctx.font = "600 10px ui-monospace, Menlo, Consolas, monospace";
      labelRects.current = [];
      let laneTop = RULER_H - scrollY.current;
      for (let li = 0; li < lanes.length; li++) {
        const collapsed = flags[li];
        const laneH = collapsed ? COLLAPSED_H : expandedH;
        const top = laneTop;
        laneTop += laneH;
        if (top + laneH < RULER_H || top > h) continue;
        if (li > 0) {
          ctx.strokeStyle = css("--line-soft");
          ctx.beginPath();
          ctx.moveTo(0, Math.round(top) + 0.5);
          ctx.lineTo(w, Math.round(top) + 0.5);
          ctx.stroke();
        }
        const muted = layerViews[li]?.muted ?? false;
        const chCount = Math.max(lanes[li].channels.length, 1);
        if (collapsed) {
          ctx.fillStyle = css("--bg-deep");
          ctx.globalAlpha = 0.5;
          ctx.fillRect(0, top, w, laneH);
          ctx.globalAlpha = 1;
          drawLane(lanes[li], "mono", top + laneH / 2, laneH * 0.34, 0, true);
        } else {
          // Each layer shows its REAL channels: stereo files get two
          // sub-lanes (L/R, usual channel colors), mono files a single one.
          const subH = laneH / chCount;
          for (let c = 0; c < chCount; c++) {
            const scy = top + subH * (c + 0.5);
            ctx.strokeStyle = centerColor;
            ctx.beginPath();
            ctx.moveTo(0, Math.round(scy) + 0.5);
            ctx.lineTo(w, Math.round(scy) + 0.5);
            ctx.stroke();
            drawLane(lanes[li], c, scy, subH * 0.42, c, muted);
          }
        }
        // Clickable lane label: accent chevron + high-contrast file name
        // + dimmer meta (layout, muted), on a solid bordered chip.
        const name = layerViews[li]?.name ?? `Layer ${li + 1}`;
        const layout = chCount === 1 ? "mono" : chCount === 2 ? "stereo" : `${chCount} ch`;
        const chevron = collapsed ? "▸" : "▾";
        const meta = ` · ${layout}${muted ? " · muted" : ""}`;
        ctx.font = "700 11px ui-monospace, Menlo, Consolas, monospace";
        const chevronW = ctx.measureText(chevron).width;
        const nameW = ctx.measureText(name).width;
        ctx.font = "600 10px ui-monospace, Menlo, Consolas, monospace";
        const metaW = ctx.measureText(meta).width;
        const chipH = 20;
        const chipW = chevronW + nameW + metaW + 22;
        const ly = collapsed ? top + laneH / 2 - chipH / 2 : top + 6;
        ctx.fillStyle = css("--panel");
        ctx.strokeStyle = css("--copper-lo");
        ctx.beginPath();
        ctx.roundRect(6.5, ly + 0.5, chipW, chipH, 5);
        ctx.fill();
        ctx.stroke();
        ctx.textBaseline = "middle";
        ctx.font = "700 11px ui-monospace, Menlo, Consolas, monospace";
        ctx.fillStyle = css("--copper-hi");
        ctx.fillText(chevron, 13, ly + chipH / 2 + 1);
        ctx.fillStyle = css("--text");
        ctx.fillText(name, 13 + chevronW + 5, ly + chipH / 2 + 1);
        ctx.font = "600 10px ui-monospace, Menlo, Consolas, monospace";
        ctx.fillStyle = css("--text-2");
        ctx.fillText(meta, 13 + chevronW + 5 + nameW, ly + chipH / 2 + 1);
        if (layerViews[li]) {
          labelRects.current.push({
            x: 6,
            y: ly,
            w: chipW + 1,
            h: chipH,
            id: layerViews[li].id,
            collapsed,
          });
        }
      }
      ctx.restore();

      // Thin scrollbar when the lanes overflow.
      if (maxScroll.current > 0) {
        const trackH = area - 4;
        const thumbH = Math.max(24, (area / contentH.current) * trackH);
        const thumbY =
          RULER_H + 2 + (scrollY.current / maxScroll.current) * (trackH - thumbH);
        ctx.fillStyle = css("--line");
        ctx.globalAlpha = 0.5;
        ctx.fillRect(w - SCROLLBAR_W - 2, RULER_H + 2, SCROLLBAR_W, trackH);
        ctx.globalAlpha = 1;
        ctx.fillStyle = css("--copper-lo");
        ctx.beginPath();
        ctx.roundRect(w - SCROLLBAR_W - 2, thumbY, SCROLLBAR_W, thumbH, 3);
        ctx.fill();
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
    clipRects.current = [];
    if (view.audio.clips.length > 1) {
      const clipColor = css("--clip");
      const { selectedClip } = propsRef.current;
      ctx.save();
      view.audio.clips.forEach((clip, ci) => {
        const x0 = sampleToX(clip.start_sample, vp);
        const x1 = sampleToX(clip.start_sample + clip.duration_samples, vp);
        if (x1 < 0 || x0 > w) return;
        const selected = ci === selectedClip;
        if (selected) {
          ctx.fillStyle = clipColor;
          ctx.globalAlpha = 0.08;
          ctx.fillRect(x0 + 1, RULER_H + 1, x1 - x0 - 2, h - RULER_H - 2);
        }
        ctx.strokeStyle = clipColor;
        ctx.lineWidth = selected ? 3 : 2;
        ctx.globalAlpha = selected ? 1 : 0.9;
        ctx.strokeRect(x0 + 1, RULER_H + 1, x1 - x0 - 2, h - RULER_H - 2);
        ctx.globalAlpha = 1;
      });
      ctx.font = "700 11px ui-monospace, Menlo, Consolas, monospace";
      ctx.textBaseline = "middle";
      view.audio.clips.forEach((clip, ci) => {
        const x0 = sampleToX(clip.start_sample, vp);
        const x1 = sampleToX(clip.start_sample + clip.duration_samples, vp);
        if (x1 < 0 || x0 > w) return;
        const label = clip.name;
        const tw = ctx.measureText(label).width;
        const bx = Math.max(x0, 0) + 5;
        const bh = 20;
        const by = h - bh - 5;
        const bw = Math.min(tw + 16, Math.min(x1, w) - bx - 4);
        if (bw < 30) return;
        const selected = ci === propsRef.current.selectedClip;
        ctx.fillStyle = selected ? css("--copper-hi") : clipColor;
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
        clipRects.current.push({ x: bx, y: by, w: bw, h: bh, index: ci });
      });
      // ⋯ menu chip at each clip's top-right corner.
      clipMenuRects.current = [];
      view.audio.clips.forEach((clip, ci) => {
        const x0 = sampleToX(clip.start_sample, vp);
        const x1 = sampleToX(clip.start_sample + clip.duration_samples, vp);
        if (x1 < 0 || x0 > w) return;
        if (Math.min(x1, w) - Math.max(x0, 0) < 60) return;
        const cw = 24;
        const chh = 17;
        const cx = Math.min(x1, w) - cw - 6;
        const cy = RULER_H + 6;
        const selected = ci === propsRef.current.selectedClip;
        ctx.fillStyle = selected ? css("--copper-hi") : clipColor;
        ctx.globalAlpha = selected ? 1 : 0.85;
        ctx.beginPath();
        ctx.roundRect(cx, cy, cw, chh, 5);
        ctx.fill();
        ctx.globalAlpha = 1;
        ctx.fillStyle = css("--wave-bg");
        ctx.font = "700 12px ui-monospace, Menlo, Consolas, monospace";
        ctx.fillText("⋯", cx + 6, cy + chh / 2 + 1);
        clipMenuRects.current.push({ x: cx, y: cy, w: cw, h: chh, index: ci });
      });
      ctx.restore();
    } else {
      clipMenuRects.current = [];
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
    // Explicitly excluded proposals: still fully drawn, but in the error
    // tint with a corner cross — reviewable and re-includable at a glance.
    const { excludedProposals } = propsRef.current;
    if (excludedProposals && excludedProposals.length > 0) {
      ctx.save();
      ctx.setLineDash([5, 4]);
      for (const r of excludedProposals) {
        const x0 = sampleToX(r.start, vp);
        const x1 = sampleToX(r.end, vp);
        if (x1 < 0 || x0 > w) continue;
        const left = Math.max(x0, 0);
        const width = Math.min(x1, w) - left;
        ctx.globalAlpha = 0.10;
        ctx.fillStyle = css("--err");
        ctx.fillRect(left, RULER_H, width, area);
        ctx.globalAlpha = 0.55;
        ctx.strokeStyle = css("--err");
        ctx.strokeRect(left + 0.5, RULER_H + 1.5, width - 1, area - 3);
        // Small ✕ chip at the top of the span.
        ctx.globalAlpha = 0.9;
        ctx.setLineDash([]);
        ctx.font = "700 10px ui-monospace, Menlo, monospace";
        ctx.fillStyle = css("--err");
        ctx.textAlign = "center";
        ctx.fillText("✕", left + width / 2, RULER_H + 16);
        ctx.setLineDash([5, 4]);
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

    // Clip-boundary snap feedback: while an edge drag is magnetized to a
    // file frontier, the whole boundary lights up.
    if (snappedAt.current != null && drag.current?.type === "edge") {
      const sx = Math.round(sampleToX(snappedAt.current, vp)) + 0.5;
      if (sx >= 0 && sx <= w) {
        ctx.save();
        ctx.strokeStyle = css("--copper-hi");
        ctx.lineWidth = 2;
        ctx.setLineDash([5, 4]);
        ctx.beginPath();
        ctx.moveTo(sx, RULER_H);
        ctx.lineTo(sx, h);
        ctx.stroke();
        ctx.restore();
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

  // Resize observer. Bump sizeTick so the peaks fetch runs again for the
  // new width (e.g. when the side panel collapses and the canvas widens).
  const [sizeTick, setSizeTick] = useState(0);
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      const r = el.getBoundingClientRect();
      const changed = Math.abs(sizeRef.current.w - r.width) >= 1;
      sizeRef.current = { w: r.width, h: r.height };
      propsRef.current.onWidthChange(r.width);
      draw();
      if (changed) setSizeTick((t) => t + 1);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [draw]);

  // Fetch peaks whenever the visible window, the layer mix, the per-track
  // overrides or the view mode change (the backend applies all gains).
  const mixKey =
    p.view.layers
      .map((l) => `${l.id}:${l.gain_db}:${l.muted}:${l.solo}`)
      .join(",") +
    "|" +
    p.view.tracks
      .map(
        (t) =>
          `${t.id}@${t.start_sample}-${t.end_sample}:${JSON.stringify(t.gain_overrides)}${JSON.stringify(t.mute_overrides)}${JSON.stringify(t.solo_overrides)}`
      )
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
  }, [p.viewport.start, p.viewport.spp, p.view.audio.path, p.view.audio.duration_samples, mixKey, p.waveMode, sizeTick, draw]);

  // Redraw on any relevant prop change.
  useEffect(() => {
    draw();
  }, [p.view, p.viewport, p.playheadSample, p.waveMode, p.proposals, p.ignoredProposals, p.excludedProposals, p.selection, p.pendingStart, p.selectedTrack, draw]);

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
      const { viewport, selectedTrack } = propsRef.current;
      let best: EdgeRef | null = null;
      let bestDist = Infinity;
      for (const e of edges()) {
        const ex = sampleToX(e.sample, viewport);
        const dist = Math.abs(x - ex);
        if (dist > EDGE_HIT_PX) continue;
        // When two edges are equally reachable (adjacent tracks), prefer the
        // selected track's edge so you grab the one you're working on.
        const bias = selectedTrack != null && e.id === selectedTrack ? -4 : 0;
        if (dist + bias < bestDist) {
          best = e;
          bestDist = dist + bias;
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

  const trackSpans = useCallback(() => {
    return propsRef.current.view.tracks.map((t) => ({
      id: t.id,
      start: t.start_sample,
      end: t.end_sample,
    }));
  }, []);

  /** Resolve an edge-drag position: magnetic snap to clip boundaries,
   * then hard clamp against the neighbouring tracks (butée). */
  const resolveEdgePos = useCallback(
    (id: number, edge: RegionEdge, raw: number): number => {
      const { view, viewport } = propsRef.current;
      const bounds: number[] = [];
      if (view.audio.clips.length > 1) {
        for (const c of view.audio.clips) {
          bounds.push(c.start_sample, c.start_sample + c.duration_samples);
        }
      }
      const tol = 8 * viewport.spp;
      const snap = snapToClipBoundary(raw, bounds, tol);
      snappedAt.current = snap.snapped ? snap.pos : null;
      const minLen = Math.round(view.audio.sample_rate * 0.2);
      return clampEdgeToNeighbors(id, edge, snap.pos, trackSpans(), minLen);
    },
    [trackSpans]
  );

  /** Index of the clip under x (only when clip frames are drawn). */
  const clipAt = useCallback((x: number): number | null => {
    const { view, viewport } = propsRef.current;
    if (view.audio.clips.length <= 1) return null;
    const s = xToSample(x, viewport);
    const i = view.audio.clips.findIndex(
      (c) => s >= c.start_sample && s < c.start_sample + c.duration_samples
    );
    return i >= 0 ? i : null;
  }, []);

  /** Auto-scroll while a drag hugs a viewport edge: pans the viewport
   * (App clamps) and keeps the dragged thing following the pointer. */
  const stopAutoScroll = useCallback(() => {
    if (autoScrollRaf.current != null) {
      cancelAnimationFrame(autoScrollRaf.current);
      autoScrollRaf.current = null;
    }
  }, []);
  const autoScrollTick = useCallback(() => {
    autoScrollRaf.current = null;
    const d = drag.current;
    if (!d) return;
    const { viewport, onViewportChange, onSelectionChange, onMoveEdge } = propsRef.current;
    const v = edgeScrollVelocity(lastPointerX.current, sizeRef.current.w, 40, 18);
    if (v === 0) return;
    onViewportChange({ start: viewport.start + v * viewport.spp, spp: viewport.spp });
    const sample = Math.max(0, xToSample(lastPointerX.current, propsRef.current.viewport));
    if (d.type === "select") {
      d.moved = true;
      onSelectionChange(clampSpanToFreeHole(d.anchor, sample, trackSpans()));
    } else if (d.type === "edge") {
      d.pos = resolveEdgePos(d.id, d.edge, sample);
      d.moved = true;
      const now = performance.now();
      if (now - dragSendAt.current >= 90) {
        dragSendAt.current = now;
        onMoveEdge(d.id, d.edge, Math.round(d.pos));
      }
    }
    draw();
    autoScrollRaf.current = requestAnimationFrame(autoScrollTick);
  }, [draw, trackSpans, resolveEdgePos]);
  useEffect(() => stopAutoScroll, [stopAutoScroll]);

  // Wheel: zoom centered on cursor; horizontal delta pans (non-passive).
  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      // ⌥ + wheel = vertical navigation of the Layers view; plain wheel and
      // pinch keep their usual time-zoom behavior.
      if (
        e.altKey &&
        propsRef.current.waveMode === "layers" &&
        maxScroll.current > 0
      ) {
        scrollY.current = Math.min(
          Math.max(scrollY.current + e.deltaY, 0),
          maxScroll.current
        );
        draw();
        return;
      }
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
        const { x, y } = toLocal(e);
        if (propsRef.current.waveMode === "layers") {
          // Lane label chevron: collapse/expand this layer.
          const hit = labelRects.current.find(
            (r) => x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
          );
          if (hit) {
            propsRef.current.onToggleLayerCollapsed(hit.id, !hit.collapsed);
            return;
          }
          // Thin scrollbar drag.
          if (maxScroll.current > 0 && x >= sizeRef.current.w - SCROLLBAR_W - 6) {
            scrollbarDrag.current = { startY: y, startScroll: scrollY.current };
            (e.target as HTMLElement).setPointerCapture(e.pointerId);
            return;
          }
        }
        // Clip ⋯ menu chip: select the clip and open its menu.
        const menuChip = clipMenuRects.current.find(
          (r) => x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
        );
        if (menuChip) {
          propsRef.current.onSelectClip(menuChip.index);
          propsRef.current.onOpenClipMenu(menuChip.index, menuChip.x, menuChip.y + menuChip.h + 4);
          draw();
          return;
        }
        // Clip name badge: explicit clip selection (no drag started).
        const badge = clipRects.current.find(
          (r) => x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
        );
        if (badge) {
          propsRef.current.onSelectClip(badge.index);
          propsRef.current.onSelectTrack(null);
          propsRef.current.onSeek(Math.round(clampSample(xToSample(x, propsRef.current.viewport))));
          draw();
          return;
        }
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
        const { x, y } = toLocal(e);
        if (scrollbarDrag.current) {
          const area = sizeRef.current.h - RULER_H;
          const ratio = contentH.current / Math.max(area, 1);
          scrollY.current = Math.min(
            Math.max(
              scrollbarDrag.current.startScroll +
                (y - scrollbarDrag.current.startY) * ratio,
              0
            ),
            maxScroll.current
          );
          draw();
          return;
        }
        const d = drag.current;
        if (d) {
          lastPointerX.current = x;
          const v = edgeScrollVelocity(x, sizeRef.current.w, 40, 18);
          if (v !== 0 && autoScrollRaf.current == null) {
            autoScrollRaf.current = requestAnimationFrame(autoScrollTick);
          } else if (v === 0) {
            stopAutoScroll();
          }
        }
        if (d?.type === "edge") {
          const wasMoved = d.moved;
          d.pos = resolveEdgePos(
            d.id,
            d.edge,
            clampSample(xToSample(x, propsRef.current.viewport))
          );
          d.moved = true;
          // One undo snapshot at the first real move, then throttled LIVE
          // previews so the track panel (durations) follows the drag.
          if (!wasMoved) {
            propsRef.current.onBeginEdgeDrag();
          }
          const now = performance.now();
          if (now - dragSendAt.current >= 90) {
            dragSendAt.current = now;
            propsRef.current.onMoveEdge(d.id, d.edge, Math.round(d.pos));
          }
          draw();
          return;
        }
        if (d?.type === "select") {
          const cur = clampSample(xToSample(x, propsRef.current.viewport));
          if (Math.abs(cur - d.anchor) > 3 * propsRef.current.viewport.spp) {
            d.moved = true;
            propsRef.current.onSelectionChange(
              clampSpanToFreeHole(d.anchor, cur, trackSpans())
            );
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
        stopAutoScroll();
        if (scrollbarDrag.current) {
          scrollbarDrag.current = null;
          return;
        }
        const { x } = toLocal(e);
        const d = drag.current;
        drag.current = null;
        if (d?.type === "edge") {
          if (d.moved) {
            propsRef.current.onMoveEdge(d.id, d.edge, Math.round(d.pos));
          }
          snappedAt.current = null;
          draw();
          return;
        }
        if (d?.type === "select") {
          if (!d.moved) {
            const sample = clampSample(xToSample(x, propsRef.current.viewport));
            if (e.shiftKey) {
              // Shift-click: complete a selection from the App-held
              // anchor (pending start / playhead / selection edge).
              propsRef.current.onShiftClick(Math.round(sample));
            } else {
              // Simple click: seek, select track and clip under the
              // cursor, clear selection.
              propsRef.current.onSelectionChange(null);
              propsRef.current.onSelectTrack(trackAt(x));
              propsRef.current.onSelectClip(clipAt(x));
              propsRef.current.onSeek(Math.round(sample));
            }
          }
          draw();
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        const { x, y } = toLocal(e);
        const badge = clipRects.current.find(
          (r) => x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h
        );
        if (badge) {
          propsRef.current.onSelectClip(badge.index);
          propsRef.current.onOpenClipMenu(badge.index, x, y);
          draw();
          return;
        }
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
