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
    /// Concrete output parameters when the config asks for
    /// `ExportFormat::Source`: (format, bit depth, sample rate) probed from
    /// this job's reference source file at plan time. None = use the
    /// config as-is.
    pub source_fmt: Option<(ExportFormat, u8, Option<u32>)>,
    /// A window with no audible audio produces a full-length SILENT file
    /// instead of an error. Stems set this: a layer with no clips under a
    /// track must still yield a stem, or the set desynchronizes in a DAW.
    pub silence_ok: bool,
    /// Normalized ISRC of the track ("" = none) — cue sheets and DDP.
    pub isrc: String,
    /// Album gap rendered BEFORE this track in continuous deliverables
    /// (CD image, DDP), in Red Book sectors. Per-file exports ignore it.
    pub gap_before_sectors: u64,
}

impl ExportJob {
    /// The config this job actually encodes with (`Source` resolved).
    pub fn effective_cfg(&self, cfg: &ExportConfig) -> ExportConfig {
        match self.source_fmt {
            Some((format, bit_depth, target_sample_rate)) => ExportConfig {
                format,
                bit_depth,
                target_sample_rate,
                ..cfg.clone()
            },
            None => cfg.clone(),
        }
    }
}

/// One layer's identity for stems planning.
pub struct StemLayer {
    /// Display name: the custom layer name, or the source file's stem.
    pub name: String,
    /// First source file of the layer (format reference for Source mode).
    pub source_path: String,
}

/// Output parameters mirroring `path`'s container: format from the
/// extension (unknown → WAV), bit depth clamped to the 16/24 the encoders
/// accept, sample rate as probed. Lossy formats keep the configured
/// bitrate.
pub fn resolve_source_format(path: &Path) -> (ExportFormat, u8, Option<u32>) {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let format = match ext.as_str() {
        "flac" => ExportFormat::Flac,
        "mp3" => ExportFormat::Mp3,
        "m4a" | "aac" | "mp4" => ExportFormat::Aac,
        _ => ExportFormat::Wav,
    };
    let props = lofty::probe::Probe::open(path)
        .ok()
        .and_then(|p| p.read().ok())
        .map(|f| {
            use lofty::file::AudioFile;
            let p = f.properties();
            (p.bit_depth(), p.sample_rate())
        });
    let (depth, rate) = props.unwrap_or((None, None));
    let bit_depth = depth.map(|b| if b >= 24 { 24 } else { 16 }).unwrap_or(24);
    (format, bit_depth, rate)
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
    /// True during the post-encode loudness measurement pass: the counters
    /// then describe analysis steps, not encoded tracks.
    #[serde(default)]
    pub analyzing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ExportedFile {
    pub track_title: String,
    pub path: String,
    /// Integrated loudness of the DELIVERED file (EBU R128), measured by
    /// an analysis pass after encoding. None = analysis unavailable.
    #[serde(default)]
    pub lufs_i: Option<f64>,
    #[serde(default)]
    pub lra: Option<f64>,
    /// Max true peak (dBTP) of the delivered file.
    #[serde(default)]
    pub true_peak_db: Option<f64>,
    /// Per-track breakdown when this single file holds several tracks
    /// (CD image): each entry is measured on its cue segment.
    #[serde(default)]
    pub track_measures: Vec<TrackMeasure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct TrackMeasure {
    pub number: u32,
    pub title: String,
    pub lufs_i: Option<f64>,
    pub lra: Option<f64>,
    pub true_peak_db: Option<f64>,
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
    let src_fmt = (cfg.format == ExportFormat::Source)
        .then(|| resolve_source_format(source_path));
    let ext = src_fmt
        .map(|(f, _, _)| f.extension())
        .unwrap_or_else(|| cfg.format.extension());

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
            source_fmt: src_fmt,
            silence_ok: false,
            isrc: t.isrc.clone(),
            gap_before_sectors: (t.gap_before_effective_ms as u64 * 75 + 500) / 1000,
        });
    }
    Ok(jobs)
}

