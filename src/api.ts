// Typed wrappers around the Tauri commands. The frontend only ever sends
// intentions and displays what the backend returns (SPEC §3).
import { invoke } from "@tauri-apps/api/core";
import type { ProjectView } from "./types/ProjectView";
import type { PeakSlice } from "./types/PeakSlice";
import type { PlaybackState } from "./types/PlaybackState";
import type { ExportConfig } from "./types/ExportConfig";
import type { ExportReport } from "./types/ExportReport";
import type { SilenceParams } from "./types/SilenceParams";
import type { RegionSpan } from "./types/RegionSpan";
import type { RegionEdge } from "./types/RegionEdge";

export const api = {
  loadAudio: (paths: string[]) => invoke<ProjectView>("load_audio", { paths }),
  loadMultitrack: (paths: string[]) =>
    invoke<ProjectView>("load_multitrack", { paths }),
  addClips: (paths: string[]) => invoke<ProjectView>("add_clips", { paths }),
  addLayers: (paths: string[]) => invoke<ProjectView>("add_layers", { paths }),
  setLayerGain: (id: number, gainDb: number) =>
    invoke<ProjectView>("set_layer_gain", { id, gainDb }),
  setLayerMuted: (id: number, muted: boolean) =>
    invoke<ProjectView>("set_layer_muted", { id, muted }),
  setLayerSolo: (id: number, solo: boolean) =>
    invoke<ProjectView>("set_layer_solo", { id, solo }),
  removeLayer: (id: number) => invoke<ProjectView>("remove_layer", { id }),
  setLayerCollapsed: (id: number, collapsed: boolean) =>
    invoke<ProjectView>("set_layer_collapsed", { id, collapsed }),
  setTrackLayerGain: (trackId: number, layerId: number, gainDb: number | null) =>
    invoke<ProjectView>("set_track_layer_gain", { trackId, layerId, gainDb }),
  setTrackLayerMute: (trackId: number, layerId: number, muted: boolean | null) =>
    invoke<ProjectView>("set_track_layer_mute", { trackId, layerId, muted }),
  setTrackLayerSolo: (trackId: number, layerId: number, solo: boolean | null) =>
    invoke<ProjectView>("set_track_layer_solo", { trackId, layerId, solo }),
  loadProject: (path: string) => invoke<ProjectView>("load_project", { path }),
  cancelLoad: () => invoke<void>("cancel_load"),
  saveProject: (path?: string) =>
    invoke<ProjectView>("save_project", { path: path ?? null }),
  getProjectView: () => invoke<ProjectView>("get_project_view"),
  getPeaks: (startSample: number, endSample: number, maxBuckets: number) =>
    invoke<PeakSlice>("get_peaks", { startSample, endSample, maxBuckets }),
  getPeaksSplit: (startSample: number, endSample: number, maxBuckets: number) =>
    invoke<PeakSlice[]>("get_peaks_split", { startSample, endSample, maxBuckets }),
  addRegion: (start: number, end: number, title?: string) =>
    invoke<ProjectView>("add_region", {
      start: Math.round(start),
      end: Math.round(end),
      title: title?.trim() ? title.trim() : null,
    }),
  addRegions: (regions: RegionSpan[]) =>
    invoke<ProjectView>("add_regions", {
      regions: regions.map((r) => ({
        start: Math.round(r.start),
        end: Math.round(r.end),
      })),
    }),
  moveRegionEdge: (id: number, edge: RegionEdge, position: number) =>
    invoke<ProjectView>("move_region_edge", {
      id,
      edge,
      position: Math.round(position),
    }),
  removeRegion: (id: number) => invoke<ProjectView>("remove_region", { id }),
  renameTrack: (id: number, title: string) =>
    invoke<ProjectView>("rename_track", { id, title }),
  setSnapToZero: (enabled: boolean) =>
    invoke<ProjectView>("set_snap_to_zero", { enabled }),
  setExportConfig: (config: ExportConfig) =>
    invoke<ProjectView>("set_export_config", { config }),
  undo: () => invoke<ProjectView>("undo"),
  redo: () => invoke<ProjectView>("redo"),
  detectSilences: (params: SilenceParams) =>
    invoke<RegionSpan[]>("detect_silences", { params }),
  exportTracks: (config: ExportConfig) =>
    invoke<ExportReport>("export_tracks", { config }),
  cancelExport: () => invoke<void>("cancel_export"),
  playerToggle: () => invoke<PlaybackState>("player_toggle"),
  playerPause: () => invoke<PlaybackState>("player_pause"),
  playerSeek: (positionSamples: number) =>
    invoke<PlaybackState>("player_seek", {
      positionSamples: Math.max(0, Math.round(positionSamples)),
    }),
  playerState: () => invoke<PlaybackState>("player_state"),
  getDefaultExportDir: () => invoke<string>("get_default_export_dir"),
};
