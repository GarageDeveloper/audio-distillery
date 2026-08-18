use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, StillError};

/// Locate an ffmpeg binary. Order: `STILL_FFMPEG` env var, explicit candidates
/// (e.g. a bundled sidecar provided by the app layer), then well-known
/// locations and the PATH. GUI apps on macOS don't inherit the shell PATH,
/// hence the explicit Homebrew/MacPorts paths.
pub fn resolve_ffmpeg(extra_candidates: &[PathBuf]) -> Result<PathBuf> {
    if let Ok(p) = std::env::var("STILL_FFMPEG") {
        let p = PathBuf::from(p);
        if is_runnable(&p) {
            return Ok(p);
        }
    }
    for c in extra_candidates {
        if is_runnable(c) {
            return Ok(c.clone());
        }
    }
    let known = [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/opt/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ];
    for c in known {
        let p = PathBuf::from(c);
        if is_runnable(&p) {
            return Ok(p);
        }
    }
    // Finally, whatever "ffmpeg" resolves to on the PATH.
    let p = PathBuf::from("ffmpeg");
    if is_runnable(&p) {
        return Ok(p);
    }
    Err(StillError::FfmpegNotFound)
}

fn is_runnable(path: &Path) -> bool {
    Command::new(path)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
