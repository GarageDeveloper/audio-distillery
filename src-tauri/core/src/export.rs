use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::audio::{layer_parts, ClipInfo, TimelinePart};
use crate::error::{Result, StillError};
use crate::naming::render_track_filename;
use crate::metadata::{is_empty_meta, resolve_tags, write_tags, AlbumMeta, TrackTags};
use crate::project::{ExportConfig, ExportFormat, TrackInfo};

/// What the mixer needs to know about one layer at export time. Volumes are
/// resolved PER JOB (`ExportJob::layer_volumes`), not here.
#[derive(Debug, Clone)]
pub struct LayerMix {
    pub clips: Vec<ClipInfo>,
}

/// One file to produce. `out_path` is already unique (collisions resolved by
/// suffixing — existing files are never overwritten, SPEC §3 bis).
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub number: u32,
    pub title: String,
    pub start_sample: u64,
    pub end_sample: u64,
    pub out_path: PathBuf,
    /// Resolved linear volume per layer for THIS track (gains, mutes and
    /// solos, session-wide or overridden — straight from TrackInfo).
    pub layer_volumes: Vec<f32>,
    /// Tags to write into the finished file (None = tagging disabled).
    pub tags: Option<TrackTags>,
}

/// Progress event for ONE track. Exports run in parallel, so several tracks
/// progress at once; the display layer keeps one bar per track number and
/// uses `overall_progress`/`completed_tracks` for the global state.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ExportProgress {
    /// Track number this event is about (1-based).
    pub track_number: u32,
    pub track_count: u32,
    pub track_title: String,
    /// 0.0..=1.0 within this track.
    pub track_progress: f32,
    /// Tracks fully finished so far.
    pub completed_tracks: u32,
    /// 0.0..=1.0 across the whole export.
    pub overall_progress: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ExportedFile {
    pub track_title: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ExportReport {
    pub files: Vec<ExportedFile>,
    pub errors: Vec<String>,
    pub cancelled: bool,
}

/// Compute the list of output files: rendered names, resolved collisions.
pub fn plan_export(
    tracks: &[TrackInfo],
    cfg: &ExportConfig,
    source_path: &Path,
) -> Result<Vec<ExportJob>> {
    plan_export_with_meta(tracks, cfg, source_path, &AlbumMeta::default())
}

/// Like [`plan_export`], resolving each track's tags from the album
/// metadata (macros expanded, disc numbering computed).
pub fn plan_export_with_meta(
    tracks: &[TrackInfo],
    cfg: &ExportConfig,
    source_path: &Path,
    meta: &AlbumMeta,
) -> Result<Vec<ExportJob>> {
    if cfg.dest_dir.trim().is_empty() {
        return Err(StillError::InvalidProject(
            "no destination folder selected".into(),
        ));
    }
    let dest = PathBuf::from(&cfg.dest_dir);
    std::fs::create_dir_all(&dest)?;
    let source_stem = source_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio".into());
    let ext = cfg.format.extension();

    let mut used: Vec<PathBuf> = Vec::new();
    let mut jobs = Vec::with_capacity(tracks.len());
    for t in tracks {
        // File naming supports the same macros as the metadata fields
        // ({album}, {disc}, {year}…); classic tokens keep working through
        // render_track_filename's sanitizing pass.
        let numbering = crate::metadata::disc_numbering(
            &meta.disc_breaks,
            t.number,
            tracks.len() as u32,
        );
        let expanded = crate::metadata::expand_macros(
            &cfg.template,
            &crate::metadata::MacroContext {
                meta,
                title: &t.title,
                source_stem: &source_stem,
                numbering,
            },
        );
        let base = render_track_filename(
            &expanded,
            t.number as usize,
            tracks.len(),
            &t.title,
            &source_stem,
        );
        let mut candidate = dest.join(format!("{base}.{ext}"));
        let mut k = 1;
        while candidate.exists() || used.contains(&candidate) {
            candidate = dest.join(format!("{base} ({k}).{ext}"));
            k += 1;
        }
        used.push(candidate.clone());
        jobs.push(ExportJob {
            number: t.number,
            title: t.title.clone(),
            start_sample: t.start_sample,
            end_sample: t.end_sample,
            out_path: candidate,
            layer_volumes: t.layer_volumes.clone(),
            tags: if is_empty_meta(meta) {
                None
            } else {
                Some(resolve_tags(
                    meta,
                    &t.title,
                    &source_stem,
                    t.number,
                    tracks.len() as u32,
                ))
            },
        });
    }
    Ok(jobs)
}

fn codec_args(cfg: &ExportConfig) -> Vec<String> {
    match cfg.format {
        ExportFormat::Wav => vec![
            "-c:a".into(),
            if cfg.bit_depth == 24 {
                "pcm_s24le".into()
            } else {
                "pcm_s16le".into()
            },
        ],
        ExportFormat::Flac => vec!["-c:a".into(), "flac".into()],
        ExportFormat::Mp3 => vec![
            "-c:a".into(),
            "libmp3lame".into(),
            "-b:a".into(),
            format!("{}k", cfg.bitrate_kbps),
        ],
        ExportFormat::Aac => vec![
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            format!("{}k", cfg.bitrate_kbps),
        ],
    }
}

/// How many ffmpeg jobs to run at once: use the available cores but always
/// leave two of them free so the machine stays responsive (audio encoders are
/// essentially single-threaded, so one core per job is the right model).
/// Overridable with the `STILL_EXPORT_JOBS` env var.
pub fn export_concurrency(job_count: usize, available_cores: usize) -> usize {
    if let Some(n) = std::env::var("STILL_EXPORT_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        return n.min(job_count.max(1));
    }
    available_cores.saturating_sub(2).max(1).min(job_count.max(1))
}

/// Cut and encode every track with ffmpeg (sample-accurate via `atrim`),
/// running jobs in parallel across the available cores. The source is only
/// ever read; each job writes a brand-new file.
pub fn run_export(
    ffmpeg: &Path,
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    jobs: &[ExportJob],
    cfg: &ExportConfig,
    cancel: &AtomicBool,
    on_progress: impl Fn(ExportProgress) + Send + Sync,
) -> ExportReport {
    let count = jobs.len() as u32;
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let workers = export_concurrency(jobs.len(), cores);

    // Shared per-job progress (f32 bits) for the overall percentage, and a
    // work-stealing index so idle workers pick up the next pending job.
    let progress: Vec<AtomicU32> = (0..jobs.len()).map(|_| AtomicU32::new(0)).collect();
    let completed = AtomicU32::new(0);
    let next_job = AtomicUsize::new(0);
    let files: Mutex<Vec<(usize, ExportedFile)>> = Mutex::new(Vec::new());
    let errors: Mutex<Vec<(usize, String)>> = Mutex::new(Vec::new());

    let overall = |progress: &[AtomicU32]| -> f32 {
        if progress.is_empty() {
            return 1.0;
        }
        let sum: f32 = progress
            .iter()
            .map(|p| f32::from_bits(p.load(Ordering::Relaxed)))
            .sum();
        sum / progress.len() as f32
    };
    let emit = |i: usize, p: f32| {
        progress[i].store(p.to_bits(), Ordering::Relaxed);
        on_progress(ExportProgress {
            track_number: jobs[i].number,
            track_count: count,
            track_title: jobs[i].title.clone(),
            track_progress: p,
            completed_tracks: completed.load(Ordering::Relaxed),
            overall_progress: overall(&progress),
        });
    };

    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next_job.fetch_add(1, Ordering::SeqCst);
                if i >= jobs.len() || cancel.load(Ordering::SeqCst) {
                    break;
                }
                let job = &jobs[i];
                emit(i, 0.0);
                match export_one(
                    ffmpeg,
                    layers,
                    session_channels,
                    sample_rate,
                    job,
                    cfg,
                    cancel,
                    |p| emit(i, p),
                ) {
                    Ok(()) => {
                        // Tag the NEW file (never a source). A tagging
                        // failure keeps the audio and surfaces as an error.
                        if let Some(tags) = &job.tags {
                            if let Err(e) = write_tags(&job.out_path, tags) {
                                errors
                                    .lock()
                                    .unwrap()
                                    .push((i, format!("{}: {e}", job.out_path.display())));
                            }
                        }
                        files.lock().unwrap().push((
                            i,
                            ExportedFile {
                                track_title: job.title.clone(),
                                path: job.out_path.display().to_string(),
                            },
                        ));
                        completed.fetch_add(1, Ordering::Relaxed);
                        emit(i, 1.0);
                    }
                    Err(_) if cancel.load(Ordering::SeqCst) => {
                        let _ = std::fs::remove_file(&job.out_path);
                        break;
                    }
                    Err(e) => {
                        errors
                            .lock()
                            .unwrap()
                            .push((i, format!("{}: {e}", job.out_path.display())));
                        let _ = std::fs::remove_file(&job.out_path);
                    }
                }
            });
        }
    });

    // Report in track order, whatever order the workers finished in.
    let mut files = files.into_inner().unwrap();
    files.sort_by_key(|(i, _)| *i);
    let mut errors = errors.into_inner().unwrap();
    errors.sort_by_key(|(i, _)| *i);
    ExportReport {
        files: files.into_iter().map(|(_, f)| f).collect(),
        errors: errors.into_iter().map(|(_, e)| e).collect(),
        cancelled: cancel.load(Ordering::SeqCst),
    }
}

