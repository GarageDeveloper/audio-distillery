import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import type { ProjectView } from "./types/ProjectView";
import type { ExportProgress } from "./types/ExportProgress";
import type { RegionSpan } from "./types/RegionSpan";
import { api } from "./api";
import { Toolbar, type Theme } from "./components/Toolbar";
import { Waveform } from "./components/Waveform";
import { Minimap } from "./components/Minimap";
import { TrackList } from "./components/TrackList";
import { ExportDialog } from "./components/ExportDialog";
import { AlbumMetaForm } from "./components/AlbumMetaForm";
import { Backdrop } from "./components/Backdrop";
import { MasteringPanel } from "./components/MasteringPanel";
import { clampSpanToFreeHole } from "./lib/spans";
import { EmptyState } from "./components/EmptyState";
import { RecordDialog } from "./components/RecordDialog";
import { StatusBar } from "./components/StatusBar";
import { AboutDialog } from "./components/AboutDialog";
import { usePlayback } from "./hooks/usePlayback";
import type { Viewport } from "./lib/viewport";
import { clampViewport } from "./lib/viewport";

const AUDIO_EXTS = ["wav", "flac", "mp3", "aiff", "aif"];
const THEME_KEY = "still-theme";

interface LoadState {
  active: boolean;
  progress: number;
  fileName: string;
}