/// Plan a MULTITRACK export (stems): one job per (track × layer), each
/// rendering a single layer via one-hot volumes. `apply_mix` = false cuts
/// the layer raw (unity volume; callers pass no chains); true keeps the
/// resolved per-track layer volume and skips fully muted stems. Templates
/// gain the `{layer}` (name) and `{ln}` (layer index) macros.
pub fn plan_export_stems(
    tracks: &[TrackInfo],
    cfg: &ExportConfig,
    source_path: &Path,
    meta: &AlbumMeta,
    stem_layers: &[StemLayer],
    apply_mix: bool,
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
    let artwork = if meta.artwork_path.is_empty() {
        None
    } else {
        Some(Arc::new(load_artwork(Path::new(&meta.artwork_path))?))
    };
    let mut used: Vec<PathBuf> = Vec::new();
    let mut jobs = Vec::new();
    let mut seq = 0u32;
    for t in tracks {
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
        for (li, layer) in stem_layers.iter().enumerate() {
            let volume = if apply_mix {
                t.layer_volumes.get(li).copied().unwrap_or(1.0)
            } else {
                1.0
            };
            if apply_mix && volume <= 0.0 {
                continue; // fully muted for this track: no stem
            }
            let src_fmt = (cfg.format == ExportFormat::Source)
                .then(|| resolve_source_format(Path::new(&layer.source_path)));
            let ext = src_fmt
                .map(|(f, _, _)| f.extension())
                .unwrap_or_else(|| cfg.format.extension());
            // Same segment machinery as the mixdown planner: layer macros
            // are injected AFTER the '/' split so a layer name containing a
            // slash can never create a directory.
            let ln = format!("{:02}", li + 1);
            let segments: Vec<String> = cfg
                .template
                .split('/')
                .map(|seg| {
                    let seg = seg.replace("{ln}", &ln).replace("{layer}", &layer.name);
                    render_track_filename(
                        &crate::metadata::expand_macros(&seg, &ctx),
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
            let mut volumes = vec![0.0f32; stem_layers.len()];
            volumes[li] = volume;
            seq += 1;
            jobs.push(ExportJob {
                // Sequential across all stems: progress events and report
                // rows need a unique number per file.
                number: seq,
                title: format!("{} · {}", t.title, layer.name),
                start_sample: t.start_sample,
                end_sample: t.end_sample,
                out_path: candidate,
                layer_volumes: volumes,
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
                source_fmt: src_fmt,
                silence_ok: true,
                isrc: String::new(),
                gap_before_sectors: 0,
            });
        }
    }
    Ok(jobs)
}

/// The aresample stage shared by both export paths: sample-rate
/// conversion to the target rate and/or TPDF/noise-shaped dither when a
/// lossless output reduces to 16-bit. None = nothing to do (bit-exact
/// path preserved).
fn resample_filter(cfg: &ExportConfig, session_rate: u32) -> Option<String> {
    use crate::project::DitherMode;
    let out_rate = cfg.target_sample_rate.unwrap_or(session_rate);
    let rate_change = out_rate != session_rate;
    let reduces_depth =
        matches!(cfg.format, ExportFormat::Wav | ExportFormat::Flac) && cfg.bit_depth <= 16;
    let dither = match cfg.dither {
        DitherMode::Off => None,
        DitherMode::Auto => reduces_depth.then_some("triangular_hp"),
        DitherMode::Triangular => reduces_depth.then_some("triangular"),
        DitherMode::TriangularHp => reduces_depth.then_some("triangular_hp"),
        DitherMode::Shibata => reduces_depth.then_some("shibata"),
    };
    if !rate_change && dither.is_none() {
        return None;
    }
    let mut f = format!("aresample=osr={out_rate}");
    if rate_change {
        // Larger kernel + conservative cutoff: better SRC than defaults.
        f.push_str(":filter_size=128:cutoff=0.96");
    }
    if let Some(method) = dither {
        // Dither triggers on the conversion to the output sample format.
        f.push_str(&format!(":out_sample_fmt=s16:dither_method={method}"));
    }
    Some(f)
}

fn codec_args(cfg: &ExportConfig) -> Vec<String> {
    match cfg.format {
        // Source never reaches encoding (resolved at plan time via
        // ExportJob::effective_cfg); a stray value encodes as WAV.
        ExportFormat::Wav | ExportFormat::Source => vec![
            "-c:a".into(),
            if cfg.bit_depth == 24 {
                "pcm_s24le".into()
            } else {
                "pcm_s16le".into()
            },
        ],
        ExportFormat::Flac => {
            let mut v = vec!["-c:a".into(), "flac".into()];
            if cfg.bit_depth == 24 {
                v.extend(["-sample_fmt".into(), "s32".into()]);
                v.extend(["-bits_per_raw_sample".into(), "24".into()]);
            } else {
                v.extend(["-sample_fmt".into(), "s16".into()]);
            }
            v
        }
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

/// Measure the DELIVERED file with ffmpeg's ebur128 filter (summary
/// parsing): integrated loudness, loudness range and max true peak. This
/// meters exactly what the listener receives, after every codec quirk.
pub fn analyze_loudness(ffmpeg: &Path, file: &Path) -> (Option<f64>, Option<f64>, Option<f64>) {
    analyze_loudness_af(ffmpeg, file, "ebur128=peak=true")
}

/// Same measurement restricted to `start_sample..end_sample` of the file
/// (sample frames at the file's rate) — used for per-track figures inside
/// a CD image.
pub fn analyze_loudness_segment(
    ffmpeg: &Path,
    file: &Path,
    start_sample: u64,
    end_sample: u64,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    analyze_loudness_af(
        ffmpeg,
        file,
        &format!("atrim=start_sample={start_sample}:end_sample={end_sample},ebur128=peak=true"),
    )
}

/// Loudness of a slice of a RAW CD image (headerless s16le 44.1 kHz
/// stereo) — sample positions are frames at 44.1 kHz.
pub fn analyze_loudness_raw_segment(
    ffmpeg: &Path,
    file: &Path,
    start_sample: u64,
    end_sample: u64,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    analyze_loudness_cmd(
        ffmpeg,
        file,
        &format!("atrim=start_sample={start_sample}:end_sample={end_sample},ebur128=peak=true"),
        true,
    )
}

fn analyze_loudness_af(
    ffmpeg: &Path,
    file: &Path,
    af: &str,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    analyze_loudness_cmd(ffmpeg, file, af, false)
}

fn analyze_loudness_cmd(
    ffmpeg: &Path,
    file: &Path,
    af: &str,
    raw_cd_pcm: bool,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-hide_banner").arg("-nostdin");
    if raw_cd_pcm {
        cmd.arg("-f")
            .arg("s16le")
            .arg("-ar")
            .arg("44100")
            .arg("-ac")
            .arg("2");
    }
    let out = cmd
        .arg("-i")
        .arg(file)
        .arg("-af")
        .arg(af)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();
    let Ok(out) = out else {
        return (None, None, None);
    };
    let text = String::from_utf8_lossy(&out.stderr);
    let tail = &text[text.rfind("Summary:").unwrap_or(0)..];
    let grab = |label: &str| -> Option<f64> {
        tail.lines()
            .find(|l| l.trim_start().starts_with(label))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().split_whitespace().next())
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
    };
    // "Peak:" appears under BOTH "Sample peak:" and "True peak:" — take the
    // one from the True peak section specifically.
    let true_peak = tail
        .find("True peak:")
        .map(|i| &tail[i..])
        .and_then(|sect| {
            sect.lines()
                .find(|l| l.trim_start().starts_with("Peak:"))
                .and_then(|l| l.split(':').nth(1))
                .and_then(|v| v.trim().split_whitespace().next())
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
        });
    (grab("I:"), grab("LRA:"), true_peak)
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
            analyzing: false,
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
                // Resolve ExportFormat::Source into this job's concrete
                // format/depth/rate before touching any encoder path.
                let jcfg = job.effective_cfg(cfg);
                let cfg = &jcfg;
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
                        // Loudness is measured in a dedicated pass after all
                        // encoding, with its own progress phase.
                        files.lock().unwrap().push((
                            i,
                            ExportedFile {
                                track_title: job.title.clone(),
                                path: job.out_path.display().to_string(),
                                lufs_i: None,
                                lra: None,
                                true_peak_db: None,
                                track_measures: Vec::new(),
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
    let mut files: Vec<ExportedFile> = files.into_iter().map(|(_, f)| f).collect();
    let mut errors = errors.into_inner().unwrap();
    errors.sort_by_key(|(i, _)| *i);

    // Measurement pass on the delivered files (decode-only, still worth a
    // visible phase so the dialog doesn't look stalled before the report).
    let n = files.len() as u32;
    for (k, f) in files.iter_mut().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        on_progress(ExportProgress {
            track_number: (k + 1) as u32,
            track_count: n,
            track_title: f.track_title.clone(),
            track_progress: 0.0,
            completed_tracks: k as u32,
            overall_progress: k as f32 / n.max(1) as f32,
            analyzing: true,
        });
        let (lufs_i, lra, true_peak_db) = analyze_loudness(ffmpeg, Path::new(&f.path));
        f.lufs_i = lufs_i;
        f.lra = lra;
        f.true_peak_db = true_peak_db;
        on_progress(ExportProgress {
            track_number: (k + 1) as u32,
            track_count: n,
            track_title: f.track_title.clone(),
            track_progress: 1.0,
            completed_tracks: (k + 1) as u32,
            overall_progress: (k + 1) as f32 / n.max(1) as f32,
            analyzing: true,
        });
    }

    ExportReport {
        files,
        errors: errors.into_iter().map(|(_, e)| e).collect(),
        cancelled: cancel.load(Ordering::SeqCst),
    }
}

/// Render one track OFFLINE through the mastering chain and pipe the PCM
/// to ffmpeg for encoding. Latency-compensated: the chain's total reported
/// latency is rendered ahead and dropped, so the output stays sample-exact.
#[allow(clippy::too_many_arguments)]
/// Build a per-track renderer with every chain instantiated on the CALLING
/// thread: decoders for the audible layers (compaction mapped back to the
/// ORIGINAL layer indices for lane chains), the track's own chain + the
/// mastering chain on the master bus, latency summed for compensation.
fn build_track_renderer(
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    job: &ExportJob,
    chain: &[MasterPluginSpec],
    lane_chains: &[Vec<MasterPluginSpec>],
) -> Result<(Renderer, u64, usize)> {
    let channels = (session_channels.max(1) as usize).min(2);

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
                    offset: 0,
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
    let mut inserts = instantiate(&job.track_chain)?;
    inserts.extend(instantiate(chain)?);
    // Latency compensation covers the master bus (track + mastering).
    // Lane-chain latency is NOT compensated (same inter-layer skew as
    // playback; typical layer inserts are zero-latency).
    let latency: u64 = inserts.iter().map(|p| p.latency_samples() as u64).sum();

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
    Ok((renderer, latency, channels))
}

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
    let track_len = job.end_sample.saturating_sub(job.start_sample);
    let (mut renderer, latency, channels) =
        build_track_renderer(layers, session_channels, sample_rate, job, chain, lane_chains)?;

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
        .arg("pipe:0");
    if let Some(rf) = resample_filter(cfg, sample_rate) {
        cmd.arg("-af").arg(rf);
    }
    cmd.args(codec_args(cfg))
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
    if active.is_empty() && !job.silence_ok {
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
    if active.is_empty() {
        // Silent stem: full window length, so the stem set stays aligned.
        let layout = if session_channels >= 2 { "stereo" } else { "mono" };
        let win = job.end_sample.saturating_sub(job.start_sample);
        let mut filter = format!(
            "anullsrc=r={sample_rate}:cl={layout},atrim=end_sample={win},asetpts=PTS-STARTPTS[s0]"
        );
        let final_label = if let Some(rf) = resample_filter(cfg, sample_rate) {
            filter.push_str(&format!(";[s0]{rf}[cond]"));
            "cond"
        } else {
            "s0"
        };
        cmd.arg("-filter_complex")
            .arg(&filter)
            .arg("-map")
            .arg(format!("[{final_label}]"));
    } else if single_plain {
        let TimelinePart::File { start: s, end: e, .. } = active[0].2[0] else {
            unreachable!()
        };
        let mut af = format!("atrim=start_sample={s}:end_sample={e},asetpts=PTS-STARTPTS");
        if let Some(rf) = resample_filter(cfg, sample_rate) {
            af.push(',');
            af.push_str(&rf);
        }
        cmd.arg("-map").arg("0:a:0").arg("-af").arg(af);
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
        // Output conditioning (SRC + dither) appended to the graph tail.
        let final_label = if let Some(rf) = resample_filter(cfg, sample_rate) {
            filter.push_str(&format!("[{final_label}]{rf}[cond];"));
            "cond".to_string()
        } else {
            final_label
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
            isrc: String::new(),
            gap_before_ms: None,
            gap_before_effective_ms: 0,
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

/// Red Book frame: 1/75 s at 44.1 kHz.
const CD_FRAME_SAMPLES: u64 = 588;
const CD_RATE: u32 = 44_100;

fn cue_time(frames: u64) -> String {
    let ff = frames % 75;
    let secs = frames / 75;
    format!("{:02}:{:02}:{:02}", secs / 60, secs % 60, ff)
}

/// Export ONE Red Book image (44.1 kHz / 16-bit stereo WAV, every track
/// padded to a CD frame boundary) plus its cue sheet with CD-Text from the
/// album metadata. Tracks render through the full chain stack exactly like
/// per-file exports; the image is sequential by nature (single worker).
#[allow(clippy::too_many_arguments)]
pub fn run_export_cd_image(
    ffmpeg: &Path,
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    jobs: &[ExportJob],
    cfg: &ExportConfig,
    chain: &[MasterPluginSpec],
    lane_chains: &[Vec<MasterPluginSpec>],
    meta: &AlbumMeta,
    cancel: &AtomicBool,
    on_progress: impl Fn(ExportProgress) + Send + Sync,
) -> ExportReport {
    run_export_cd_common(
        ffmpeg, layers, session_channels, sample_rate, jobs, cfg, chain, lane_chains, meta,
        cancel, &on_progress, false,
    )
}

/// Tier-3 pro export (#5): the same rendered Red Book program delivered
/// as a DDP 2.00 fileset (IMAGE.DAT + DDPID + DDPMS + PQDESCR +
/// checksums) with a human-readable PQ sheet — what pressing plants
/// actually ingest. Subcode carries per-track ISRCs and the album EAN.
#[allow(clippy::too_many_arguments)]
pub fn run_export_ddp(
    ffmpeg: &Path,
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    jobs: &[ExportJob],
    cfg: &ExportConfig,
    chain: &[MasterPluginSpec],
    lane_chains: &[Vec<MasterPluginSpec>],
    meta: &AlbumMeta,
    cancel: &AtomicBool,
    on_progress: impl Fn(ExportProgress) + Send + Sync,
) -> ExportReport {
    run_export_cd_common(
        ffmpeg, layers, session_channels, sample_rate, jobs, cfg, chain, lane_chains, meta,
        cancel, &on_progress, true,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_export_cd_common(
    ffmpeg: &Path,
    layers: &[LayerMix],
    session_channels: u16,
    sample_rate: u32,
    jobs: &[ExportJob],
    cfg: &ExportConfig,
    chain: &[MasterPluginSpec],
    lane_chains: &[Vec<MasterPluginSpec>],
    meta: &AlbumMeta,
    cancel: &AtomicBool,
    on_progress: &(impl Fn(ExportProgress) + Send + Sync),
    ddp: bool,
) -> ExportReport {
    let mut report = ExportReport {
        files: Vec::new(),
        errors: Vec::new(),
        cancelled: false,
    };

    // Image + cue paths (collision-suffixed like every export output).
    let dest = PathBuf::from(&cfg.dest_dir);
    if let Err(e) = std::fs::create_dir_all(&dest) {
        report.errors.push(e.to_string());
        return report;
    }
    let base_name = if meta.album.trim().is_empty() {
        "CD Image".to_string()
    } else {
        crate::naming::sanitize_filename(meta.album.trim())
    };
    let mut wav_path = dest.join(format!("{base_name}.wav"));
    let mut cue_path = dest.join(format!("{base_name}.cue"));
    let mut ddp_dir = dest.join(format!("{base_name} DDP"));
    let mut k = 1;
    loop {
        let collides = if ddp {
            ddp_dir.exists()
        } else {
            wav_path.exists() || cue_path.exists()
        };
        if !collides {
            break;
        }
        wav_path = dest.join(format!("{base_name} ({k}).wav"));
        cue_path = dest.join(format!("{base_name} ({k}).cue"));
        ddp_dir = dest.join(format!("{base_name} DDP ({k})"));
        k += 1;
    }

    // One ffmpeg process encodes the whole image: 44.1 kHz f32 stereo in,
    // dithered 16-bit WAV out (SRC to CD rate happens on OUR side so the
    // cue frame offsets are exact).
    use crate::project::DitherMode;
    let dither = match cfg.dither {
        DitherMode::Off => "0",
        DitherMode::Triangular => "triangular",
        DitherMode::Shibata => "shibata",
        _ => "triangular_hp",
    };
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-f")
        .arg("f32le")
        .arg("-ar")
        .arg(CD_RATE.to_string())
        .arg("-ac")
        .arg("2")
        .arg("-i")
        .arg("pipe:0")
        .arg("-af")
        .arg(format!(
            "aresample=osr={CD_RATE}:out_sample_fmt=s16:dither_method={dither}"
        ))
        .arg("-c:a")
        .arg("pcm_s16le");
    if ddp {
        // Raw little-endian PCM to stdout, streamed into the fileset's
        // image (the 150-sector pause is written by the fileset itself).
        cmd.arg("-f").arg("s16le").arg("pipe:1").stdout(Stdio::piped());
    } else {
        cmd.arg(&wav_path).stdout(Stdio::null());
    }
    cmd.stdin(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            report.errors.push(if e.kind() == std::io::ErrorKind::NotFound {
                StillError::FfmpegNotFound.to_string()
            } else {
                e.to_string()
            });
            return report;
        }
    };
    let mut stdin = child.stdin.take().expect("stdin piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        use std::io::Read as _;
        let _ = stderr.read_to_string(&mut buf);
        buf
    });

    // DDP: a dedicated thread drains ffmpeg's stdout into the image
    // (hashing as it goes) while the render loop feeds stdin.
    let copier = if ddp {
        let mut out = child.stdout.take().expect("stdout piped");
        let fileset = match ddp_fileset::Fileset::create(&ddp_dir) {
            Ok(f) => f,
            Err(e) => {
                report.errors.push(format!("DDP fileset: {e}"));
                return report;
            }
        };
        Some(std::thread::spawn(
            move || -> std::io::Result<ddp_fileset::Fileset> {
                let mut fileset = fileset;
                let mut buf = vec![0u8; 256 * 1024];
                loop {
                    let n = out.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    fileset.write_audio(&buf[..n])?;
                }
                Ok(fileset)
            },
        ))
    } else {
        None
    };

    // Per-track render → SRC to 44.1 stereo → frame padding, counting
    // OUTPUT samples exactly for the cue offsets.
    // (number, title, isrc, start frame, pregap sectors)
    let mut track_starts: Vec<(u32, String, String, u64, u64)> = Vec::new();
    let mut image_samples: u64 = 0;
    let mut failed = false;
    use std::io::Write as _;
    let push = |stdin: &mut std::process::ChildStdin, data: &[f32]| -> bool {
        let bytes = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4)
        };
        stdin.write_all(bytes).is_ok()
    };

    'tracks: for (ti, job) in jobs.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            report.cancelled = true;
            break;
        }
        let (mut renderer, latency, channels) = match build_track_renderer(
            layers,
            session_channels,
            sample_rate,
            job,
            chain,
            lane_chains,
        ) {
            Ok(r) => r,
            Err(e) => {
                report
                    .errors
                    .push(format!("{}: {e}", job.title));
                failed = true;
                break;
            }
        };
        // Album gap: silence between the previous track and this one —
        // pushed BEFORE this track's start entry, so cue INDEX 01, the
        // DDP table and the loudness segments all shift together.
        if job.gap_before_sectors > 0 {
            let zeros = vec![0.0f32; (job.gap_before_sectors * CD_FRAME_SAMPLES) as usize * 2];
            if !push(&mut stdin, &zeros) {
                failed = true;
                break;
            }
            image_samples += job.gap_before_sectors * CD_FRAME_SAMPLES;
        }
        track_starts.push((
            job.number,
            job.title.clone(),
            job.isrc.clone(),
            image_samples / CD_FRAME_SAMPLES,
            job.gap_before_sectors,
        ));

        let mut resampler = (sample_rate != CD_RATE)
            .then(|| crate::engine::resample::StreamResampler::new(sample_rate, CD_RATE, 2));

        let track_len = job.end_sample.saturating_sub(job.start_sample);
        let mut block = vec![0.0f32; BLOCK_FRAMES * channels];
        let mut stereo = vec![0.0f32; BLOCK_FRAMES * 2];
        let mut rs_out = vec![0.0f32; (BLOCK_FRAMES * 2 + 16) * 2];
        let mut to_skip = latency;
        let mut consumed = 0u64;
        let mut track_out = 0u64;

        let feed = |resampler: &mut Option<crate::engine::resample::StreamResampler>,
                        stereo: &[f32],
                        frames: usize,
                        rs_out: &mut Vec<f32>,
                        stdin: &mut std::process::ChildStdin,
                        track_out: &mut u64|
         -> bool {
            match resampler {
                Some(rs) => {
                    let need = rs.max_out_frames(frames) * 2;
                    if rs_out.len() < need {
                        rs_out.resize(need, 0.0);
                    }
                    let n = rs.process(stereo, frames, rs_out);
                    *track_out += n as u64;
                    push(stdin, &rs_out[..n * 2])
                }
                None => {
                    *track_out += frames as u64;
                    push(stdin, &stereo[..frames * 2])
                }
            }
        };

        while consumed < track_len {
            if cancel.load(Ordering::SeqCst) {
                report.cancelled = true;
                break 'tracks;
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
            let usable = ((got - offset) as u64).min(track_len - consumed) as usize;
            if usable == 0 {
                continue;
            }
            // Upmix/copy to stereo interleaved.
            for f in 0..usable {
                let src = (offset + f) * channels;
                stereo[f * 2] = block[src];
                stereo[f * 2 + 1] = block[src + (channels - 1).min(1)];
            }
            if !feed(&mut resampler, &stereo, usable, &mut rs_out, &mut stdin, &mut track_out) {
                failed = true;
                break 'tracks;
            }
            consumed += usable as u64;
            on_progress(ExportProgress {
                track_number: job.number,
                track_count: jobs.len() as u32,
                track_title: job.title.clone(),
                track_progress: (consumed as f32 / track_len.max(1) as f32).min(1.0),
                completed_tracks: ti as u32,
                overall_progress: (ti as f32 + consumed as f32 / track_len.max(1) as f32)
                    / jobs.len().max(1) as f32,
                analyzing: false,
            });
        }
        // Flush the resampler tail with silence so the last output frames
        // of this track come out before the padding.
        if resampler.is_some() {
            let silence = vec![0.0f32; 64 * 2];
            for _ in 0..2 {
                if !feed(&mut resampler, &silence, 64, &mut rs_out, &mut stdin, &mut track_out) {
                    failed = true;
                    break 'tracks;
                }
            }
        }
        // Pad to the Red Book frame boundary.
        let rem = (track_out % CD_FRAME_SAMPLES) as usize;
        if rem != 0 {
            let pad = CD_FRAME_SAMPLES as usize - rem;
            let zeros = vec![0.0f32; pad * 2];
            if !push(&mut stdin, &zeros) {
                failed = true;
                break 'tracks;
            }
            track_out += pad as u64;
        }
        image_samples += track_out;
        on_progress(ExportProgress {
            track_number: job.number,
            track_count: jobs.len() as u32,
            track_title: job.title.clone(),
            track_progress: 1.0,
            completed_tracks: (ti + 1) as u32,
            overall_progress: (ti + 1) as f32 / jobs.len().max(1) as f32,
            analyzing: false,
        });
    }

    drop(stdin);
    let status = child.wait();
    let err_output = stderr_reader.join().unwrap_or_default();
    // The copier ends when ffmpeg's stdout closes; collect its fileset.
    let fileset = copier.map(|h| {
        h.join()
            .unwrap_or_else(|_| Err(std::io::Error::other("image writer thread panicked")))
    });
    if report.cancelled || failed || !status.map(|s| s.success()).unwrap_or(false) {
        if ddp {
            let _ = std::fs::remove_dir_all(&ddp_dir);
        } else {
            let _ = std::fs::remove_file(&wav_path);
        }
        if !report.cancelled {
            let msg = err_output.trim();
            report.errors.push(if msg.is_empty() {
                "the CD image encoder failed".into()
            } else {
                msg.to_string()
            });
        }
        return report;
    }

    if ddp {
        let fileset = match fileset.expect("ddp mode always has a copier") {
            Ok(f) => f,
            Err(e) => {
                report.errors.push(format!("DDP image: {e}"));
                let _ = std::fs::remove_dir_all(&ddp_dir);
                return report;
            }
        };
        let program_sectors = image_samples / CD_FRAME_SAMPLES;
        let tracks: Vec<ddp_fileset::Track> = track_starts
            .iter()
            .enumerate()
            .map(|(i, (number, title, isrc, start, gap))| {
                // A track's audio ends where the NEXT track's pregap
                // starts — the pause belongs to the following title.
                let end = track_starts
                    .get(i + 1)
                    .map(|t| t.3 - t.4)
                    .unwrap_or(program_sectors);
                ddp_fileset::Track {
                    number: *number,
                    title: title.clone(),
                    start_sector: *start,
                    length_sectors: end.saturating_sub(*start),
                    isrc: (!isrc.is_empty()).then(|| isrc.clone()),
                    pregap_sectors: *gap,
                }
            })
            .collect();
        let ean: String = meta
            .catalog_ean
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect();
        let disc = ddp_fileset::Disc {
            title: if meta.album.trim().is_empty() {
                base_name.clone()
            } else {
                meta.album.trim().to_string()
            },
            performer: if meta.album_artist.trim().is_empty() {
                meta.artist.trim().to_string()
            } else {
                meta.album_artist.trim().to_string()
            },
            ean: (ean.len() == 12 || ean.len() == 13).then_some(ean),
            tracks,
        };
        if let Err(e) = fileset.finish(&disc) {
            report.errors.push(format!("DDP fileset: {e}"));
            let _ = std::fs::remove_dir_all(&ddp_dir);
            return report;
        }

        // Loudness on the raw image, program area only (pause skipped).
        let image_path = ddp_dir.join(ddp_fileset::IMAGE_NAME);
        let pause = ddp_fileset::PAUSE_SECTORS * CD_FRAME_SAMPLES;
        let steps = 1 + track_starts.len() as u32;
        let analyze_progress = |k: u32, title: &str, done: bool| {
            on_progress(ExportProgress {
                track_number: k + 1,
                track_count: steps,
                track_title: title.to_string(),
                track_progress: if done { 1.0 } else { 0.0 },
                completed_tracks: k + done as u32,
                overall_progress: (k + done as u32) as f32 / steps.max(1) as f32,
                analyzing: true,
            });
        };
        analyze_progress(0, "DDP image", false);
        let (lufs_i, lra, true_peak_db) =
            analyze_loudness_raw_segment(ffmpeg, &image_path, pause, pause + image_samples);
        analyze_progress(0, "DDP image", true);
        let track_measures = track_starts
            .iter()
            .enumerate()
            .map(|(i, (number, title, _isrc, start_frame, _gap))| {
                analyze_progress(1 + i as u32, title, false);
                let start = pause + start_frame * CD_FRAME_SAMPLES;
                let end = pause
                    + track_starts
                        .get(i + 1)
                        .map(|t| (t.3 - t.4) * CD_FRAME_SAMPLES)
                        .unwrap_or(image_samples);
                let (l, lra, tp) = analyze_loudness_raw_segment(ffmpeg, &image_path, start, end);
                analyze_progress(1 + i as u32, title, true);
                TrackMeasure {
                    number: *number,
                    title: title.clone(),
                    lufs_i: l,
                    lra,
                    true_peak_db: tp,
                }
            })
            .collect();
        report.files.push(ExportedFile {
            track_title: "DDP fileset".into(),
            path: ddp_dir.display().to_string(),
            lufs_i,
            lra,
            true_peak_db,
            track_measures,
        });
        report.files.push(ExportedFile {
            track_title: "PQ sheet".into(),
            path: ddp_dir.join("PQ_SHEET.TXT").display().to_string(),
            lufs_i: None,
            lra: None,
            true_peak_db: None,
            track_measures: Vec::new(),
        });
        return report;
    }

    // Cue sheet with CD-Text; CATALOG only when the EAN looks valid.
    let performer = if meta.album_artist.trim().is_empty() {
        meta.artist.trim()
    } else {
        meta.album_artist.trim()
    };
    let mut cue = String::new();
    let ean: String = meta.catalog_ean.chars().filter(|c| c.is_ascii_digit()).collect();
    if ean.len() == 13 || ean.len() == 12 {
        cue.push_str(&format!("CATALOG {ean}\r\n"));
    }
    if !performer.is_empty() {
        cue.push_str(&format!("PERFORMER \"{}\"\r\n", performer.replace('"', "'")));
    }
    if !meta.album.trim().is_empty() {
        cue.push_str(&format!("TITLE \"{}\"\r\n", meta.album.trim().replace('"', "'")));
    }
    cue.push_str(&format!(
        "FILE \"{}\" WAVE\r\n",
        wav_path.file_name().unwrap_or_default().to_string_lossy()
    ));
    for (number, title, isrc, start_frame, gap) in &track_starts {
        cue.push_str(&format!("  TRACK {number:02} AUDIO\r\n"));
        cue.push_str(&format!("    TITLE \"{}\"\r\n", title.replace('"', "'")));
        if !performer.is_empty() {
            cue.push_str(&format!("    PERFORMER \"{}\"\r\n", performer.replace('"', "'")));
        }
        if !isrc.is_empty() {
            cue.push_str(&format!("    ISRC {isrc}\r\n"));
        }
        // Real Red Book pregap: INDEX 00 opens the pause, INDEX 01 the
        // downbeat - players count the gap down, rippers show it.
        if *gap > 0 {
            cue.push_str(&format!("    INDEX 00 {}\r\n", cue_time(start_frame - gap)));
        }
        cue.push_str(&format!("    INDEX 01 {}\r\n", cue_time(*start_frame)));
    }
    if let Err(e) = std::fs::write(&cue_path, cue) {
        report.errors.push(e.to_string());
        return report;
    }

    // Measurement pass: whole image first, then every cue segment on its
    // own (exactly what a CD player delivers per track) — with progress so
    // the dialog doesn't look stalled before the report.
    let steps = 1 + track_starts.len() as u32;
    let analyze_progress = |k: u32, title: &str, done: bool| {
        on_progress(ExportProgress {
            track_number: k + 1,
            track_count: steps,
            track_title: title.to_string(),
            track_progress: if done { 1.0 } else { 0.0 },
            completed_tracks: k + done as u32,
            overall_progress: (k + done as u32) as f32 / steps.max(1) as f32,
            analyzing: true,
        });
    };
    analyze_progress(0, "CD image", false);
    let (lufs_i, lra, true_peak_db) = analyze_loudness(ffmpeg, &wav_path);
    analyze_progress(0, "CD image", true);
    let track_measures = track_starts
        .iter()
        .enumerate()
        .map(|(i, (number, title, _isrc, start_frame, _gap))| {
            analyze_progress(1 + i as u32, title, false);
            let start = start_frame * CD_FRAME_SAMPLES;
            // Audio only: stop where the next track's pregap begins.
            let end = track_starts
                .get(i + 1)
                .map(|t| (t.3 - t.4) * CD_FRAME_SAMPLES)
                .unwrap_or(image_samples);
            let (l, lra, tp) = analyze_loudness_segment(ffmpeg, &wav_path, start, end);
            analyze_progress(1 + i as u32, title, true);
            TrackMeasure {
                number: *number,
                title: title.clone(),
                lufs_i: l,
                lra,
                true_peak_db: tp,
            }
        })
        .collect();
    report.files.push(ExportedFile {
        track_title: "CD image".into(),
        path: wav_path.display().to_string(),
        lufs_i,
        lra,
        true_peak_db,
        track_measures,
    });
    report.files.push(ExportedFile {
        track_title: "Cue sheet".into(),
        path: cue_path.display().to_string(),
        lufs_i: None,
        lra: None,
        true_peak_db: None,
        track_measures: Vec::new(),
    });
    report
}
