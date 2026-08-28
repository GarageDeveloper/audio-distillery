//! AudioDistillery core ("still-core"): all business logic, with zero Tauri
//! dependency. The decisive test (ARCHITECTURE.md §3): a CLI could replace the GUI
//! frontend entirely by calling into this crate.
//!
//! Non-negotiable rule (ARCHITECTURE.md §3 bis): source audio files are opened
//! read-only, everywhere, always. Only exports create new files.

pub mod audio;
pub mod aunit;
pub mod b64;
pub mod chain_presets;
pub mod error;
pub mod export;
pub mod ffmpeg;
pub mod metadata;
pub mod naming;
pub mod engine;
pub mod peaks;
pub mod plugins;
pub mod project;
pub mod vst3;
pub mod silence;

pub use audio::{
    clip_segments, layer_parts, scan_file, scan_files, scan_layers, snap_to_zero_crossing,
    AudioInfo, ClipInfo, ScannedLayer, TimelinePart, SUPPORTED_EXTENSIONS,
};
pub use error::{Result, StillError};
pub use export::{
    export_concurrency, plan_export, run_export, ExportProgress, ExportReport, ExportedFile,
    LayerMix,
};
pub use ffmpeg::resolve_ffmpeg;
pub use metadata::{resolve_tags, write_tags, AlbumMeta, TrackTags};
pub use peaks::{merged_base_pyramid, merged_query, PeakPyramid, PeakSlice};
pub use chain_presets::{ChainPreset, ChainPresetInfo};
pub use plugins::{create_plugin, list_plugins, PluginFormat, PluginInfo};
pub use engine::record::{
    list_input_devices, mic_permission, request_mic_access, watch_input_devices,
    InputDeviceInfo, RecordConfig, RecordLane, RecordStatus, RecorderHandle,
};
pub use engine::{LayerPlay, MasterPluginSpec, MeterState, PlaybackState, PlayerHandle, VolumeAutomation};
pub use project::{
    db_to_linear, read_project, sanitize_regions, save_project, ChainTarget, ExportConfig,
    ExportFormat, Layer, LayerView, MasteringPluginCfg, MasteringPluginView, Project,
    ProjectState, ProjectView, RegionEdge, RegionSpan, SourceRef, TrackInfo,
};
pub use silence::{detect_track_regions, SilenceParams};
