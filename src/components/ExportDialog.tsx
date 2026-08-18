import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import type { ProjectView } from "../types/ProjectView";
import type { ExportConfig } from "../types/ExportConfig";
import type { ExportFormat } from "../types/ExportFormat";
import type { ExportProgress } from "../types/ExportProgress";
import type { ExportReport } from "../types/ExportReport";
import { api } from "../api";

interface Props {
  view: ProjectView;
  progress: ExportProgress | null;
  onClose: () => void;
  onError: (msg: string) => void;
  onViewChange: (v: ProjectView) => void;
}

const FORMATS: { key: ExportFormat; label: string }[] = [
  { key: "wav", label: "WAV" },
  { key: "flac", label: "FLAC" },
  { key: "mp3", label: "MP3" },
  { key: "aac", label: "AAC" },
];
const BITRATES = [320, 256, 192, 128];
// Default bitrate applied when picking a lossy format: MP3 → 320 kbps,
// AAC → 128 kbps CBR (WaveLab's "iTunes Standard" preset).
const DEFAULT_BITRATE: Partial<Record<ExportFormat, number>> = { mp3: 320, aac: 128 };
const EXT: Record<ExportFormat, string> = { wav: "wav", flac: "flac", mp3: "mp3", aac: "m4a" };

type Phase = "settings" | "running" | "report";

