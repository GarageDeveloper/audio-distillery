import { useEffect, useRef, useState } from "react";

import type { ProjectView } from "../types/ProjectView";
import { formatDuration, formatTimecode } from "../lib/format";
import { LayersPanel } from "./LayersPanel";
import { GainInput } from "./GainInput";

/** (track-in-disc, disc) for a global 1-based track number. */
function discPos(breaks: number[], number: number): { n: number; disc: number } {
  const starts = [1, ...breaks.filter((b) => b >= 2).sort((a, b) => a - b)];
  let disc = 0;
  for (let i = 0; i < starts.length; i++) if (starts[i] <= number) disc = i;
  return { n: number - starts[disc] + 1, disc: disc + 1 };
}

interface Props {
  view: ProjectView;
  playheadSample: number;
  selectedTrack: number | null;
  onSelectTrack: (id: number | null) => void;
  onRename: (id: number, title: string) => void;
  onRemoveRegion: (id: number) => void;
  onSeek: (sample: number) => void;
  onLayerGain: (id: number, gainDb: number) => void;
  onLayerMute: (id: number, muted: boolean) => void;
  onLayerSolo: (id: number, solo: boolean) => void;
  onLayerCollapse: (id: number, collapsed: boolean) => void;
  onLayerRemove: (id: number) => void;
  onAddLayers: () => void;
  onTrackLayerGain: (trackId: number, layerId: number, gainDb: number | null) => void;
  onTrackLayerMute: (trackId: number, layerId: number, muted: boolean | null) => void;
  onTrackLayerSolo: (trackId: number, layerId: number, solo: boolean | null) => void;
  onDiscBreaksChange: (breaks: number[]) => void;
}

