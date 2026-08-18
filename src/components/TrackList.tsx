import { useEffect, useRef, useState } from "react";
import type { ProjectView } from "../types/ProjectView";
import { formatDuration } from "../lib/format";

interface Props {
  view: ProjectView;
  playheadSample: number;
  selectedTrack: number | null;
  onSelectTrack: (id: number | null) => void;
  onRename: (id: number, title: string) => void;
  onRemoveRegion: (id: number) => void;
  onSeek: (sample: number) => void;
}

export function TrackList({
  view,
  playheadSample,
  selectedTrack,
  onSelectTrack,
  onRename,
  onRemoveRegion,
  onSeek,
}: Props) {
  const [editing, setEditing] = useState<number | null>(null);
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
          return (
            <div
              key={t.id}
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
              <span className="num">{String(t.number).padStart(2, "0")}</span>
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
              <span className="dur">{formatDuration(t.duration_seconds)}</span>
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
