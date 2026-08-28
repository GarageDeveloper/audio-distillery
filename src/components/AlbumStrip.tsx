import { useState } from "react";

import type { AlbumLayout } from "../types/AlbumLayout";

interface SourceTrack {
  id: number;
  start_sample: number;
  end_sample: number;
}

interface Props {
  /** Master phase: taller lane, two-line blocks (duration + ISRC). */
  tall?: boolean;
  album: AlbumLayout;
  sampleRate: number;
  albumGapMs: number;
  discBreaks: number[];
  /** Track spans on the SOURCE timeline (for prev/next in source mode). */
  sourceTracks: SourceTrack[];
  /** Playhead in the CURRENT program's samples (source or album time). */
  playheadSample: number;
  playMode: "edit" | "album";
  playing: boolean;
  /** Explicit program choice from the Source | Album toggle. */
  onSetMode: (mode: "edit" | "album") => void;
  /** Play a track (block clicks): SOURCE start sample — the app maps
   * it into the current program and never changes the mode. */
  onTrackPlay: (sourceStartSample: number) => void;
  /** Seek within the CURRENT program. */
  onSeek: (sample: number) => void;
  onTogglePlay: () => void;
  onSetTrackGap: (id: number, gapMs: number | null) => void;
  /** id → ISRC ("" = none), for the tall variant's second line. */
  isrcById?: Record<number, string>;
}

const fmt = (samples: number, sr: number) => {
  const s = Math.floor(Math.max(0, samples) / sr);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

/** THE player: one transport for both programs, with an explicit
 * Source | Album mode toggle. The phase sets the default mode; only the
 * user changes it afterwards. Display only — the programs live in the
 * backend. */
export function AlbumStrip(p: Props) {
  const [gapEdit, setGapEdit] = useState<{ id: number; draft: string } | null>(null);
  const total = Math.max(p.album.total_samples, 1);
  const albumMode = p.playMode === "album";
  const hasAlbum = p.album.tracks.length > 0;

  /** Track starts in the CURRENT program's time. */
  const starts: number[] = albumMode
    ? p.album.tracks.map((t) => t.start_sample)
    : p.sourceTracks.map((t) => t.start_sample);
  const currentIndex = (() => {
    let i = -1;
    for (let k = 0; k < starts.length; k++) {
      if (p.playheadSample >= starts[k]) i = k;
    }
    return i;
  })();

  const prev = () => {
    const cur = currentIndex >= 0 ? currentIndex : 0;
    const start = starts[cur] ?? 0;
    // Standard transport: restart the current title, then step back.
    if (p.playheadSample > start + p.sampleRate) {
      p.onSeek(start);
    } else if (cur > 0) {
      p.onSeek(starts[cur - 1]);
    } else {
      p.onSeek(0);
    }
  };
  const next = () => {
    const s = starts[currentIndex + 1];
    if (s != null) p.onSeek(s);
  };

  return (
    <div className={`album-strip ${albumMode ? "active" : ""} ${p.tall ? "tall" : ""}`}>
      <div className="album-transport">
        <span className="mode-toggle" title="Which program the player runs: the source timeline, or the album as delivered (tracks + gaps). Edit defaults to Source, Master to Album.">
          <button
            className={`mode-seg ${!albumMode ? "on" : ""}`}
            onClick={() => p.onSetMode("edit")}
          >
            Source
          </button>
          <button
            className={`mode-seg ${albumMode ? "on" : ""}`}
            disabled={!hasAlbum}
            title={hasAlbum ? undefined : "No tracks yet — the album program is empty"}
            onClick={() => p.onSetMode("album")}
          >
            Album
          </button>
        </span>
        <button
          className="btn btn-icon"
          title="Previous track"
          disabled={starts.length === 0}
          onClick={prev}
        >
          ⏮
        </button>
        <button className="btn btn-icon" title="Play/pause (Space)" onClick={p.onTogglePlay}>
          {p.playing ? "❚❚" : "▶"}
        </button>
        <button
          className="btn btn-icon"
          title="Next track"
          disabled={starts.length === 0 || currentIndex + 1 >= starts.length}
          onClick={next}
        >
          ⏭
        </button>
        {albumMode && (
          <span className="album-total">
            {fmt(p.playheadSample, p.sampleRate)} / {fmt(p.album.total_samples, p.sampleRate)}
          </span>
        )}
      </div>
      {hasAlbum && (
      <div className="album-lane">
        {p.album.tracks.map((t, i) => {
          const gapW = (t.start_sample - (i === 0 ? 0 : p.album.tracks[i - 1].start_sample + p.album.tracks[i - 1].length_samples)) / total;
          const w = t.length_samples / total;
          const isBreak = p.discBreaks.includes(t.number);
          const overridden = t.gap_before_ms !== p.albumGapMs && i > 0;
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
                className={`album-block ${albumMode && i === currentIndex ? "current" : ""}`}
                style={{ width: `${w * 100}%` }}
                title={`${t.number}. ${t.title} — ${fmt(t.length_samples, p.sampleRate)} — click to listen (current program)`}
                onClick={() => p.onTrackPlay(p.sourceTracks[i]?.start_sample ?? 0)}
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
        {albumMode && (
          <span
            className="album-playhead"
            style={{ left: `${Math.min(100, (p.playheadSample / total) * 100)}%` }}
          />
        )}
      </div>
      )}
    </div>
  );
}
