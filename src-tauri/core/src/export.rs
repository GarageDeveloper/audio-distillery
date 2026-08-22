use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::audio::{layer_parts, ClipInfo, TimelinePart};
use crate::engine::decode::{LayerDecoder, PlayItem};
use crate::engine::render::{Renderer, BLOCK_FRAMES};
use crate::engine::{MasterPluginSpec, VolumeAutomation};
use crate::error::{Result, StillError};
use crate::naming::render_track_filename;
use crate::metadata::{
    is_empty_meta, load_artwork, resolve_tags, write_tags, AlbumMeta, TrackTags,
};
use std::sync::Arc;
use crate::project::{ExportConfig, ExportFormat, TrackInfo};

/// What the mixer needs to know about one layer at export time. Volumes are
/// resolved PER JOB (`ExportJob::layer_volumes`), not here.
#[derive(Debug, Clone)]
pub struct LayerMix {
    pub clips: Vec<ClipInfo>,
}

/// One file to produce. `out_path` is already unique (collisions resolved by
/// suffixing — existing files are never overwritten, ARCHITECTURE.md §3 bis).
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
    /// This track's own insert chain (master-bus position, active for the
    /// whole job). Filled by the caller AFTER planning (states snapshotted).
    pub track_chain: Vec<MasterPluginSpec>,
    /// Tags to write into the finished file (None = tagging disabled).
    pub tags: Option<TrackTags>,
    /// Validated cover image shared by all jobs.
    pub artwork: Option<Arc<(Vec<u8>, lofty::picture::MimeType)>>,
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

    // Load and validate the cover once (fail the plan early with a clear
    // message rather than per-track during the export).
    let artwork = if meta.artwork_path.is_empty() {
        None
    } else {
        Some(Arc::new(load_artwork(Path::new(&meta.artwork_path))?))
    };
    let mut used: Vec<PathBuf> = Vec::new();
    let mut jobs = Vec::with_capacity(tracks.len());
    for t in tracks {
        // File naming supports the same macros as the metadata fields.
        // `/` in the TEMPLATE creates subfolders (e.g. "{disc}/{n} - {title}"
        // sorts a multi-disc album into one folder per disc); the template is
        // split BEFORE values are injected, so a title containing a slash can
        // never create a directory — each segment is sanitized on its own.
        let numbering = crate::metadata::disc_numbering(
            &meta.disc_breaks,
            t.number,
            tracks.len() as u32,
        );
        let ctx = crate::metadata::MacroContext {
            meta,
            title: &t.title,
            source_stem: &source_stem,
            numbering,
        };
        let segments: Vec<String> = cfg
            .template
            .split('/')
            .map(|seg| {
                render_track_filename(
                    &crate::metadata::expand_macros(seg, &ctx),
                    t.number as usize,
                    tracks.len(),
                    &t.title,
                    &source_stem,
                )
            })
            .filter(|s| !s.is_empty())
            .collect();
        let (base, subdirs) = segments
            .split_last()
            .map(|(b, d)| (b.clone(), d.to_vec()))
            .unwrap_or_else(|| ("Untitled".to_string(), Vec::new()));
        let parent = subdirs.iter().fold(dest.clone(), |acc, d| acc.join(d));
        std::fs::create_dir_all(&parent)?;
        let mut candidate = parent.join(format!("{base}.{ext}"));
        let mut k = 1;
        while candidate.exists() || used.contains(&candidate) {
            candidate = parent.join(format!("{base} ({k}).{ext}"));
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
            track_chain: Vec::new(),
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
            artwork: artwork.clone(),
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
    run_export_with_chain(
        ffmpeg,
        layers,
        session_channels,
        sample_rate,
        jobs,
        cfg,
        &[],
        &[],
        cancel,
        on_progress,
    )
}

/// Like [`run_export`], rendering each track through the plugin chains
/// when any is given: the engine's own Renderer produces the processed PCM
/// (each worker instantiates its own plugin instances from the specs, with
/// the live states snapshotted by the caller), which is piped to ffmpeg for
/// encoding only. `lane_chains` is index-aligned with `layers` (pre-fader,
/// per layer); each job may carry its own `track_chain`, processed before
/// the master `chain`. With no chains anywhere the pure-ffmpeg graph path
/// is kept.
#[allow(clippy::too_many_arguments)]
pub fn run_export_with_chain(
    ffmpeg: &Path,
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    jobs: &[ExportJob],
    cfg: &ExportConfig,
    chain: &[MasterPluginSpec],
    lane_chains: &[Vec<MasterPluginSpec>],
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
                let needs_render = !chain.is_empty()
                    || !job.track_chain.is_empty()
                    || lane_chains.iter().any(|c| !c.is_empty());
                let result = if !needs_render {
                    export_one(
                        ffmpeg,
                        layers,
                        session_channels,
                        sample_rate,
                        job,
                        cfg,
                        cancel,
                        |p| emit(i, p),
                    )
                } else {
                    export_one_rendered(
                        ffmpeg,
                        layers,
                        session_channels,
                        sample_rate,
                        job,
                        cfg,
                        chain,
                        lane_chains,
                        cancel,
                        |p| emit(i, p),
                    )
                };
                match result {
                    Ok(()) => {
                        // Tag the NEW file (never a source). A tagging
                        // failure keeps the audio and surfaces as an error.
                        if let Some(tags) = &job.tags {
                            if let Err(e) =
                                write_tags(&job.out_path, tags, job.artwork.as_deref())
                            {
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

/// Render one track OFFLINE through the mastering chain and pipe the PCM
/// to ffmpeg for encoding. Latency-compensated: the chain's total reported
/// latency is rendered ahead and dropped, so the output stays sample-exact.
#[allow(clippy::too_many_arguments)]
fn export_one_rendered(
    ffmpeg: &Path,
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    job: &ExportJob,
    cfg: &ExportConfig,
    chain: &[MasterPluginSpec],
    lane_chains: &[Vec<MasterPluginSpec>],
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(f32),
) -> Result<()> {
    let channels = (session_channels.max(1) as usize).min(2);

    // Full-timeline playlists per layer; the decoder seek positions us at
    // the region start sample-accurately. Muted layers are dropped, which
    // BREAKS the layer-index ↔ lane-index correspondence — `kept` records
    // each surviving lane's ORIGINAL layer index (lane chains follow it).
    let kept: Vec<usize> = (0..layers.len())
        .filter(|i| job.layer_volumes.get(*i).copied().unwrap_or(1.0) > 0.0)
        .collect();
    let decoders: Vec<LayerDecoder> = layers
        .iter()
        .enumerate()
        .filter(|(i, _)| kept.contains(i))
        .map(|(_, l)| {
            let mut items = Vec::new();
            let mut cursor = 0u64;
            for c in &l.clips {
                if c.start_sample > cursor {
                    items.push(PlayItem::Silence {
                        samples: c.start_sample - cursor,
                    });
                }
                items.push(PlayItem::File {
                    path: PathBuf::from(&c.path),
                    samples: c.duration_samples,
                });
                cursor = c.start_sample + c.duration_samples;
            }
            LayerDecoder::new(items, channels)
        })
        .collect();
    let volumes: Vec<f32> = job
        .layer_volumes
        .iter()
        .copied()
        .filter(|v| *v > 0.0)
        .collect();
    if decoders.is_empty() {
        return Err(StillError::Ffmpeg(
            "the track region covers no audible audio (are all layers muted?)".into(),
        ));
    }

    // Instantiate THIS worker's own plugin instances from the specs (any
    // format — the factory dispatches on the component id).
    let instantiate = |specs: &[MasterPluginSpec]| -> Result<Vec<Box<dyn crate::engine::render::BlockProcessor>>> {
        let mut out = Vec::with_capacity(specs.len());
        for spec in specs {
            let mut p = crate::plugins::create_plugin(
                &spec.component,
                sample_rate,
                channels,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            )
            .map_err(|e| StillError::Ffmpeg(format!("{}: {e}", spec.component)))?;
            if let Some(state) = &spec.state {
                let _ = p.restore_state(state);
            }
            p.set_bypassed(spec.bypass);
            out.push(p);
        }
        Ok(out)
    };
    // Master bus for this job: the track's own chain (active the whole
    // job), then the global mastering chain — same order as playback.
    let mut inserts = instantiate(&job.track_chain)?;
    inserts.extend(instantiate(chain)?);
    // Latency compensation covers the master bus (track + mastering).
    // Lane-chain latency is NOT compensated (same inter-layer skew as
    // playback; typical layer inserts are zero-latency).
    let latency: u64 = inserts.iter().map(|p| p.latency_samples() as u64).sum();

    let track_len = job.end_sample.saturating_sub(job.start_sample);
    let mut renderer = Renderer::new(
        decoders,
        VolumeAutomation {
            default: volumes,
            spans: Vec::new(),
        },
        sample_rate,
        channels,
        job.end_sample + latency,
    );
    renderer.master_sections = vec![crate::engine::render::InsertSection::new(None, inserts)];
    for (lane_idx, orig_idx) in kept.iter().enumerate() {
        if let Some(specs) = lane_chains.get(*orig_idx) {
            if !specs.is_empty() {
                renderer.lanes[lane_idx].inserts = instantiate(specs)?;
            }
        }
    }
    renderer.seek(job.start_sample);

    // ffmpeg encodes raw f32le PCM from stdin.
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-f")
        .arg("f32le")
        .arg("-ar")
        .arg(sample_rate.to_string())
        .arg("-ac")
        .arg(channels.to_string())
        .arg("-i")
        .arg("pipe:0")
        .args(codec_args(cfg))
        .arg(&job.out_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            StillError::FfmpegNotFound
        } else {
            StillError::Ffmpeg(e.to_string())
        }
    })?;
    let mut stdin = child.stdin.take().expect("stdin piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        use std::io::Read as _;
        let _ = stderr.read_to_string(&mut buf);
        buf
    });

    let mut block = vec![0.0f32; BLOCK_FRAMES * channels];
    let mut to_skip = latency; // drop the chain's latency head
    let mut written = 0u64;
    let mut failed_write = false;
    'render: while written < track_len {
        if cancel.load(Ordering::SeqCst) {
            break 'render;
        }
        let got = renderer.render_block(&mut block, BLOCK_FRAMES);
        if got == 0 {
            break;
        }
        let mut offset = 0usize;
        if to_skip > 0 {
            let drop_n = (to_skip.min(got as u64)) as usize;
            offset = drop_n;
            to_skip -= drop_n as u64;
        }
        let usable = ((got - offset) as u64).min(track_len - written) as usize;
        if usable > 0 {
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    block[offset * channels..].as_ptr() as *const u8,
                    usable * channels * 4,
                )
            };
            use std::io::Write as _;
            if stdin.write_all(bytes).is_err() {
                failed_write = true;
                break 'render;
            }
            written += usable as u64;
            if written % (sample_rate as u64 / 2).max(1) < BLOCK_FRAMES as u64 {
                on_progress((written as f32 / track_len.max(1) as f32).min(1.0));
            }
        }
    }
    drop(stdin); // EOF → ffmpeg finalizes the file
    let status = child.wait().map_err(|e| StillError::Ffmpeg(e.to_string()))?;
    let err_output = stderr_reader.join().unwrap_or_default();
    if cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&job.out_path);
        return Err(StillError::Ffmpeg("cancelled".into()));
    }
    if !status.success() || failed_write {
        let msg = err_output.trim();
        return Err(StillError::Ffmpeg(if msg.is_empty() {
            format!("ffmpeg exited with {status}")
        } else {
            msg.to_string()
        }));
    }
    on_progress(1.0);
    Ok(())
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
            inserts: Vec::new(),
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
    fn multi_disc_template_builds_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ExportConfig {
            template: "{disc}/{n} - {title}".into(),
            dest_dir: dir.path().display().to_string(),
            ..Default::default()
        };
        let meta = AlbumMeta {
            disc_breaks: vec![2],
            album: "X".into(),
            ..Default::default()
        };
        let tracks = vec![
            track(1, "One", 0, 44_100),
            track(2, "Two", 44_100, 88_200),
        ];
        let jobs =
            plan_export_with_meta(&tracks, &cfg, Path::new("/x/source.wav"), &meta).unwrap();
        assert!(jobs[0].out_path.ends_with("1/01 - One.flac"), "{:?}", jobs[0].out_path);
        assert!(jobs[1].out_path.ends_with("2/01 - Two.flac"), "{:?}", jobs[1].out_path);
        assert!(jobs[0].out_path.parent().unwrap().is_dir());
        // A slash in a TITLE must not create a folder.
        let tracks2 = vec![track(1, "AC/DC Cover", 0, 44_100)];
        let jobs2 =
            plan_export_with_meta(&tracks2, &cfg, Path::new("/x/source.wav"), &meta).unwrap();
        assert!(
            jobs2[0].out_path.ends_with("1/01 - AC_DC Cover.flac"),
            "{:?}",
            jobs2[0].out_path
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