export function TrackList({
  view,
  playheadSample,
  selectedTrack,
  onSelectTrack,
  onRename,
  onRemoveRegion,
  onSeek,
  onLayerGain,
  onLayerMute,
  onLayerSolo,
  onLayerCollapse,
  onLayerRemove,
  onAddLayers,
  onTrackLayerGain,
  onTrackLayerMute,
  onTrackLayerSolo,
  onDiscBreaksChange,
}: Props) {
  const [editing, setEditing] = useState<number | null>(null);
  const [mixOpen, setMixOpen] = useState<number | null>(null);
  const overrideThrottle = useRef<Record<string, number>>({});

  const sendOverride = (trackId: number, layerId: number, value: number, force: boolean) => {
    const key = `${trackId}:${layerId}`;
    const now = performance.now();
    if (force || now - (overrideThrottle.current[key] ?? 0) >= 80) {
      overrideThrottle.current[key] = now;
      onTrackLayerGain(trackId, layerId, value);
    }
  };
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing != null) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const commit = (id: number, nextId?: number) => {
    onRename(id, draft);
    if (nextId !== undefined) {
      const next = view.tracks.find((t) => t.id === nextId);
      if (next) {
        setDraft(next.title);
        setEditing(nextId);
        return;
      }
    }
    setEditing(null);
  };

  const totalSelected = view.tracks.reduce((acc, t) => acc + t.duration_seconds, 0);

  return (
    <aside className="track-panel">
      {view.layers.length > 1 && (
        <LayersPanel
          view={view}
          onGain={onLayerGain}
          onMute={onLayerMute}
          onSolo={onLayerSolo}
          onCollapse={onLayerCollapse}
          onRemove={onLayerRemove}
          onAdd={onAddLayers}
        />
      )}
      <div className="track-panel-head">
        <span className="label">Tracks</span>
        <span className="meta">
          {view.tracks.length} · {formatDuration(totalSelected)}
        </span>
      </div>
      <div className="track-rows">
        {view.tracks.length === 0 && (
          <div className="track-empty">
            No tracks yet. Drag on the waveform to select a region, or press{" "}
            <kbd>M</kbd> at the start and end of a song.
          </div>
        )}
        {view.tracks.map((t, i) => {
          const isPlaying =
            playheadSample >= t.start_sample && playheadSample < t.end_sample;
          const breaks = view.album_meta.disc_breaks
            .filter((b) => b >= 2 && b <= view.tracks.length)
            .sort((a, b) => a - b);
          const multiDisc = breaks.length > 0;
          const pos = discPos(breaks, t.number);
          const breakIdx = breaks.indexOf(t.number);
          const canMoveUp = breakIdx >= 0 && !breaks.includes(t.number - 1) && t.number > 2;
          const canMoveDown =
            breakIdx >= 0 && !breaks.includes(t.number + 1) && t.number < view.tracks.length;
          const moveBreak = (delta: number) => {
            const next = breaks.filter((b) => b !== t.number);
            next.push(t.number + delta);
            onDiscBreaksChange(next);
          };
          return (
            <div key={t.id} className="track-item">
            {breakIdx >= 0 && (
              <div className="disc-sep">
                <span className="disc-sep-label">Disc {pos.disc}</span>
                <span className="disc-sep-line" />
                <button
                  className="disc-sep-btn"
                  disabled={!canMoveUp}
                  title="Start this disc one track earlier"
                  onClick={() => moveBreak(-1)}
                >
                  ↑
                </button>
                <button
                  className="disc-sep-btn"
                  disabled={!canMoveDown}
                  title="Start this disc one track later"
                  onClick={() => moveBreak(1)}
                >
                  ↓
                </button>
                <button
                  className="disc-sep-btn del"
                  title="Remove this disc break (merge with the previous disc)"
                  onClick={() => onDiscBreaksChange(breaks.filter((b) => b !== t.number))}
                >
                  ✕
                </button>
              </div>
            )}
            {i > 0 && breakIdx < 0 && (
              <button
                className="disc-gap"
                title={`Start a new disc at track ${t.number}`}
                onClick={() => onDiscBreaksChange([...breaks, t.number])}
              >
                + Disc break
              </button>
            )}
            <div
              className={`track-row ${isPlaying ? "playing" : ""} ${
                selectedTrack === t.id ? "selected" : ""
              }`}
              onClick={() => {
                onSelectTrack(t.id);
                onSeek(t.start_sample);
              }}
              onDoubleClick={(e) => {
                e.stopPropagation();
                setDraft(t.title);
                setEditing(t.id);
              }}
            >
              <span
                className="num"
                title={multiDisc ? `Album track ${t.number} — disc ${pos.disc}` : undefined}
              >
                {String(multiDisc ? pos.n : t.number).padStart(2, "0")}
              </span>
              {editing === t.id ? (
                <input
                  ref={inputRef}
                  className="rename-input"
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onClick={(e) => e.stopPropagation()}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commit(t.id);
                    else if (e.key === "Escape") setEditing(null);
                    else if (e.key === "Tab") {
                      e.preventDefault();
                      commit(t.id, view.tracks[i + 1]?.id);
                    }
                  }}
                  onBlur={() => commit(t.id)}
                />
              ) : (
                <span className="title" title={t.title}>
                  {t.title}
                </span>
              )}
              <span
                className="dur"
                title={`${formatTimecode(t.start_sample / view.audio.sample_rate)} → ${formatTimecode(t.end_sample / view.audio.sample_rate)}`}
              >
                {formatDuration(t.duration_seconds)}
              </span>
              {view.layers.length > 1 && (
                <button
                  className={`mix-toggle ${Object.keys(t.gain_overrides).length > 0 ? "has-override" : ""} ${
                    mixOpen === t.id ? "open" : ""
                  }`}
                  title="Per-track layer levels (override the session faders for this track)"
                  onClick={(e) => {
                    e.stopPropagation();
                    setMixOpen(mixOpen === t.id ? null : t.id);
                  }}
                >
                  mix
                </button>
              )}
              <button
                className="del"
                title="Remove this track (its audio is then ignored)"
                onClick={(e) => {
                  e.stopPropagation();
                  onRemoveRegion(t.id);
                }}
              >
                ✕
              </button>
            </div>
            {mixOpen === t.id && view.layers.length > 1 && (
              <div className="track-mix" onClick={(e) => e.stopPropagation()}>
                {view.layers.map((l) => {
                  const key = String(l.id);
                  const override = t.gain_overrides[key];
                  const hasOverride = override !== undefined;
                  const muteOv = t.mute_overrides[key];
                  const soloOv = t.solo_overrides[key];
                  const anyOverride =
                    hasOverride || muteOv !== undefined || soloOv !== undefined;
                  const shown = hasOverride ? override : l.gain_db;
                  return (
                    <div key={l.id} className={`track-mix-row ${anyOverride ? "overridden" : "inheriting"}`}>
                      <div className="track-mix-top">
                        <span className="layer-name" title={l.name}>
                          {l.name}
                        </span>
                        <span className="mix-state">
                          {anyOverride ? "override" : "session"}
                        </span>
                        <button
                          className={`layer-mute ${(muteOv ?? l.muted) ? "on" : ""} ${muteOv !== undefined ? "ov" : ""}`}
                          title={
                            muteOv === undefined
                              ? "Mute this layer for this track only (currently following the session)"
                              : "Following this track's own mute — click to go back to the session"
                          }
                          onClick={() =>
                            onTrackLayerMute(t.id, l.id, muteOv === undefined ? !l.muted : null)
                          }
                        >
                          M
                        </button>
                        <button
                          className={`layer-solo ${(soloOv ?? l.solo) ? "on" : ""} ${soloOv !== undefined ? "ov" : ""}`}
                          title={
                            soloOv === undefined
                              ? "Solo this layer for this track only (currently following the session)"
                              : "Following this track's own solo — click to go back to the session"
                          }
                          onClick={() =>
                            onTrackLayerSolo(t.id, l.id, soloOv === undefined ? !l.solo : null)
                          }
                        >
                          S
                        </button>
                        <GainInput
                          value={hasOverride ? override : null}
                          placeholder={`${l.gain_db > 0 ? "+" : ""}${Number(l.gain_db.toFixed(1))}`}
                          clearable
                          onCommit={(v) => onTrackLayerGain(t.id, l.id, v)}
                          title="Override this layer's gain for this track only — empty = inherit the session fader"
                        />
                        <span className="layer-db-unit">dB</span>
                        <button
                          className={`del ${hasOverride ? "" : "hidden-btn"}`}
                          title="Back to the session fader (clear the override)"
                          onClick={() => onTrackLayerGain(t.id, l.id, null)}
                        >
                          ✕
                        </button>
                      </div>
                      <input
                        type="range"
                        className="mix-fader"
                        min={-60}
                        max={12}
                        step={0.5}
                        value={shown}
                        title={
                          hasOverride
                            ? "This track's own level for this layer — double-click to go back to the session fader"
                            : "Following the session fader — drag to give this track its own level"
                        }
                        onChange={(e) => sendOverride(t.id, l.id, Number(e.target.value), false)}
                        onPointerUp={(e) =>
                          sendOverride(t.id, l.id, Number((e.target as HTMLInputElement).value), true)
                        }
                        onDoubleClick={() => onTrackLayerGain(t.id, l.id, null)}
                      />
                    </div>
                  );
                })}
                <div className="hint">
                  Grey fader = follows the session mix · drag/type to set this track's own
                  level · M/S set a per-track mute/solo (dot = own value, click again to
                  follow the session) · heard live and applied at export
                </div>
              </div>
            )}
            </div>
          );
        })}
      </div>
      <div className="track-panel-foot">
        <span>Double-click to rename</span>
        <span>
          {view.tracks.length} track{view.tracks.length !== 1 ? "s" : ""}
        </span>
      </div>
    </aside>
  );
}