export function ExportDialog({ view, progress, onClose, onError, onViewChange }: Props) {
  const [cfg, setCfg] = useState<ExportConfig>({ ...view.export_config });
  const [phase, setPhase] = useState<Phase>("settings");
  const [report, setReport] = useState<ExportReport | null>(null);
  // Tracks export in parallel: accumulate the per-track progress events.
  const [perTrack, setPerTrack] = useState<Record<number, number>>({});

  useEffect(() => {
    if (progress) {
      setPerTrack((m) => ({ ...m, [progress.track_number]: progress.track_progress }));
    }
  }, [progress]);

  // Default destination: ~/Music/AudioDistillery (never the source folder).
  useEffect(() => {
    if (!cfg.dest_dir) {
      api
        .getDefaultExportDir()
        .then((d) => setCfg((c) => (c.dest_dir ? c : { ...c, dest_dir: d })))
        .catch(() => {});
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const preview = () => {
    const t = view.tracks[Math.min(1, view.tracks.length - 1)];
    const width = Math.max(String(view.tracks.length).length, 2);
    const n = String(t.number).padStart(width, "0");
    const name = cfg.template
      .replace("{n}", n)
      .replace("{title}", t.title)
      .replace("{titre}", t.title)
      .replace("{source}", view.audio.path.split(/[/\\]/).pop()?.replace(/\.[^.]+$/, "") ?? "");
    return `${name}.${EXT[cfg.format]}`;
  };

  const start = async () => {
    if (!cfg.dest_dir.trim()) {
      onError("Choose a destination folder first.");
      return;
    }
    setPhase("running");
    setPerTrack({});
    try {
      const r = await api.exportTracks(cfg);
      setReport(r);
      setPhase("report");
      // The backend stored the config in the project; refresh the view.
      const v = await api.getProjectView();
      onViewChange(v);
    } catch (e) {
      onError(String(e));
      setPhase("settings");
    }
  };

  const trackCount = view.tracks.length;

  return (
    <div
      className="modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget && phase !== "running") onClose();
      }}
    >
      <div className="modal">
        {phase === "settings" && (
          <>
            <div>
              <h2>
                Export {trackCount} track{trackCount > 1 ? "s" : ""}
              </h2>
              <div className="subtitle">{view.audio.path.split(/[/\\]/).pop()}</div>
            </div>

            <div className="field">
              <label>Format</label>
              <div className="segmented">
                {FORMATS.map((f) => (
                  <button
                    key={f.key}
                    className={cfg.format === f.key ? "on" : ""}
                    onClick={() =>
                      setCfg({
                        ...cfg,
                        format: f.key,
                        bitrate_kbps: DEFAULT_BITRATE[f.key] ?? cfg.bitrate_kbps,
                      })
                    }
                  >
                    {f.label}
                  </button>
                ))}
              </div>
            </div>

            <div className="field-row">
              {(cfg.format === "mp3" || cfg.format === "aac") && (
                <div className="field">
                  <label>Quality</label>
                  <select
                    className="select"
                    value={cfg.bitrate_kbps}
                    onChange={(e) => setCfg({ ...cfg, bitrate_kbps: Number(e.target.value) })}
                  >
                    {BITRATES.map((b) => (
                      <option key={b} value={b}>
                        {b} kbps
                        {cfg.format === "aac" && b === 128 ? " (iTunes Standard)" : ""}
                      </option>
                    ))}
                  </select>
                </div>
              )}
              {cfg.format === "wav" && (
                <div className="field">
                  <label>Bit depth</label>
                  <select
                    className="select"
                    value={cfg.bit_depth}
                    onChange={(e) => setCfg({ ...cfg, bit_depth: Number(e.target.value) })}
                  >
                    <option value={16}>16-bit</option>
                    <option value={24}>24-bit</option>
                  </select>
                </div>
              )}
            </div>

            <div className="field">
              <label>Destination</label>
              <div className="dest-row">
                <input className="text-input mono" value={cfg.dest_dir} readOnly title={cfg.dest_dir} />
                <button
                  className="btn"
                  onClick={async () => {
                    const dir = await openDialog({ directory: true, defaultPath: cfg.dest_dir || undefined });
                    if (typeof dir === "string") setCfg({ ...cfg, dest_dir: dir });
                  }}
                >
                  Choose…
                </button>
              </div>
              <div className="hint">Existing files are never overwritten — a suffix is added instead.</div>
            </div>

            <div className="field">
              <label>File naming</label>
              <input
                className="text-input mono"
                value={cfg.template}
                onChange={(e) => setCfg({ ...cfg, template: e.target.value })}
              />
              <div className="hint">
                Preview: {preview()} — placeholders: {"{n} {title} {source}"}
              </div>
            </div>

            <div className="modal-foot">
              <button className="btn" onClick={onClose}>
                Cancel
              </button>
              <button className="btn btn-primary" onClick={() => void start()}>
                Export {trackCount} track{trackCount > 1 ? "s" : ""}
              </button>
            </div>
          </>
        )}

        {phase === "running" && (
          <>
            <h2>Exporting…</h2>
            <div className="export-rows">
              {view.tracks.map((t) => {
                const pct = perTrack[t.number] ?? 0;
                const state = pct >= 1 ? "done" : pct > 0 ? "active" : "waiting";
                return (
                  <div key={t.id} className={`export-row ${state}`}>
                    <span className="num">{String(t.number).padStart(2, "0")}</span>
                    <span className="name">{t.title}</span>
                    <span className="bar">
                      <div style={{ width: `${Math.round(pct * 100)}%` }} />
                    </span>
                    <span className="st">
                      {state === "done" ? "done" : state === "active" ? `${Math.round(pct * 100)}%` : "waiting"}
                    </span>
                  </div>
                );
              })}
            </div>
            <div className="export-global">
              <div className="progress-track">
                <div
                  className="progress-fill"
                  style={{ width: `${Math.round((progress?.overall_progress ?? 0) * 100)}%` }}
                />
              </div>
              <div className="row">
                <span>
                  {progress?.completed_tracks ?? 0} of {progress?.track_count ?? trackCount} done
                </span>
                <span>{Math.round((progress?.overall_progress ?? 0) * 100)}%</span>
              </div>
            </div>
            <div className="modal-foot">
              <button
                className="btn"
                onClick={() => {
                  void api.cancelExport();
                }}
              >
                Cancel export
              </button>
            </div>
          </>
        )}

        {phase === "report" && report && (
          <>
            <h2>Export finished</h2>
            {report.cancelled ? (
              <div className="report-banner err">Export cancelled — {report.files.length} file(s) were written before stopping.</div>
            ) : report.errors.length === 0 ? (
              <div className="report-banner">
                <span>
                  {report.files.length} track{report.files.length > 1 ? "s" : ""} exported
                </span>
                <span className="path">{cfg.dest_dir}</span>
              </div>
            ) : (
              <div className="report-banner err">
                {report.files.length} exported · {report.errors.length} failed
              </div>
            )}
            <div className="report-files">
              {report.files.map((f) => (
                <span key={f.path} className="ok" title={f.path}>
                  {f.path.split(/[/\\]/).pop()}
                </span>
              ))}
              {report.errors.map((e, i) => (
                <span key={i} className="fail">
                  {e}
                </span>
              ))}
            </div>
            <div className="modal-foot">
              {report.files.length > 0 && (
                <button
                  className="btn"
                  onClick={() => {
                    revealItemInDir(report.files[0].path).catch((e) => onError(String(e)));
                  }}
                >
                  Show in folder
                </button>
              )}
              <button className="btn btn-primary" onClick={onClose}>
                Done
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
