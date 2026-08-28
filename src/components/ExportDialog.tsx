import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import type { ProjectView } from "../types/ProjectView";
import type { ExportConfig } from "../types/ExportConfig";
import type { ExportFormat } from "../types/ExportFormat";
import type { ExportProgress } from "../types/ExportProgress";
import type { ExportReport } from "../types/ExportReport";
import type { AlbumMeta } from "../types/AlbumMeta";
import { api } from "../api";
import { AlbumMetaForm } from "./AlbumMetaForm";
import { Backdrop } from "./Backdrop";

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
  { key: "source", label: "Source" },
];
const DEFAULT_TEMPLATE = "{n} - {title}";
const STEMS_TEMPLATE = "{n} - {title}/{ln} - {layer}";
const BITRATES = [320, 256, 192, 128];
// Default bitrate applied when picking a lossy format: MP3 → 320 kbps,
// AAC → 128 kbps CBR (WaveLab's "iTunes Standard" preset).
const DEFAULT_BITRATE: Partial<Record<ExportFormat, number>> = { mp3: 320, aac: 128 };
const EXT: Record<ExportFormat, string> = { wav: "wav", flac: "flac", mp3: "mp3", aac: "m4a", source: "wav" };

type Phase = "settings" | "running" | "report";

export function ExportDialog({ view, progress, onClose, onError, onViewChange }: Props) {
  const [cfg, setCfg] = useState<ExportConfig>({ ...view.export_config });
  /// The Red Book-compatible combination the CD features require.
  const cdCombo =
    cfg.format === "wav" && cfg.bit_depth <= 16 && cfg.target_sample_rate === 44100;
  useEffect(() => {
    if (!cdCombo && (cfg.cd_image || cfg.ddp))
      setCfg((c) => ({ ...c, cd_image: false, ddp: false }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cdCombo]);
  // Collapsed by default: metadata is edited any time via the Album… button;
  // this section is just a shortcut.
  const [metaOpen, setMetaOpen] = useState(false);
  const meta: AlbumMeta = view.album_meta;
  const saveMeta = (m: AlbumMeta) => {
    api.setAlbumMeta(m).then(onViewChange).catch((e) => onError(String(e)));
  };
  const [phase, setPhase] = useState<Phase>("settings");
  const [report, setReport] = useState<ExportReport | null>(null);
  // Tracks export in parallel: accumulate the per-track progress events.
  const [perTrack, setPerTrack] = useState<Record<number, number>>({});
  // Stems mode: the rows (track × layer) are only known from the events.
  const [stemRows, setStemRows] = useState<Record<number, { title: string; pct: number }>>({});

  useEffect(() => {
    // Analysis events reuse the same channel but count measurement steps,
    // not encoded tracks — keep them out of the per-track bars.
    if (progress && !progress.analyzing) {
      setPerTrack((m) => ({ ...m, [progress.track_number]: progress.track_progress }));
      setStemRows((m) => ({
        ...m,
        [progress.track_number]: { title: progress.track_title, pct: progress.track_progress },
      }));
    }
  }, [progress]);

  // Stems naming: swap the default template both ways, leave any custom
  // template alone.
  useEffect(() => {
    setCfg((c) => {
      if (c.stems && c.template === DEFAULT_TEMPLATE) return { ...c, template: STEMS_TEMPLATE };
      if (!c.stems && c.template === STEMS_TEMPLATE) return { ...c, template: DEFAULT_TEMPLATE };
      return c;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cfg.stems]);

  // Multi-disc album still on the default naming: sort tracks into one
  // folder per disc.
  useEffect(() => {
    if (
      view.album_meta.disc_breaks.length > 0 &&
      cfg.template === "{n} - {title}"
    ) {
      setCfg((c) => ({ ...c, template: "Disc{disc} - {n} - {title}" }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view.album_meta.disc_breaks.length]);

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
    const breaks = view.album_meta.disc_breaks
      .filter((b) => b >= 2 && b <= view.tracks.length)
      .sort((a, b) => a - b);
    const starts = [1, ...breaks];
    let discIdx = 0;
    for (let i = 0; i < starts.length; i++) if (starts[i] <= t.number) discIdx = i;
    const inDisc = t.number - starts[discIdx] + 1;
    const width = Math.max(String(view.tracks.length).length, 2);
    const name = cfg.template
      .replace("{n}", String(breaks.length > 0 ? inDisc : t.number).padStart(width, "0"))
      .replace("{disc}", String(discIdx + 1))
      .replace("{title}", t.title)
      .replace("{titre}", t.title)
      .replace("{album}", view.album_meta.album)
      .replace("{year}", view.album_meta.date.replace(/\D/g, "").slice(0, 4))
      .replace("{source}", view.audio.path.split(/[/\\]/).pop()?.replace(/\.[^.]+$/, "") ?? "")
      .replace("{ln}", "01")
      .replace("{layer}", view.layers[0]?.name.replace(/\.[^.]+$/, "") ?? "Layer 1");
    const ext =
      cfg.format === "source"
        ? view.audio.path.split(".").pop()?.toLowerCase() ?? "wav"
        : EXT[cfg.format];
    return `${name}.${ext}`;
  };

  const start = async () => {
    if (!cfg.dest_dir.trim()) {
      onError("Choose a destination folder first.");
      return;
    }
    setPhase("running");
    setPerTrack({});
    setStemRows({});
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

  // Per-track ISRC drafts (committed on blur; backend validates).
  const [isrcDraft, setIsrcDraft] = useState<Record<number, string>>({});
  const commitIsrc = (id: number) => {
    const t = view.tracks.find((x) => x.id === id);
    const draft = isrcDraft[id];
    if (t === undefined || draft === undefined || draft === t.isrc) return;
    api
      .setTrackIsrc(id, draft)
      .then((v) => {
        onViewChange(v);
        setIsrcDraft((m) => {
          const { [id]: _, ...rest } = m;
          return rest;
        });
      })
      .catch((e) => onError(String(e)));
  };

  return (
    <Backdrop
      onClose={() => {
        if (phase !== "running") onClose();
      }}
    >
      <div className="modal export-modal">
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
              <div className="preset-row">
                <button
                  className={`btn cd-preset ${cdCombo ? "active" : ""}`}
                  title="Red Book CD delivery: WAV · 44.1 kHz · 16-bit · dithered"
                  onClick={() =>
                    setCfg({
                      ...cfg,
                      format: "wav",
                      bit_depth: 16,
                      target_sample_rate: 44100,
                      dither: "auto",
                    })
                  }
                >
                  CD preset (44.1 kHz · 16-bit · dither)
                </button>
                {cdCombo && !cfg.stems && (
                  <select
                    className="select cd-deliverable"
                    value={cfg.ddp ? "ddp" : cfg.cd_image ? "cue" : "tracks"}
                    onChange={(e) =>
                      setCfg({
                        ...cfg,
                        cd_image: e.target.value === "cue",
                        ddp: e.target.value === "ddp",
                        stems: false,
                      })
                    }
                    title="How the Red Book master is delivered"
                  >
                    <option value="tracks">Separate track files</option>
                    <option value="cue">Single image + cue sheet (burning)</option>
                    <option value="ddp">DDP fileset + PQ sheet (pressing plant)</option>
                  </select>
                )}
                {cdCombo && !cfg.stems && (cfg.cd_image || cfg.ddp) && (
                  <div className="hint">
                    Track gaps: {(view.album_gap_ms / 1000).toFixed(1)} s default between
                    titles (edit in Album…, override any boundary in the track list) —
                    pressed into the image exactly as heard in the Album program.
                  </div>
                )}
              </div>
              {cfg.format === "source" && (
                <div className="hint">
                  Mirrors each original file: same container, bit depth and sample rate
                  {cfg.stems ? " — every stem keeps its own layer's format" : ""}.
                </div>
              )}
            </div>

            <div className="field">
              <label className="cd-image-toggle stems-toggle" title="One file per layer per track, laid out as one folder per track — for re-importing into a DAW">
                <input
                  type="checkbox"
                  checked={cfg.stems}
                  onChange={(e) =>
                    setCfg({
                      ...cfg,
                      stems: e.target.checked,
                      cd_image: e.target.checked ? false : cfg.cd_image,
                      ddp: e.target.checked ? false : cfg.ddp,
                    })
                  }
                />
                Multitrack stems — one file per layer, one folder per track
              </label>
              {cfg.stems && (
                <select
                  className="select"
                  value={cfg.stems_apply_mix ? "mix" : "raw"}
                  onChange={(e) =>
                    setCfg({ ...cfg, stems_apply_mix: e.target.value === "mix" })
                  }
                  title="Raw cuts are untouched slices of each layer; mix settings apply layer gain, mute/solo and layer inserts (never the track or mastering chains)"
                >
                  <option value="raw">Raw cuts — untouched layer audio (DAW re-import)</option>
                  <option value="mix">With mix settings — layer gain/mute/solo + layer inserts</option>
                </select>
              )}
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
              {(cfg.format === "wav" || cfg.format === "flac") && (
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
              {cfg.format !== "source" && (
              <div className="field">
                <label>Sample rate</label>
                <select
                  className="select"
                  value={cfg.target_sample_rate ?? "session"}
                  onChange={(e) =>
                    setCfg({
                      ...cfg,
                      target_sample_rate:
                        e.target.value === "session" ? null : Number(e.target.value),
                    })
                  }
                >
                  <option value="session">
                    Session ({(view.audio.sample_rate / 1000).toLocaleString("en-US", { maximumFractionDigits: 1 })} kHz)
                  </option>
                  <option value={44100}>44.1 kHz</option>
                  <option value={48000}>48 kHz</option>
                  <option value={96000}>96 kHz</option>
                </select>
              </div>
              )}
              {(cfg.format === "wav" || cfg.format === "flac") && cfg.bit_depth <= 16 && (
                <div className="field">
                  <label>Dither</label>
                  <select
                    className="select"
                    value={cfg.dither}
                    onChange={(e) =>
                      setCfg({ ...cfg, dither: e.target.value as ExportConfig["dither"] })
                    }
                    title="Applied when reducing to 16-bit; Off truncates (not recommended)"
                  >
                    <option value="auto">Auto (triangular HP)</option>
                    <option value="triangular">Triangular</option>
                    <option value="triangular_hp">Triangular HP</option>
                    <option value="shibata">Shibata</option>
                    <option value="off">Off</option>
                  </select>
                </div>
              )}
            </div>



            {(cfg.ddp || cfg.cd_image) && (
              <div className="field">
                <label>ISRC per track</label>
                <div className="isrc-grid">
                  {view.tracks.map((t) => (
                    <label key={t.id} className="isrc-row" title="International Standard Recording Code — written into the CD subcode (DDP) and the cue sheet. Leave empty if you don't have one.">
                      <span className="isrc-name">
                        {String(t.number).padStart(2, "0")} · {t.title}
                      </span>
                      <input
                        className="text-input mono isrc-input"
                        placeholder="CC-XXX-YY-NNNNN"
                        value={isrcDraft[t.id] ?? t.isrc}
                        onChange={(e) =>
                          setIsrcDraft((m) => ({ ...m, [t.id]: e.target.value }))
                        }
                        onBlur={() => commitIsrc(t.id)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                        }}
                      />
                    </label>
                  ))}
                </div>
                <div className="hint">
                  Optional — stored in the project, embedded in the {cfg.ddp ? "DDP subcode and PQ sheet" : "cue sheet"}.
                </div>
              </div>
            )}

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
              <button
                className="meta-toggle"
                onClick={() => setMetaOpen(!metaOpen)}
                aria-expanded={metaOpen}
              >
                {metaOpen ? "▾" : "▸"} Metadata (tags)
                <span className="hint-inline">
                  written natively per format — ID3, MP4, Vorbis, RIFF
                </span>
              </button>
              {metaOpen && (
                <AlbumMetaForm meta={meta} onChange={saveMeta} showDiscBreaks />
              )}
            </div>

            <div className="field">
              <label>File naming</label>
              <input
                className="text-input mono"
                value={cfg.template}
                onChange={(e) => setCfg({ ...cfg, template: e.target.value })}
              />
              <div className="hint">
                Preview: {preview()} — macros: {"{n} {title} {disc} {album} {year}"}
                {cfg.stems ? " {layer} {ln}" : ""} … — a
                {" / "}creates a subfolder (e.g. {"{disc}/{n} - {title}"})
              </div>
            </div>

            <div className="modal-foot">
              <button className="btn" onClick={onClose}>
                Cancel
              </button>
              <button className="btn btn-primary" onClick={() => void start()}>
                {cfg.stems
                  ? `Export ${trackCount * Math.max(view.layers.length, 1)} stems`
                  : `Export ${trackCount} track${trackCount > 1 ? "s" : ""}`}
              </button>
            </div>
          </>
        )}

        {phase === "running" && (
          <>
            <h2>Exporting…</h2>
            <div className="export-rows">
              {cfg.stems
                ? Object.entries(stemRows)
                    .sort((a, b) => Number(a[0]) - Number(b[0]))
                    .map(([num, r]) => {
                      const state = r.pct >= 1 ? "done" : r.pct > 0 ? "active" : "waiting";
                      return (
                        <div key={num} className={`export-row ${state}`}>
                          <span className="num">{num.padStart(2, "0")}</span>
                          <span className="name">{r.title}</span>
                          <span className="bar">
                            <div style={{ width: `${Math.round(r.pct * 100)}%` }} />
                          </span>
                          <span className="st">
                            {state === "done" ? "done" : `${Math.round(r.pct * 100)}%`}
                          </span>
                        </div>
                      );
                    })
                : view.tracks.map((t) => {
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
              {progress?.analyzing && (
                <div className="export-row active">
                  <span className="num">~</span>
                  <span className="name">Analyzing loudness — {progress.track_title}</span>
                  <span className="bar">
                    <div
                      style={{ width: `${Math.round(progress.overall_progress * 100)}%` }}
                    />
                  </span>
                  <span className="st">
                    {progress.completed_tracks} / {progress.track_count}
                  </span>
                </div>
              )}
            </div>
            <div className="export-global">
              <div className="progress-track">
                <div
                  className="progress-fill"
                  style={{
                    width: `${
                      progress?.analyzing
                        ? 100
                        : Math.round((progress?.overall_progress ?? 0) * 100)
                    }%`,
                  }}
                />
              </div>
              <div className="row">
                <span>
                  {progress?.analyzing
                    ? "All tracks encoded — measuring loudness…"
                    : `${progress?.completed_tracks ?? 0} of ${progress?.track_count ?? trackCount} done`}
                </span>
                <span>
                  {progress?.analyzing
                    ? "100%"
                    : `${Math.round((progress?.overall_progress ?? 0) * 100)}%`}
                </span>
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
              {(() => {
                const measured = report.files.filter((f) => f.lufs_i != null);
                const mean =
                  measured.length > 1
                    ? measured.reduce((a, f) => a + (f.lufs_i ?? 0), 0) / measured.length
                    : null;
                return report.files.map((f) => {
                  const outlier =
                    mean != null && f.lufs_i != null && Math.abs(f.lufs_i - mean) > 1.5;
                  const hotTp = f.true_peak_db != null && f.true_peak_db > -1;
                  const measured = f.track_measures.filter((m) => m.lufs_i != null);
                  const trackMean =
                    measured.length > 1
                      ? measured.reduce((a, m) => a + (m.lufs_i ?? 0), 0) / measured.length
                      : null;
                  return (
                    <span key={f.path} className="report-group">
                      <span className="ok report-row" title={f.path}>
                        <span className="report-name">{f.path.split(/[/\\]/).pop()}</span>
                        {f.lufs_i != null && (
                          <span
                            className={`report-lufs ${outlier ? "outlier" : ""}`}
                            title={
                              outlier
                                ? "More than 1.5 LU away from the album average — check this track's level"
                                : "Integrated loudness / max true peak of the delivered file"
                            }
                          >
                            {f.lufs_i.toFixed(1)} LUFS-I
                            {f.true_peak_db != null && (
                              <span
                                className={hotTp ? "report-tp-hot" : undefined}
                                title={
                                  hotTp
                                    ? "True peak above −1 dBTP — this file will clip on playback or lossy decoding; lower the level or add a limiter"
                                    : undefined
                                }
                              >
                                {` · ${f.true_peak_db.toFixed(1)} dBTP`}
                              </span>
                            )}
                          </span>
                        )}
                      </span>
                      {f.track_measures.map((m) => {
                        const mOutlier =
                          trackMean != null &&
                          m.lufs_i != null &&
                          Math.abs(m.lufs_i - trackMean) > 1.5;
                        const mHot = m.true_peak_db != null && m.true_peak_db > -1;
                        return (
                          <span
                            key={m.number}
                            className="report-row report-track"
                            title="Measured on this track's cue segment of the image"
                          >
                            <span className="report-name">
                              {String(m.number).padStart(2, "0")} · {m.title}
                            </span>
                            {m.lufs_i != null && (
                              <span className={`report-lufs ${mOutlier ? "outlier" : ""}`}>
                                {m.lufs_i.toFixed(1)} LUFS-I
                                {m.true_peak_db != null && (
                                  <span className={mHot ? "report-tp-hot" : undefined}>
                                    {` · ${m.true_peak_db.toFixed(1)} dBTP`}
                                  </span>
                                )}
                              </span>
                            )}
                          </span>
                        );
                      })}
                    </span>
                  );
                });
              })()}
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
    </Backdrop>
  );
}
