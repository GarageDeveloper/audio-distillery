//! AudioDistillery core ("still-core"): all business logic, with zero Tauri
//! dependency. The decisive test (SPEC §3): a CLI could replace the GUI
//! frontend entirely by calling into this crate.
//!
//! Non-negotiable rule (SPEC §3 bis): source audio files are opened
//! read-only, everywhere, always. Only exports create new files.

pub mod audio;
pub mod error;
pub mod export;
pub mod ffmpeg;
pub mod naming;
pub mod peaks;
pub mod playback;
pub mod project;
pub mod silence;

pub use audio::{
    clip_segments, scan_file, scan_files, snap_to_zero_crossing, AudioInfo, ClipInfo,
    SUPPORTED_EXTENSIONS,
};
pub use error::{Result, StillError};
pub use export::{
    export_concurrency, plan_export, run_export, ExportProgress, ExportReport, ExportedFile,
};
pub use ffmpeg::resolve_ffmpeg;
pub use peaks::{PeakPyramid, PeakSlice};
pub use playback::{PlaybackState, PlayerHandle};
pub use project::{
    read_project, sanitize_regions, save_project, ExportConfig, ExportFormat, Project,
    ProjectState, ProjectView, RegionEdge, RegionSpan, TrackInfo,
};
pub use silence::{detect_track_regions, SilenceParams};