#[allow(clippy::too_many_arguments)]
fn export_one(
    ffmpeg: &Path,
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    job: &ExportJob,
    cfg: &ExportConfig,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<()> {
    // For every audible layer, map the timeline region onto the clip files it
    // covers; each layer is trimmed sample-accurately (concat across clip
    // boundaries), gain-adjusted, then all layers are summed with amix
    // (normalize=0 → plain weighted sum, exactly the mix the user dialed in).
    // atrim keeps original timestamps; asetpts resets them so tracks start
    // at t = 0 (otherwise players show leading silence).
    // Volumes were resolved per track by the core (gains, mutes, solos and
    // this track's overrides): silent layers are simply skipped.
    let active: Vec<(&LayerMix, f32, Vec<TimelinePart>)> = layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l, job.layer_volumes.get(i).copied().unwrap_or(1.0)))
        .filter(|(_, vol)| *vol > 0.0)
        .map(|(l, vol)| (l, vol, layer_parts(&l.clips, job.start_sample, job.end_sample)))
        .filter(|(_, _, parts)| {
            parts
                .iter()
                .any(|p| matches!(p, TimelinePart::File { .. }))
        })
        .collect();
    if active.is_empty() {
        return Err(StillError::Ffmpeg(
            "the track region covers no audible audio (are all layers muted?)".into(),
        ));
    }

    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-nostdin")
        .arg("-v")
        .arg("error")
        .arg("-progress")
        .arg("pipe:1");
    for (layer, _, parts) in &active {
        for part in parts {
            if let TimelinePart::File { clip, .. } = part {
                cmd.arg("-i").arg(&layer.clips[*clip].path);
            }
        }
    }

    let single_plain = active.len() == 1
        && active[0].2.len() == 1
        && matches!(active[0].2[0], TimelinePart::File { .. })
        && (active[0].1 - 1.0).abs() < 1e-6;
    if single_plain {
        let TimelinePart::File { start: s, end: e, .. } = active[0].2[0] else {
            unreachable!()
        };
        cmd.arg("-map").arg("0:a:0").arg("-af").arg(format!(
            "atrim=start_sample={s}:end_sample={e},asetpts=PTS-STARTPTS"
        ));
    } else {
        let layout = if session_channels >= 2 { "stereo" } else { "mono" };
        let mut filter = String::new();
        let mut input_idx = 0usize;
        let mut layer_labels: Vec<String> = Vec::new();
        for (li, (_, volume, parts)) in active.iter().enumerate() {
            let mut seg_labels: Vec<String> = Vec::new();
            for (k, part) in parts.iter().enumerate() {
                let label = format!("t{li}x{k}");
                match part {
                    TimelinePart::File { start: s, end: e, .. } => {
                        filter.push_str(&format!(
                            "[{input_idx}:a:0]atrim=start_sample={s}:end_sample={e},asetpts=PTS-STARTPTS[{label}];"
                        ));
                        input_idx += 1;
                    }
                    TimelinePart::Silence { samples } => {
                        // Sample-exact silence keeps this layer aligned with
                        // the others across take gaps.
                        filter.push_str(&format!(
                            "anullsrc=r={sample_rate}:cl={layout},atrim=end_sample={samples},asetpts=PTS-STARTPTS[{label}];"
                        ));
                    }
                }
                seg_labels.push(label);
            }
            let joined = if seg_labels.len() > 1 {
                let label = format!("c{li}");
                for l in &seg_labels {
                    filter.push_str(&format!("[{l}]"));
                }
                filter.push_str(&format!(
                    "concat=n={}:v=0:a=1[{label}];",
                    seg_labels.len()
                ));
                label
            } else {
                seg_labels.remove(0)
            };
            let out = format!("l{li}");
            filter.push_str(&format!(
                "[{joined}]aformat=sample_fmts=fltp:channel_layouts={layout},volume={volume:.6}[{out}];"
            ));
            layer_labels.push(out);
        }
        let final_label = if layer_labels.len() > 1 {
            for l in &layer_labels {
                filter.push_str(&format!("[{l}]"));
            }
            filter.push_str(&format!(
                "amix=inputs={}:normalize=0[mix];",
                layer_labels.len()
            ));
            "mix".to_string()
        } else {
            layer_labels.remove(0)
        };
        // Drop the trailing semicolon.
        filter.pop();
        cmd.arg("-filter_complex")
            .arg(&filter)
            .arg("-map")
            .arg(format!("[{final_label}]"));
    }
    cmd.args(codec_args(cfg))
        .arg(&job.out_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            StillError::FfmpegNotFound
        } else {
            StillError::Ffmpeg(e.to_string())
        }
    })?;

    let track_secs =
        (job.end_sample.saturating_sub(job.start_sample)) as f64 / sample_rate as f64;
    let stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf);
        buf
    });

    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            break;
        }
        // ffmpeg -progress emits `out_time_us=...` (µs); `out_time_ms` is
        // also µs due to a long-standing ffmpeg quirk.
        if let Some(v) = line
            .strip_prefix("out_time_us=")
            .or_else(|| line.strip_prefix("out_time_ms="))
        {
            if let Ok(us) = v.trim().parse::<i64>() {
                if track_secs > 0.0 && us >= 0 {
                    let p = ((us as f64 / 1_000_000.0) / track_secs).min(1.0);
                    on_progress(p as f32);
                }
            }
        }
    }

    let status = child
        .wait()
        .map_err(|e| StillError::Ffmpeg(e.to_string()))?;
    let err_output = stderr_reader.join().unwrap_or_default();
    if cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&job.out_path);
        return Err(StillError::Ffmpeg("cancelled".into()));
    }
    if !status.success() {
        let msg = err_output.trim();
        return Err(StillError::Ffmpeg(if msg.is_empty() {
            format!("ffmpeg exited with {status}")
        } else {
            msg.to_string()
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn track(number: u32, title: &str, start: u64, end: u64) -> TrackInfo {
        TrackInfo {
            id: number,
            number,
            title: title.into(),
            start_sample: start,
            end_sample: end,
            duration_seconds: (end - start) as f64 / 44_100.0,
            gain_overrides: HashMap::new(),
            mute_overrides: HashMap::new(),
            solo_overrides: HashMap::new(),
            layer_volumes: vec![1.0],
        }
    }

    #[test]
    fn plan_renders_names_and_resolves_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ExportConfig {
            dest_dir: dir.path().display().to_string(),
            ..Default::default()
        };
        // Two tracks with the same title → second gets a suffix.
        let tracks = vec![
            track(1, "Same", 0, 44_100),
            track(2, "Same", 44_100, 88_200),
        ];
        let cfg2 = ExportConfig {
            template: "{title}".into(),
            ..cfg
        };
        let jobs = plan_export(&tracks, &cfg2, Path::new("/x/source.wav")).unwrap();
        assert_eq!(jobs[0].out_path.file_name().unwrap(), "Same.flac");
        assert_eq!(jobs[1].out_path.file_name().unwrap(), "Same (1).flac");
    }

    #[test]
    fn plan_avoids_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("01 - Intro.flac"), b"x").unwrap();
        let cfg = ExportConfig {
            dest_dir: dir.path().display().to_string(),
            ..Default::default()
        };
        let tracks = vec![track(1, "Intro", 0, 44_100)];
        let jobs = plan_export(&tracks, &cfg, Path::new("/x/source.wav")).unwrap();
        assert_eq!(
            jobs[0].out_path.file_name().unwrap(),
            "01 - Intro (1).flac"
        );
    }

    #[test]
    fn plan_requires_destination() {
        let cfg = ExportConfig::default();
        let tracks = vec![track(1, "Intro", 0, 44_100)];
        assert!(plan_export(&tracks, &cfg, Path::new("/x/s.wav")).is_err());
    }

    #[test]
    fn concurrency_leaves_two_cores_free() {
        // Guard against an env override leaking into the test run.
        std::env::remove_var("STILL_EXPORT_JOBS");
        assert_eq!(export_concurrency(100, 10), 8);
        assert_eq!(export_concurrency(100, 4), 2);
        // Never below one worker, even on tiny machines.
        assert_eq!(export_concurrency(100, 2), 1);
        assert_eq!(export_concurrency(100, 1), 1);
        // Never more workers than jobs.
        assert_eq!(export_concurrency(3, 16), 3);
        assert_eq!(export_concurrency(0, 16), 1);
    }

    #[test]
    fn codec_args_match_format() {
        let mut cfg = ExportConfig::default();
        cfg.format = ExportFormat::Mp3;
        cfg.bitrate_kbps = 192;
        assert!(codec_args(&cfg).contains(&"192k".to_string()));
        cfg.format = ExportFormat::Wav;
        cfg.bit_depth = 24;
        assert!(codec_args(&cfg).contains(&"pcm_s24le".to_string()));
    }
}
