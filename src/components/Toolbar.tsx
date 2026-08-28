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
  onAbout: () => void;
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
  /** Elapsed seconds of a rolling take (null = none). */
  recSeconds: number | null;
  phase: "record" | "edit" | "master";
  masterPulse: boolean;
  onPhaseChange: (phase: "record" | "edit" | "master") => void;
  readiness: { key: string; label: string; ok: boolean; action?: "export" | "album" }[];
  onReadinessAction: (action: "export" | "album") => void;
  onTogglePlay: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onUndo: () => void;
  onRedo: () => void;
  onDetectSilences: () => void;
  onExport: () => void;
  onAlbum: () => void;
  onTogglePanel: () => void;
  onToggleSnap: () => void;
}

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
      <button
        className="wordmark"
        title="About AudioDistillery (version, licenses)"
        onClick={p.onAbout}
      >
        <svg className="drop" width="12" height="16" viewBox="0 0 12 16" fill="currentColor">
          <path d="M6 0C6 4 0 7.5 0 11a6 6 0 0 0 12 0C12 7.5 6 4 6 0z" />
        </svg>
        Still
      </button>
      <span className="sep" />
      <button className="btn" onClick={p.onOpen} title="Open an audio file or project (⌘O)">
        Open
      </button>
      <div className="phase-switch" title="Workflow phase — emphasis only, nothing is ever locked away (keys 1/2/3)">
        {(["record", "edit", "master"] as const).map((ph) => (
          <button
            key={ph}
            className={`phase-seg ${p.phase === ph ? "on" : ""} ${
              ph === "master" && p.masterPulse ? "pulse" : ""
            }`}
            disabled={!p.view && ph !== "record"}
            title={
              !p.view && ph !== "record"
                ? "Record a take or add audio files first — there is nothing to edit yet"
                : undefined
            }
            onClick={() => p.onPhaseChange(ph)}
          >
            {ph === "record" ? "Record" : ph === "edit" ? "Edit" : "Master"}
          </button>
        ))}
      </div>
      {p.recSeconds != null && p.phase !== "record" && (
        <button
          className="rec-chip"
          title="A take is rolling — click to return to the Record surface"
          onClick={() => p.onPhaseChange("record")}
        >
          <span className="record-dot" />
          {String(Math.floor(p.recSeconds / 60)).padStart(2, "0")}:
          {String(Math.floor(p.recSeconds % 60)).padStart(2, "0")}
        </button>
      )}
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
            <span className="readiness-wrap">
              <button
                className={`btn btn-primary ${p.phase === "master" ? "export-cta" : ""}`}
                onClick={p.onExport}
                disabled={p.view.tracks.length === 0}
                title={
                  p.view.tracks.length === 0
                    ? "Mark at least one track region first"
                    : "Export tracks, stems, CD image or DDP (⌘E)"
                }
              >
                Export…
                {p.phase === "master" && p.readiness.some((r) => !r.ok) && (
                  <span className="readiness-badge">
                    ○ {p.readiness.filter((r) => !r.ok).length}
                  </span>
                )}
              </button>
              {p.phase === "master" && p.readiness.length > 0 && (
                <span className="readiness-pop">
                  <b>Album readiness</b>
                  {p.readiness.map((r) =>
                    !r.ok && r.action ? (
                      <button
                        key={r.key}
                        className="readiness-line todo actionable"
                        onClick={() => p.onReadinessAction(r.action!)}
                      >
                        {r.label} → fix
                      </button>
                    ) : (
                      <span key={r.key} className={`readiness-line ${r.ok ? "ok" : "todo"}`}>
                        {r.label}
                      </span>
                    )
                  )}
                </span>
              )}
            </span>
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
