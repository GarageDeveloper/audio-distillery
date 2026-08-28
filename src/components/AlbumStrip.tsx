import { useState } from "react";

import type { AlbumLayout } from "../types/AlbumLayout";

interface Props {
  /** Master phase: taller lane, two-line blocks (duration + ISRC). */
  tall?: boolean;
  album: AlbumLayout;
  sampleRate: number;
  albumGapMs: number;
  discBreaks: number[];
  /** Playhead in ALBUM samples — only meaningful in album mode. */
  playheadSample: number;
  playMode: "edit" | "album";
  playing: boolean;
  onEnterAlbum: (seekSample: number | null) => void;
  onExitAlbum: () => void;
  onSeek: (albumSample: number) => void;
  onTogglePlay: () => void;
  onSetTrackGap: (id: number, gapMs: number | null) => void;
  /** id → ISRC ("" = none), for the tall variant's second line. */
  isrcById?: Record<number, string>;
}

const fmt = (samples: number, sr: number) => {
  const s = Math.floor(samples / sr);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

/** The TARGET timeline: the album as delivered — titles + gaps — with
 * its own transport. Display only; the layout comes computed from the
 * backend and every edit refreshes it. */
export function AlbumStrip(p: Props) {
  const [gapEdit, setGapEdit] = useState<{ id: number; draft: string } | null>(null);
  const total = Math.max(p.album.total_samples, 1);
  const active = p.playMode === "album";

  const currentIndex = p.album.tracks.findIndex(
    (t, i) =>
      p.playheadSample >= t.start_sample &&
      (i + 1 >= p.album.tracks.length ||
        p.playheadSample < p.album.tracks[i + 1].start_sample)
  );

  const prev = () => {
    const cur = currentIndex >= 0 ? currentIndex : 0;
    const t = p.album.tracks[cur];
    // Standard transport: restart the current title, then step back.
    if (t && p.playheadSample > t.start_sample + p.sampleRate) {
      p.onSeek(t.start_sample);
    } else if (cur > 0) {
      p.onSeek(p.album.tracks[cur - 1].start_sample);
    } else {
      p.onSeek(0);
    }
  };
  const next = () => {
    const cur = currentIndex >= 0 ? currentIndex : -1;
    const t = p.album.tracks[cur + 1];
    if (t) p.onSeek(t.start_sample);
  };

  return (
    <div className={`album-strip ${active ? "active" : ""} ${p.tall ? "tall" : ""}`}>
      <div className="album-transport">
        <button
          className={`album-mode-chip ${active ? "on" : ""}`}
          title={
            active
              ? "Listening to the ALBUM program (tracks + gaps). Click to go back to the source timeline."
              : "Listen to the album as it will be delivered: tracks in order with the gaps applied."
          }
          onClick={() => (active ? p.onExitAlbum() : p.onEnterAlbum(null))}
        >
          Album
        </button>
        <button className="btn btn-icon" title="Previous track" onClick={() => (active ? prev() : p.onEnterAlbum(0))}>
          ⏮
        </button>
        <button
          className="btn btn-icon"
          title="Play/pause the album program"
          onClick={() => (active ? p.onTogglePlay() : p.onEnterAlbum(null))}
        >
          {active && p.playing ? "❚❚" : "▶"}
        </button>
        <button className="btn btn-icon" title="Next track" onClick={() => (active ? next() : p.onEnterAlbum(0))}>
          ⏭
        </button>
        <span className="album-total">{fmt(p.album.total_samples, p.sampleRate)}</span>
      </div>
      <div className="album-lane">
        {p.album.tracks.map((t, i) => {
          const gapW = (t.start_sample - (i === 0 ? 0 : p.album.tracks[i - 1].start_sample + p.album.tracks[i - 1].length_samples)) / total;
          const w = t.length_samples / total;
          const isBreak = p.discBreaks.includes(t.number);
          const overridden =
            t.gap_before_ms !== p.albumGapMs && i > 0;
          return (
            <span key={t.id} className="album-cell">
              {i > 0 && (
                <button
                  className={`album-gap ${overridden ? "overridden" : ""} ${isBreak ? "disc-break" : ""}`}
                  style={{ width: `${Math.max(gapW * 100, 0.35)}%` }}
                  title={`Gap before "${t.title}": ${(t.gap_before_ms / 1000).toFixed(1)} s${overridden ? " (override)" : " (album default)"} — click to edit`}
                  onClick={() =>
                    setGapEdit({ id: t.id, draft: (t.gap_before_ms / 1000).toFixed(1) })
                  }
                />
              )}
              <button
                className={`album-block ${active && i === currentIndex ? "current" : ""}`}
                style={{ width: `${w * 100}%` }}
                title={`${t.number}. ${t.title} — ${fmt(t.length_samples, p.sampleRate)}`}
                onClick={() => p.onEnterAlbum(t.start_sample)}
              >
                <span className="album-block-label">
                  {t.number}. {t.title}
                </span>
                {p.tall && (
                  <span className="album-block-meta">
                    {fmt(t.length_samples, p.sampleRate)}
                    {" · "}
                    {p.isrcById?.[t.id] || "—"}
                  </span>
                )}
              </button>
              {gapEdit?.id === t.id && (
                <span className="album-gap-editor" onClick={(e) => e.stopPropagation()}>
                  <input
                    className="text-input num-input"
                    type="number"
                    min={0}
                    step={0.1}
                    autoFocus
                    value={gapEdit.draft}
                    onChange={(e) => setGapEdit({ id: t.id, draft: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        p.onSetTrackGap(t.id, Math.round(Math.max(0, parseFloat(gapEdit.draft) || 0) * 1000));
                        setGapEdit(null);
                      } else if (e.key === "Escape") setGapEdit(null);
                    }}
                  />
                  <button
                    className="btn"
                    title="No gap — this title segues into the previous one"
                    onClick={() => {
                      p.onSetTrackGap(t.id, 0);
                      setGapEdit(null);
                    }}
                  >
                    Segue
                  </button>
                  <button
                    className="btn"
                    title="Follow the album default again"
                    onClick={() => {
                      p.onSetTrackGap(t.id, null);
                      setGapEdit(null);
                    }}
                  >
                    × default
                  </button>
                  <button className="btn" onClick={() => setGapEdit(null)}>
                    ✕
                  </button>
                </span>
              )}
            </span>
          );
        })}
        {active && (
          <span
            className="album-playhead"
            style={{ left: `${Math.min(100, (p.playheadSample / total) * 100)}%` }}
          />
        )}
      </div>
    </div>
  );
}
