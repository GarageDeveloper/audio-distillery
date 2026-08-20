import type { ProjectView } from "../types/ProjectView";
import { formatDuration } from "../lib/format";

/** Visual themes: the three design directions plus the light Alambic variant. */
export type Theme = "alambic" | "light" | "signal" | "atelier";

const THEMES: { key: Theme; label: string }[] = [
  { key: "alambic", label: "Alambic" },
  { key: "light", label: "Alambic Light" },
  { key: "signal", label: "Signal" },
  { key: "atelier", label: "Atelier" },
];

interface Props {
  view: ProjectView | null;
  playing: boolean;
  positionSeconds: number;
  panelOpen: boolean;
  theme: Theme;
  onThemeChange: (t: Theme) => void;
  waveMode: "mix" | "layers";
  onWaveModeChange: (m: "mix" | "layers") => void;
  onOpen: () => void;
  onAddClips: () => void;
  onAddTake: () => void;
  onTogglePlay: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onUndo: () => void;
  onRedo: () => void;
  onDetectSilences: () => void;
  onExport: () => void;
  onAlbum: () => void;
  onMastering: () => void;
  onTogglePanel: () => void;
  onToggleSnap: () => void;
}

const PlayIcon = ({ playing }: { playing: boolean }) =>
  playing ? (
    <svg width="12" height="14" viewBox="0 0 12 14" fill="currentColor">
      <rect x="1" y="1" width="3.6" height="12" rx="1" />
      <rect x="7.4" y="1" width="3.6" height="12" rx="1" />
    </svg>
  ) : (
    <svg width="12" height="14" viewBox="0 0 12 14" fill="currentColor">
      <path d="M2 1.3c0-.8.9-1.3 1.6-.9l8 5.7c.6.4.6 1.4 0 1.8l-8 5.7c-.7.5-1.6 0-1.6-.9V1.3z" />
    </svg>
  );

export function Toolbar(p: Props) {
  const clipCount = p.view?.audio.clips.length ?? 0;
  const firstName = p.view?.audio.path.split(/[/\\]/).pop();
  const fileName =
    clipCount > 1 ? `${firstName} +${clipCount - 1}` : firstName;
  const meta = p.view
    ? `${clipCount > 1 ? `${clipCount} clips · ` : ""}${formatDuration(
        p.view.audio.duration_seconds
      )} · ${(p.view.audio.sample_rate / 1000).toLocaleString("en-US", {
        maximumFractionDigits: 1,
      })} kHz · ${
        p.view.audio.channels === 1 ? "mono" : p.view.audio.channels === 2 ? "stereo" : `${p.view.audio.channels} ch`
      }`
    : undefined;

  return (
    <div className="toolbar">
      <span className="wordmark">
        <svg className="drop" width="12" height="16" viewBox="0 0 12 16" fill="currentColor">
          <path d="M6 0C6 4 0 7.5 0 11a6 6 0 0 0 12 0C12 7.5 6 4 6 0z" />
        </svg>
        Still
      </span>
      <span className="sep" />
      <button className="btn" onClick={p.onOpen} title="Open an audio file or project (⌘O)">
        Open
      </button>
      {p.view && (
        <>
          <button
            className="btn"
            onClick={p.onAddClips}
            title="Append audio files to the end of the timeline (base layer)"
          >
            + Clip
          </button>
          {p.view.layers.length > 1 && (
            <button
              className="btn"
              onClick={p.onAddTake}
              title="Append a whole synced take: one file per layer, starting together after the current timeline"
            >
              + Take
            </button>
          )}
          <button className="btn" onClick={p.onSave} title="Save project (⌘S)">
            Save
          </button>
          <button
            className="btn btn-icon"
            onClick={p.onUndo}
            disabled={!p.view.can_undo}
            title="Undo (⌘Z)"
          >
            ↺
          </button>
          <button
            className="btn btn-icon"
            onClick={p.onRedo}
            disabled={!p.view.can_redo}
            title="Redo (⇧⌘Z)"
          >
            ↻
          </button>
        </>
      )}
      {fileName && (
        <div className="file-meta">
          <span className="name" title={p.view?.audio.path}>
            {fileName}
          </span>
          <span className="meta">{meta}</span>
        </div>
      )}

      <div className="transport">
        <button
          className="play-btn"
          onClick={p.onTogglePlay}
          disabled={!p.view}
          title="Play / Pause (Space)"
        >
          <PlayIcon playing={p.playing} />
        </button>
        {p.view && (
          <span className="timecode">
            {formatDuration(p.positionSeconds)}{" "}
            <span className="total">/ {formatDuration(p.view.audio.duration_seconds)}</span>
          </span>
        )}
      </div>

      <div className="toolbar-right">
        {p.view && p.view.layers.length > 1 && (
          <div className="segmented wave-mode" title="Waveform view: summed mix, or one lane per layer">
            <button
              className={p.waveMode === "mix" ? "on" : ""}
              onClick={() => p.onWaveModeChange("mix")}
            >
              Mix
            </button>
            <button
              className={p.waveMode === "layers" ? "on" : ""}
              onClick={() => p.onWaveModeChange("layers")}
            >
              Layers
            </button>
          </div>
        )}
        <select
          className="theme-select"
          value={p.theme}
          onChange={(e) => p.onThemeChange(e.target.value as Theme)}
          title="Theme"
        >
          {THEMES.map((t) => (
            <option key={t.key} value={t.key}>
              {t.label}
            </option>
          ))}
        </select>
        {p.view && (
          <>
            <button
              className={`btn ${p.view.snap_to_zero ? "active" : ""}`}
              onClick={p.onToggleSnap}
              title="Snap markers to the nearest zero crossing"
            >
              Snap 0×
            </button>
            <button
              className="btn"
              onClick={p.onDetectSilences}
              title="Suggest split points from silences"
            >
              Auto-split
            </button>
            <button
              className="btn"
              onClick={p.onAlbum}
              title="Album metadata (tags written to exported files)"
            >
              Album…
            </button>
            <button
              className={`btn ${p.view.mastering_chain.length > 0 ? "active" : ""}`}
              onClick={p.onMastering}
              title="Mastering chain (AU plugins on the master bus, live)"
            >
              Mastering…
            </button>
            <button
              className="btn btn-primary"
              onClick={p.onExport}
              disabled={p.view.tracks.length === 0}
              title={
                p.view.tracks.length === 0
                  ? "Mark at least one track region first"
                  : "Export tracks (⌘E)"
              }
            >
              Export…
            </button>
            <button
              className={`btn btn-icon ${p.panelOpen ? "active" : ""}`}
              onClick={p.onTogglePanel}
              title="Show / hide track list"
              aria-pressed={p.panelOpen}
            >
              <svg width="14" height="12" viewBox="0 0 14 12" fill="none" stroke="currentColor" strokeWidth="1.5">
                <rect x="0.75" y="0.75" width="12.5" height="10.5" rx="2" />
                <line x1="9" y1="1" x2="9" y2="11" />
              </svg>
            </button>
          </>
        )}
      </div>
    </div>
  );
}