export default function App() {
  const [view, setView] = useState<ProjectView | null>(null);
  const [loading, setLoading] = useState<LoadState>({ active: false, progress: 0, fileName: "" });
  // The overlay pops under the pointer right where the layout-choice button
  // was; arm Cancel after a beat so a stray double-click can't abort the
  // load (Esc stays immediate).
  const [cancelArmed, setCancelArmed] = useState(false);
  useEffect(() => {
    if (!loading.active) {
      setCancelArmed(false);
      return;
    }
    const t = window.setTimeout(() => setCancelArmed(true), 700);
    return () => window.clearTimeout(t);
  }, [loading.active]);
  const [error, setError] = useState<string | null>(null);
  const [panelOpen, setPanelOpen] = useState(true);
  const [exportOpen, setExportOpen] = useState(false);
  const [albumOpen, setAlbumOpen] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [exportProgress, setExportProgress] = useState<ExportProgress | null>(null);
  const [proposals, setProposals] = useState<RegionSpan[] | null>(null);
  /// Auto-split detection source: null = the mix, else a layer id.
  const [detectLayer, setDetectLayer] = useState<number | null>(null);
  /// Review state: excluded proposal keys. The "current" proposal is
  /// derived from the playhead — clicking the waveform or just letting
  /// playback run moves the review focus.
  const [excluded, setExcluded] = useState<Set<string>>(new Set());
  const [dropChoice, setDropChoice] = useState<string[] | null>(null);
  const [recordOpen, setRecordOpen] = useState(false);
  const [minTrackSecs, setMinTrackSecs] = useState(120);
  const [waveMode, setWaveMode] = useState<"mix" | "layers">("mix");
  const [selection, setSelection] = useState<RegionSpan | null>(null);
  const [pendingStart, setPendingStart] = useState<number | null>(null);
  const [selectedTrack, setSelectedTrack] = useState<number | null>(null);
  const [selectedClip, setSelectedClip] = useState<number | null>(null);
  const [viewport, setViewport] = useState<Viewport>({ start: 0, spp: 1 });
  const [waveWidth, setWaveWidth] = useState(1000);
  const [theme, setTheme] = useState<Theme>(
    () => (localStorage.getItem(THEME_KEY) as Theme) || "alambic"
  );
  const playback = usePlayback(view?.audio.sample_rate ?? 44100, !!view);
  const errorTimer = useRef<number | undefined>(undefined);

  // Theme is a pure display preference: applied on <html>, persisted locally.
  useEffect(() => {
    if (theme === "alambic") {
      delete document.documentElement.dataset.theme;
    } else {
      document.documentElement.dataset.theme = theme;
    }
    localStorage.setItem(THEME_KEY, theme);
  }, [theme]);

  const showError = useCallback((message: string) => {
    setError(message);
    window.clearTimeout(errorTimer.current);
    errorTimer.current = window.setTimeout(() => setError(null), 7000);
  }, []);

  /** Run a backend intention and adopt the returned canonical view. */
  const apply = useCallback(
    async (op: () => Promise<ProjectView>): Promise<ProjectView | null> => {
      try {
        const v = await op();
        setView(v);
        return v;
      } catch (e) {
        showError(String(e));
        return null;
      }
    },
    [showError]
  );

  const fitFile = useCallback((v: ProjectView, width: number) => {
    const total = v.audio.duration_samples;
    setViewport({ start: 0, spp: Math.max(total / Math.max(width, 100), 1) });
  }, []);

  // Keep a ref so event handlers know whether a session exists.
  const viewRef = useRef<ProjectView | null>(null);
  viewRef.current = view;

  const loadPaths = useCallback(
    async (
      paths: string[],
      mode: "open" | "album" | "append" | "project" | "multitrack" | "layers" | "take"
    ) => {
      const first = paths[0].split(/[/\\]/).pop() ?? paths[0];
      const fileName = paths.length > 1 ? `${first} +${paths.length - 1}` : first;
      setLoading({ active: true, progress: 0, fileName });
      let v: ProjectView | null = null;
      try {
        v =
          mode === "project"
            ? await api.loadProject(paths[0])
            : mode === "append"
              ? await api.addClips(paths)
              : mode === "take"
                ? await api.addTake(paths)
                : mode === "layers"
                  ? await api.addLayers(paths)
                : mode === "multitrack"
                  ? await api.loadMultitrack(paths)
                  : await api.loadAudio(paths);
        if (mode === "album" && v) {
          // Sequential import as album tracks: one titled track per clip.
          v = await api.clipsToTracks(null);
        }
        setView(v);
      } catch (e) {
        // A user-triggered cancel is not an error worth a toast.
        if (!/cancel/i.test(String(e))) showError(String(e));
      }
      setLoading({ active: false, progress: 0, fileName: "" });
      if (v) {
        // Always re-fit so the freshly appended clip is visible; only a new
        // session clears the working state.
        fitFile(v, waveWidth);
        if (mode !== "append" && mode !== "layers" && mode !== "take") {
          setProposals(null);
          setSelection(null);
          setPendingStart(null);
          setSelectedTrack(null);
          setSelectedClip(null);
        }
      }
    },
    [showError, fitFile, waveWidth]
  );

  // Re-clamp the viewport whenever the canvas width actually changes (first
  // measure after load, side panel collapse/expand, window resize): a
  // viewport computed for a stale width otherwise leaves a blank strip on
  // the right until the next zoom re-clamps it.
  useEffect(() => {
    if (!view) return;
    setViewport((vp) => clampViewport(vp, waveWidth, view.audio.duration_samples, 1));
  }, [waveWidth, view]);

  // Esc cancels a running analysis.
  useEffect(() => {
    if (!loading.active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") void api.cancelLoad();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [loading.active]);

  // Progress events from the backend.
  useEffect(() => {
    const un1 = listen<number>("load:progress", (e) => {
      setLoading((l) => (l.active ? { ...l, progress: e.payload } : l));
    });
    const un2 = listen<ExportProgress>("export:progress", (e) => {
      setExportProgress(e.payload);
    });
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  // Native file drag & drop. Several audio files load back-to-back as clips;
  // dropping audio on an existing session APPENDS it to the timeline. While
  // the Album dialog (or the export dialog's metadata section) is open, a
  // dropped IMAGE becomes the cover art instead of being refused as
  // non-audio.
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "drop" && e.payload.paths.length > 0) {
        const ext = (p: string) => p.split(".").pop()?.toLowerCase() ?? "";
        const still = e.payload.paths.find((p) => ext(p) === "still");
        const audio = e.payload.paths.filter((p) => AUDIO_EXTS.includes(ext(p)));
        const image = e.payload.paths.find((p) =>
          ["jpg", "jpeg", "png"].includes(ext(p))
        );
        if (image && viewRef.current && (albumOpen || exportOpen)) {
          void apply(() =>
            api.setAlbumMeta({ ...viewRef.current!.album_meta, artwork_path: image })
          );
          return;
        }
        if (image && !still && audio.length === 0) {
          showError(
            "To set this image as the album cover, open Album… (or the export dialog) first, then drop it again."
          );
          return;
        }
        if (still) {
          void loadPaths([still], "project");
        } else if (audio.length > 0) {
          // A single file on an empty app is unambiguous; anything else
          // (several files, or a session already open) asks: sequential
          // clips or synced multitrack layers?
          if (!viewRef.current && audio.length === 1) {
            void loadPaths(audio, "open");
          } else {
            setDropChoice(audio);
          }
        } else {
          showError("Unsupported file type. Drop WAV, FLAC, MP3, AIFF files or a .still project.");
        }
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [loadPaths, showError, albumOpen, exportOpen, apply]);

  const pickAudioPaths = useCallback(async (withProject: boolean) => {
    const picked = await openDialog({
      multiple: true,
      filters: withProject
        ? [
            { name: "Audio or project", extensions: [...AUDIO_EXTS, "still"] },
            { name: "Audio", extensions: AUDIO_EXTS },
            { name: "AudioDistillery project", extensions: ["still"] },
          ]
        : [{ name: "Audio", extensions: AUDIO_EXTS }],
    });
    if (!picked) return [];
    return Array.isArray(picked) ? picked : [picked];
  }, []);

  const openFile = useCallback(async () => {
    const paths = await pickAudioPaths(true);
    if (paths.length === 0) return;
    if (paths[0].toLowerCase().endsWith(".still")) {
      void loadPaths([paths[0]], "project");
    } else {
      void loadPaths(paths, "open");
    }
  }, [pickAudioPaths, loadPaths]);

  const addClips = useCallback(async () => {
    const paths = await pickAudioPaths(false);
    if (paths.length > 0) void loadPaths(paths, "append");
  }, [pickAudioPaths, loadPaths]);

  const addLayers = useCallback(async () => {
    const paths = await pickAudioPaths(false);
    if (paths.length > 0) void loadPaths(paths, "layers");
  }, [pickAudioPaths, loadPaths]);

  const addTake = useCallback(async () => {
    const paths = await pickAudioPaths(false);
    if (paths.length > 0) void loadPaths(paths, "take");
  }, [pickAudioPaths, loadPaths]);

  const saveProject = useCallback(
    async (forceAsk: boolean) => {
      if (!view) return;
      let path = view.project_path ?? undefined;
      if (!path || forceAsk) {
        const stem = (view.audio.path.split(/[/\\]/).pop() ?? "project").replace(/\.[^.]+$/, "");
        const chosen = await saveDialog({
          defaultPath: `${stem}.still`,
          filters: [{ name: "AudioDistillery project", extensions: ["still"] }],
        });
        if (!chosen) return;
        path = chosen;
      }
      await apply(() => api.saveProject(path));
    },
    [view, apply]
  );

  const addRegion = useCallback(
    (start: number, end: number, title?: string) => {
      void apply(() => api.addRegion(start, end, title)).then((v) => {
        if (v) {
          setSelection(null);
          setPendingStart(null);
          setTitleDraft("");
        }
      });
    },
    [apply]
  );

  // Title draft for the "Add track" bar, prefilled with the backend's
  // suggestion ("Jam" → "Jam-1" → "Jam-2" …) when a selection appears.
  const [titleDraft, setTitleDraft] = useState("");
  const hadSelection = useRef(false);
  useEffect(() => {
    if (selection && !hadSelection.current) {
      setTitleDraft(view?.suggested_title ?? "");
    }
    hadSelection.current = !!selection;
  }, [selection, view]);

  // Keyboard shortcuts.
  /** Delete a clip (ripple). The rescan streams load:progress, so the
   * usual loading overlay narrates it. */
  const removeClip = useCallback(
    async (index: number) => {
      setSelectedClip(null);
      setLoading({ active: true, progress: 0, fileName: "Updating timeline…" });
      try {
        const v = await api.removeClip(index);
        setView(v);
      } catch (e) {
        showError(String(e));
      }
      setLoading({ active: false, progress: 0, fileName: "" });
    },
    [showError]
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable) {
        return;
      }
      if (!view) return;
      const sr = view.audio.sample_rate;
      const playheadSample = playback.positionSeconds * sr;
      const mod = e.metaKey || e.ctrlKey;
      if (e.code === "Space") {
        e.preventDefault();
        api.playerToggle().then(playback.adopt).catch((err) => showError(String(err)));
      } else if (e.key === "m" || e.key === "M") {
        // First M drops a pending start marker at the playhead; the second M
        // turns the pair into a selection so the title can be given before
        // the track is created.
        if (pendingStart == null) {
          setPendingStart(Math.round(playheadSample));
        } else {
          setPendingStart(null);
          const spans = (viewRef.current?.tracks ?? []).map((t) => ({
            start: t.start_sample,
            end: t.end_sample,
          }));
          setSelection(
            clampSpanToFreeHole(pendingStart, Math.round(playheadSample), spans)
          );
        }
      } else if (e.key === "Enter" && selection) {
        e.preventDefault();
        addRegion(selection.start, selection.end, titleDraft);
      } else if (e.key === "Escape") {
        setPendingStart(null);
        setSelection(null);
        setProposals(null);
        setSelectedClip(null);
      } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        const delta = (e.key === "ArrowLeft" ? -1 : 1) * (e.shiftKey ? 30 : 5);
        api
          .playerSeek(Math.max(0, playheadSample + delta * sr))
          .then(playback.adopt)
          .catch((err) => showError(String(err)));
      } else if (mod && e.key === "z" && !e.shiftKey) {
        e.preventDefault();
        void apply(() => api.undo());
      } else if (mod && (e.key === "Z" || (e.shiftKey && e.key === "z") || e.key === "y")) {
        e.preventDefault();
        void apply(() => api.redo());
      } else if (mod && e.key === "s") {
        e.preventDefault();
        void saveProject(false);
      } else if ((e.key === "Delete" || e.key === "Backspace") && selectedTrack != null) {
        e.preventDefault();
        setSelectedTrack(null);
        void apply(() => api.removeRegion(selectedTrack));
      } else if ((e.key === "Delete" || e.key === "Backspace") && selectedClip != null) {
        e.preventDefault();
        void removeClip(selectedClip);
      } else if (e.key === "e" && mod) {
        e.preventDefault();
        setExportOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view, playback, apply, selectedTrack, selectedClip, removeClip, selection, pendingStart, titleDraft, addRegion, saveProject, showError]);

  const onViewportChange = useCallback(
    (vp: Viewport) => {
      if (!view) return;
      setViewport(clampViewport(vp, waveWidth, view.audio.duration_samples, 1));
    },
    [view, waveWidth]
  );

  const seekTo = useCallback(
    (sample: number) => {
      api.playerSeek(sample).then(playback.adopt).catch((e) => showError(String(e)));
    },
    [playback, showError]
  );

  /// Track-list clicks: jump to the track AND start playback if stopped.
  const seekToAndPlay = useCallback(
    (sample: number) => {
      api
        .playerSeek(sample)
        .then((s) => {
          playback.adopt(s);
          if (!s.playing) {
            return api.playerToggle().then(playback.adopt);
          }
        })
        .catch((e) => showError(String(e)));
    },
    [playback, showError]
  );

  const detectSilences = useCallback(async (layerId: number | null) => {
    try {
      const found = await api.detectSilences(
        {
          threshold_db: -40,
          min_silence_ms: 1500,
          min_track_seconds: 15,
        },
        layerId
      );
      // Hide proposals fully covered by an existing track already.
      const fresh = found.filter(
        (r) =>
          !view?.tracks.some(
            (t) => t.start_sample <= r.start && r.end <= t.end_sample
          )
      );
      // Open the bar even with ZERO results: in multitrack sessions the
      // remedy (switching the detection source to a quieter layer) lives
      // in the bar itself.
      setProposals(fresh);
      setExcluded(new Set());
    } catch (e) {
      showError(String(e));
    }
  }, [view, showError]);

  const playheadSample = playback.positionSeconds * (view?.audio.sample_rate ?? 44100);

  /** Shift-click completes a selection from the most sensible anchor:
   * the far edge of an existing selection (extension), else the pending
   * M-mark, else the playhead. */
  const onShiftClick = useCallback(
    (sample: number) => {
      const anchor = selection
        ? Math.abs(sample - selection.start) >= Math.abs(sample - selection.end)
          ? selection.start
          : selection.end
        : pendingStart ?? Math.round(playheadSample);
      setPendingStart(null);
      // Butée: the selection may not cross into an existing track.
      const spans = (viewRef.current?.tracks ?? []).map((t) => ({
        start: t.start_sample,
        end: t.end_sample,
      }));
      setSelection(clampSpanToFreeHole(anchor, sample, spans));
    },
    [selection, pendingStart, playheadSample]
  );

  // A structural change can shrink the clip list under the selection.
  useEffect(() => {
    if (view && selectedClip != null && selectedClip >= view.audio.clips.length) {
      setSelectedClip(null);
    }
  }, [view, selectedClip]);

  // Auto-split proposals filtered by the live minimum-length criterion,
  // minus the ones excluded during review.
  const sr = view?.audio.sample_rate ?? 44100;
  const spanKey = (r: RegionSpan) => `${r.start}-${r.end}`;
  const longEnough = (proposals ?? []).filter(
    (r) => (r.end - r.start) / sr >= minTrackSecs
  );
  const keptProposals = longEnough.filter((r) => !excluded.has(spanKey(r)));
  /// Proposal under the playhead (null = the cursor is outside every one).
  const reviewCurrent = longEnough.findIndex(
    (r) => playheadSample >= r.start && playheadSample < r.end
  );
  const ignoredProposals = (proposals ?? []).filter(
    (r) => (r.end - r.start) / sr < minTrackSecs
  );
  const excludedProposals = longEnough.filter((r) => excluded.has(spanKey(r)));

  return (
    <div className="app">
      <Toolbar
        onAbout={() => setAboutOpen(true)}
        view={view}
        playing={playback.playing}
        positionSeconds={playback.positionSeconds}
        panelOpen={panelOpen}
        theme={theme}
        onThemeChange={setTheme}
        waveMode={waveMode}
        onWaveModeChange={setWaveMode}
        onOpen={openFile}
        onRecord={() => setRecordOpen(true)}
        onAddClips={() => void addClips()}
        onAddTake={() => void addTake()}
        onTogglePlay={() =>
          api.playerToggle().then(playback.adopt).catch((e) => showError(String(e)))
        }
        onSave={() => void saveProject(false)}
        onSaveAs={() => void saveProject(true)}
        onUndo={() => void apply(() => api.undo())}
        onRedo={() => void apply(() => api.redo())}
        onDetectSilences={() => void detectSilences(detectLayer)}
        onExport={() => setExportOpen(true)}
        onAlbum={() => setAlbumOpen(true)}
        onTogglePanel={() => setPanelOpen((p) => !p)}
        onToggleSnap={() => view && void apply(() => api.setSnapToZero(!view.snap_to_zero))}
      />

      <div className="main">
        <div className="wave-area">
          {view ? (
            <>
              <Waveform
                view={view}
                viewport={viewport}
                playheadSample={playheadSample}
                waveMode={view.layers.length > 1 ? waveMode : "mix"}
                proposals={proposals ? keptProposals : null}
                ignoredProposals={proposals ? ignoredProposals : null}
                excludedProposals={proposals ? excludedProposals : null}
                selection={selection}
                pendingStart={pendingStart}
                selectedTrack={selectedTrack}
                selectedClip={selectedClip}
                onWidthChange={setWaveWidth}
                onViewportChange={onViewportChange}
                onSeek={seekTo}
                onSelectionChange={setSelection}
                onAddRegion={addRegion}
                onBeginEdgeDrag={() => void api.beginRegionEdit().catch(() => {})}
                onMoveEdge={(id, edge, pos) =>
                  void apply(() => api.moveRegionEdgePreview(id, edge, pos))
                }
                onSelectTrack={setSelectedTrack}
                onSelectClip={setSelectedClip}
                onShiftClick={onShiftClick}
                onToggleLayerCollapsed={(id, c) =>
                  void apply(() => api.setLayerCollapsed(id, c))
                }
                onRemoveRegion={(id) => {
                  setSelectedTrack(null);
                  void apply(() => api.removeRegion(id));
                }}
              />
              <Minimap
                view={view}
                viewport={viewport}
                width={waveWidth}
                playheadSample={playheadSample}
                onViewportChange={onViewportChange}
              />
              {selectedClip != null &&
                !selection &&
                !proposals &&
                view.audio.clips[selectedClip] && (
                  <div className="proposal-bar clip-bar">
                    <span className="clip-bar-name" title={view.audio.clips[selectedClip].path}>
                      {view.audio.clips[selectedClip].name}
                    </span>
                    <button
                      className="btn"
                      title="Create a track spanning exactly this clip, titled after the file"
                      onClick={() => {
                        const idx = selectedClip;
                        setSelectedClip(null);
                        void apply(() => api.clipsToTracks([idx]));
                      }}
                    >
                      Make track
                    </button>
                    <button
                      className="btn"
                      title="Remove this clip from the timeline — later clips and markers close the gap (undoable). Source files are never touched."
                      onClick={() => void removeClip(selectedClip)}
                    >
                      Delete clip (⌫)
                    </button>
                    <button className="btn" onClick={() => setSelectedClip(null)}>
                      Clear
                    </button>
                  </div>
                )}
              {selection && !proposals && (
                <div className="proposal-bar">
                  <input
                    className="text-input add-track-title"
                    placeholder="Track title (optional)"
                    value={titleDraft}
                    autoFocus
                    onFocus={(e) => e.target.select()}
                    onChange={(e) => setTitleDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        addRegion(selection.start, selection.end, titleDraft);
                      } else if (e.key === "Escape") {
                        setSelection(null);
                        setTitleDraft("");
                      }
                    }}
                  />
                  <button
                    className="btn btn-primary"
                    onClick={() => addRegion(selection.start, selection.end, titleDraft)}
                  >
                    Add track (⏎)
                  </button>
                  <button
                    className="btn"
                    onClick={() => {
                      setSelection(null);
                      setTitleDraft("");
                    }}
                  >
                    Clear
                  </button>
                </div>
              )}
              {pendingStart != null && !selection && (
                <div className="proposal-bar">
                  <span>
                    Track start set — press <kbd>M</kbd> again at the end position
                  </span>
                  <button className="btn" onClick={() => setPendingStart(null)}>
                    Cancel
                  </button>
                </div>
              )}
              {proposals && (
                <div className="proposal-bar">
                  <div className="proposal-controls">
                  {(view?.layers.length ?? 0) > 1 && (
                    <label className="proposal-source" title="Which signal the silence detection listens to — a between-songs-quiet layer often beats the mix">
                      Detect on
                      <span className="select-wrap">
                      <select
                        value={detectLayer ?? "mix"}
                        onChange={(e) => {
                          const v =
                            e.target.value === "mix" ? null : Number(e.target.value);
                          setDetectLayer(v);
                          void detectSilences(v);
                        }}
                      >
                        <option value="mix">Mix</option>
                        {view?.layers.map((l) => (
                          <option key={l.id} value={l.id}>
                            {l.name}
                          </option>
                        ))}
                      </select>
                      <span className="select-chevron">▾</span>
                      </span>
                    </label>
                  )}

                  {longEnough.length > 0 && (
                    <span className="proposal-review">
                      <button
                        className="btn btn-icon"
                        title="Previous proposal before the playhead (plays from its start)"
                        onClick={() => {
                          const before = [...longEnough]
                            .reverse()
                            .find((r) => r.start < playheadSample - sr * 0.5);
                          const target = before ?? longEnough[longEnough.length - 1];
                          seekToAndPlay(target.start);
                        }}
                      >
                        ‹
                      </button>
                      <span
                        className="review-pos"
                        title={
                          reviewCurrent >= 0
                            ? "The playhead is inside this proposal"
                            : "The playhead is outside every proposal — click the waveform or use ‹ ›"
                        }
                      >
                        {reviewCurrent >= 0 ? reviewCurrent + 1 : "–"}/{longEnough.length}
                      </span>
                      <button
                        className="btn btn-icon"
                        title="Next proposal after the playhead (plays from its start)"
                        onClick={() => {
                          const after = longEnough.find((r) => r.start > playheadSample + 1);
                          const target = after ?? longEnough[0];
                          seekToAndPlay(target.start);
                        }}
                      >
                        ›
                      </button>
                      <button
                        className="btn btn-icon"
                        disabled={reviewCurrent < 0}
                        title="Audition this proposal's ENDING (plays the last seconds)"
                        onClick={() => {
                          const r = longEnough[reviewCurrent];
                          seekToAndPlay(Math.max(r.start, r.end - 5 * sr));
                        }}
                      >
                        ⇥
                      </button>
                      {reviewCurrent >= 0 ? (
                        (() => {
                          const r = longEnough[reviewCurrent];
                          const out = excluded.has(spanKey(r));
                          return (
                            <button
                              className={`btn proposal-keep ${out ? "excluded" : ""}`}
                              title={out ? "Excluded — click to keep this track" : "Kept — click to exclude this track"}
                              onClick={() => {
                                const next = new Set(excluded);
                                if (out) next.delete(spanKey(r));
                                else next.add(spanKey(r));
                                setExcluded(next);
                              }}
                            >
                              {out ? "✕ excluded" : "✓ kept"}
                            </button>
                          );
                        })()
                      ) : (
                        <span className="proposal-outside">outside</span>
                      )}
                    </span>
                  )}

                  <label className="proposal-min">
                    min
                    <input
                      type="number"
                      min={0}
                      step={5}
                      value={minTrackSecs}
                      onChange={(e) =>
                        setMinTrackSecs(Math.max(0, Number(e.target.value) || 0))
                      }
                    />
                    s
                  </label>
                  <button
                    className="btn btn-primary"
                    disabled={keptProposals.length === 0}
                    onClick={() => {
                      void apply(() => api.addRegions(keptProposals));
                      setProposals(null);
                    }}
                  >
                    Add {keptProposals.length} track{keptProposals.length !== 1 ? "s" : ""}
                  </button>
                  <button className="btn" onClick={() => setProposals(null)}>
                    Dismiss
                  </button>
                  </div>

                  <div className="proposal-result">
                    {(proposals?.length ?? 0) === 0 ? (
                      <span className="proposal-ignored">
                        Nothing detected — try another source or lower thresholds
                      </span>
                    ) : (
                      <>
                        <strong>{keptProposals.length}</strong>&nbsp;track
                        {keptProposals.length !== 1 ? "s" : ""} kept
                        {ignoredProposals.length > 0 && (
                          <span className="proposal-ignored">
                            {" "}· {ignoredProposals.length} left out
                          </span>
                        )}
                      </>
                    )}
                  </div>
                </div>
              )}
            </>
          ) : (
            <EmptyState onOpen={openFile} onRecord={() => setRecordOpen(true)} />
          )}
          {loading.active && (
            <div className="loading-overlay">
              <div className="loading-card">
                <div className="loading-title">Analyzing {loading.fileName}…</div>
                <div className="progress-track">
                  <div
                    className="progress-fill"
                    style={{ width: `${Math.round(loading.progress * 100)}%` }}
                  />
                </div>
                <div className="loading-foot">
                  <span className="loading-pct">{Math.round(loading.progress * 100)}%</span>
                  <button
                    className="btn"
                    disabled={!cancelArmed}
                    onClick={() => {
                      if (cancelArmed) void api.cancelLoad();
                    }}
                  >
                    Cancel (Esc)
                  </button>
                </div>
              </div>
            </div>
          )}
        </div>

        {view && panelOpen && (
          <TrackList
            view={view}
            playheadSample={playheadSample}
            selectedTrack={selectedTrack}
            onSelectTrack={setSelectedTrack}
            onRename={(id, title) => void apply(() => api.renameTrack(id, title))}
            onRemoveRegion={(id) => void apply(() => api.removeRegion(id))}
            onSeek={seekToAndPlay}
            onLayerRename={(id, name) => void apply(() => api.renameLayer(id, name))}
            onLayerGain={(id, db) => void apply(() => api.setLayerGain(id, db))}
            onLayerMute={(id, m) => void apply(() => api.setLayerMuted(id, m))}
            onLayerSolo={(id, so) => void apply(() => api.setLayerSolo(id, so))}
            onLayerCollapse={(id, c) => void apply(() => api.setLayerCollapsed(id, c))}
            onLayerRemove={(id) => void apply(() => api.removeLayer(id))}
            onAddLayers={() => void addLayers()}
            onTrackLayerGain={(trackId, layerId, db) =>
              void apply(() => api.setTrackLayerGain(trackId, layerId, db))
            }
            onTrackLayerMute={(trackId, layerId, m) =>
              void apply(() => api.setTrackLayerMute(trackId, layerId, m))
            }
            onTrackLayerSolo={(trackId, layerId, so) =>
              void apply(() => api.setTrackLayerSolo(trackId, layerId, so))
            }
            onDiscBreaksChange={(breaks) =>
              view &&
              void apply(() =>
                api.setAlbumMeta({ ...view.album_meta, disc_breaks: breaks })
              )
            }
          />
        )}

        {view && (
          <MasteringPanel
            view={view}
            playheadSample={playheadSample}
            playing={playback.playing}
            onError={showError}
            onViewChange={setView}
          />
        )}
      </div>

      <StatusBar
        view={view}
        deviceError={playback.deviceError}
        onAbout={() => setAboutOpen(true)}
      />

      {aboutOpen && <AboutDialog onClose={() => setAboutOpen(false)} />}

      {recordOpen && (
        <RecordDialog
          onClose={() => setRecordOpen(false)}
          onError={showError}
          onRecorded={(paths) => {
            setRecordOpen(false);
            if (paths.length === 0) return;
            if (view) {
              // A session is open: adding recorded lanes has real
              // alternatives (append, layers, take) — keep the choice.
              setDropChoice(paths);
            } else {
              // A fresh recording IS a synced multitrack session by
              // construction: skip the question and land on the
              // per-layer view, where the take is actually visible.
              setWaveMode("layers");
              void loadPaths(paths, "multitrack");
            }
          }}
        />
      )}

      {error && (
        <div className="toast toast-error" onClick={() => setError(null)}>
          {error}
        </div>
      )}

      {albumOpen && view && (
        <Backdrop onClose={() => {}}>
          <div className="modal">
            <div>
              <h2>Album metadata</h2>
              <div className="subtitle">
                Saved in the project, written natively into every exported file (ID3, MP4,
                Vorbis, RIFF)
              </div>
            </div>
            <AlbumMetaForm
              meta={view.album_meta}
              onChange={(m) => void apply(() => api.setAlbumMeta(m))}
            />
            <div className="modal-foot">
              <button className="btn btn-primary" onClick={() => setAlbumOpen(false)}>
                Done
              </button>
            </div>
          </div>
        </Backdrop>
      )}

      {dropChoice && (
        <Backdrop onClose={() => setDropChoice(null)}>
          <div className="modal drop-choice">
            <h2>
              {dropChoice.length} audio file{dropChoice.length > 1 ? "s" : ""}
            </h2>
            <div className="subtitle">
              {view
                ? "Add them to the current session as…"
                : "How should they be laid out?"}
            </div>
            <button
              className="btn choice"
              onClick={() => {
                void loadPaths(dropChoice, view ? "append" : "open");
                setDropChoice(null);
              }}
            >
              <strong>{view ? "Append to timeline" : "One after another"}</strong>
              <span>Clips laid back-to-back on one timeline (vinyl sides, concert parts)</span>
            </button>
            {!view && dropChoice.length > 1 && (
              <button
                className="btn choice"
                onClick={() => {
                  void loadPaths(dropChoice, "album");
                  setDropChoice(null);
                }}
              >
                <strong>One after another — as album tracks</strong>
                <span>Each file becomes a clip AND a titled track — ready to master an album</span>
              </button>
            )}
            <button
              className="btn choice"
              onClick={() => {
                void loadPaths(dropChoice, view ? "layers" : "multitrack");
                setDropChoice(null);
              }}
            >
              <strong>{view ? "Add as synced layers" : "Synced multitrack"}</strong>
              <span>Time-aligned recordings of the same session (field-recorder inputs), mixed together</span>
            </button>
            {view && view.layers.length > 1 && (
              <button
                className="btn choice"
                onClick={() => {
                  void loadPaths(dropChoice, "take");
                  setDropChoice(null);
                }}
              >
                <strong>Append as next take</strong>
                <span>
                  One file per layer ({view.layers.length} expected, matched by name order),
                  starting together right after the current timeline
                </span>
              </button>
            )}
            <div className="modal-foot">
              <button className="btn" onClick={() => setDropChoice(null)}>
                Cancel
              </button>
            </div>
          </div>
        </Backdrop>
      )}

      {exportOpen && view && (
        <ExportDialog
          view={view}
          progress={exportProgress}
          onClose={() => {
            setExportOpen(false);
            setExportProgress(null);
          }}
          onError={showError}
          onViewChange={setView}
        />
      )}
    </div>
  );
}
