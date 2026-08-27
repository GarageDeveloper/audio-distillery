// Typed wrappers around the Tauri commands. The frontend only ever sends
// intentions and displays what the backend returns (ARCHITECTURE.md §3).
import { invoke } from "@tauri-apps/api/core";
import type { ProjectView } from "./types/ProjectView";
import type { PeakSlice } from "./types/PeakSlice";
import type { PlaybackState } from "./types/PlaybackState";
import type { ExportConfig } from "./types/ExportConfig";
import type { ExportReport } from "./types/ExportReport";
import type { SilenceParams } from "./types/SilenceParams";
import type { AlbumMeta } from "./types/AlbumMeta";
import type { PluginInfo } from "./types/PluginInfo";
import type { ChainPresetInfo } from "./types/ChainPresetInfo";
import type { ChainTarget } from "./types/ChainTarget";
import type { MeterState } from "./types/MeterState";
import type { RegionSpan } from "./types/RegionSpan";
import type { RegionEdge } from "./types/RegionEdge";

export const api = {
  loadAudio: (paths: string[]) => invoke<ProjectView>("load_audio", { paths }),
  loadMultitrack: (paths: string[]) =>
    invoke<ProjectView>("load_multitrack", { paths }),
  addClips: (paths: string[]) => invoke<ProjectView>("add_clips", { paths }),
  addLayers: (paths: string[]) => invoke<ProjectView>("add_layers", { paths }),
  addTake: (paths: string[]) => invoke<ProjectView>("add_take", { paths }),
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
  beginRegionEdit: () => invoke<void>("begin_region_edit"),
  moveRegionEdgePreview: (id: number, edge: RegionEdge, position: number) =>
    invoke<ProjectView>("move_region_edge_preview", {
      id,
      edge,
      position: Math.round(position),
    }),
  moveRegionEdge: (id: number, edge: RegionEdge, position: number) =>
    invoke<ProjectView>("move_region_edge", {
      id,
      edge,
      position: Math.round(position),
    }),
  removeRegion: (id: number) => invoke<ProjectView>("remove_region", { id }),
  renameLayer: (id: number, name: string) =>
    invoke<ProjectView>("rename_layer", { id, name }),
  renameTrack: (id: number, title: string) =>
    invoke<ProjectView>("rename_track", { id, title }),
  setTrackIsrc: (id: number, isrc: string) =>
    invoke<ProjectView>("set_track_isrc", { id, isrc }),
  setSnapToZero: (enabled: boolean) =>
    invoke<ProjectView>("set_snap_to_zero", { enabled }),
  setExportConfig: (config: ExportConfig) =>
    invoke<ProjectView>("set_export_config", { config }),
  setAlbumMeta: (meta: AlbumMeta) =>
    invoke<ProjectView>("set_album_meta", { meta }),
  getArtworkPreview: () => invoke<string | null>("get_artwork_preview"),
  listPlugins: () => invoke<PluginInfo[]>("list_plugins"),
  getVst3ScanPaths: () => invoke<string[]>("get_vst3_scan_paths"),
  setVst3ScanPaths: (paths: string[]) =>
    invoke<PluginInfo[]>("set_vst3_scan_paths", { paths }),
  addChainPlugin: (target: ChainTarget, component: string, name: string) =>
    invoke<ProjectView>("add_chain_plugin", { target, component, name }),
  removeChainPlugin: (id: number) =>
    invoke<ProjectView>("remove_chain_plugin", { id }),
  moveChainPlugin: (id: number, delta: number) =>
    invoke<ProjectView>("move_chain_plugin", { id, delta }),
  setChainBypass: (id: number, bypass: boolean) =>
    invoke<ProjectView>("set_chain_bypass", { id, bypass }),
  openPluginEditor: (id: number) => invoke<void>("open_plugin_editor", { id }),
  reloadChains: () => invoke<ProjectView>("reload_chains"),
  chainLatency: (target: ChainTarget) =>
    invoke<number>("chain_latency", { target }),
  meterState: () => invoke<MeterState>("meter_state"),
  resetMeter: () => invoke<void>("reset_meter"),
  saveChainPreset: (target: ChainTarget, name: string) =>
    invoke<ChainPresetInfo[]>("save_chain_preset", { target, name }),
  listChainPresets: () => invoke<ChainPresetInfo[]>("list_chain_presets"),
  loadChainPreset: (target: ChainTarget, name: string) =>
    invoke<ProjectView>("load_chain_preset", { target, name }),
  deleteChainPreset: (name: string) =>
    invoke<ChainPresetInfo[]>("delete_chain_preset", { name }),
  undo: () => invoke<ProjectView>("undo"),
  redo: () => invoke<ProjectView>("redo"),
  detectSilences: (params: SilenceParams, layerId: number | null) =>
    invoke<RegionSpan[]>("detect_silences", { params, layerId }),
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
