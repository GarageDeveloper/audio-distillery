use thiserror::Error;

/// All user-facing errors. Messages are actionable and in English (product
/// default language; localization comes later).
#[derive(Debug, Error)]
pub enum StillError {
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("Unsupported audio format: {0}")]
    UnsupportedFormat(String),
    #[error("Could not decode audio: {0}")]
    Decode(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("FFmpeg was not found. Install it (e.g. `brew install ffmpeg` on macOS, `apt install ffmpeg` on Linux, `winget install ffmpeg` on Windows) or set the STILL_FFMPEG environment variable to the binary path.")]
    FfmpegNotFound,
    #[error("FFmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("Invalid marker: {0}")]
    InvalidMarker(String),
    #[error("No audio file is loaded")]
    NoAudioLoaded,
    #[error("Invalid project file: {0}")]
    InvalidProject(String),
    #[error("An export is already running")]
    ExportAlreadyRunning,
    #[error("Import cancelled")]
    ScanCancelled,
    #[error("Audio playback error: {0}")]
    Playback(String),
}

pub type Result<T> = std::result::Result<T, StillError>;
