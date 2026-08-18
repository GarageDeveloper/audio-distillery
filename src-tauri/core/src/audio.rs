use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, Track};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;
use ts_rs::TS;

use crate::error::{Result, StillError};
use crate::peaks::{PeakBuilder, PeakPyramid};

pub const SUPPORTED_EXTENSIONS: &[&str] = &["wav", "flac", "mp3", "aiff", "aif"];

/// One source file placed on the session timeline. Clips are laid out
/// back-to-back in order; all positions are timeline samples.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ClipInfo {
    pub path: String,
    /// File name (display).
    pub name: String,
    /// Timeline position of the clip's first sample.
    #[ts(type = "number")]
    pub start_sample: u64,
    #[ts(type = "number")]
    pub duration_samples: u64,
}

/// Scan result for one layer: its clips (sequential), channel count and
/// total length. Layers are time-synchronized: they all start at t = 0.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ScannedLayer {
    pub clips: Vec<ClipInfo>,
    pub channels: u16,
    #[ts(type = "number")]
    pub duration_samples: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct AudioInfo {
    /// First clip's path of the base layer (display/naming convenience).
    pub path: String,
    /// Base layer's clips (timeline boundaries shown in the UI).
    pub clips: Vec<ClipInfo>,
    /// All layers, in order (index-aligned with the project's layer list).
    pub layers: Vec<ScannedLayer>,
    /// Total timeline length (longest layer).
    #[ts(type = "number")]
    pub duration_samples: u64,
    pub sample_rate: u32,
    /// Session channel count (max over layers).
    pub channels: u16,
    /// Display string, e.g. "FLAC".
    pub format: String,
    pub duration_seconds: f64,
}

/// A piece of a layer over a timeline range: either part of a source file,
/// or a silent gap (between takes / after the layer's end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelinePart {
    /// (clip index, local start sample, local end sample)
    File { clip: usize, start: u64, end: u64 },
    Silence { samples: u64 },
}

/// Decompose a timeline range into file parts and silent gaps for one layer.
/// The parts always cover [start, end) exactly, keeping layers in sync at
/// export even across take gaps.
pub fn layer_parts(clips: &[ClipInfo], start: u64, end: u64) -> Vec<TimelinePart> {
    let mut parts = Vec::new();
    let mut cursor = start;
    for (i, c) in clips.iter().enumerate() {
        let cs = c.start_sample;
        let ce = cs + c.duration_samples;
        if ce <= cursor {
            continue;
        }
        if cs >= end {
            break;
        }
        if cs > cursor {
            parts.push(TimelinePart::Silence { samples: cs - cursor });
            cursor = cs;
        }
        let e = end.min(ce);
        if cursor < e {
            parts.push(TimelinePart::File {
                clip: i,
                start: cursor - cs,
                end: e - cs,
            });
            cursor = e;
        }
    }
    if cursor < end {
        parts.push(TimelinePart::Silence { samples: end - cursor });
    }
    parts
}

/// Map a timeline range onto the clips it covers: (clip index, local start,
/// local end) — local positions are samples within that clip's file.
pub fn clip_segments(clips: &[ClipInfo], start: u64, end: u64) -> Vec<(usize, u64, u64)> {
    let mut out = Vec::new();
    for (i, c) in clips.iter().enumerate() {
        let c_end = c.start_sample + c.duration_samples;
        let s = start.max(c.start_sample);
        let e = end.min(c_end);
        if s < e {
            out.push((i, s - c.start_sample, e - c.start_sample));
        }
    }
    out
}

struct Opened {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    n_frames_hint: Option<u64>,
}

/// Open a source file strictly read-only and prepare a decoder.
fn open(path: &Path) -> Result<Opened> {
    if !path.is_file() {
        return Err(StillError::FileNotFound(path.display().to_string()));
    }
    // Read-only handle: the source file is sacred (SPEC §3 bis).
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|_| StillError::UnsupportedFormat(path.display().to_string()))?;
    let format = probed.format;
    let track: &Track = format
        .default_track()
        .ok_or_else(|| StillError::UnsupportedFormat("no audio track found".into()))?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let sample_rate = params
        .sample_rate
        .ok_or_else(|| StillError::Decode("unknown sample rate".into()))?;
    let channels = params
        .channels
        .map(|c| c.count())
        .filter(|&c| c > 0)
        .ok_or_else(|| StillError::Decode("unknown channel layout".into()))? as u16;
    let n_frames_hint = params.n_frames;
    let decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| StillError::Decode(e.to_string()))?;
    Ok(Opened {
        format,
        decoder,
        track_id,
        sample_rate,
        channels,
        n_frames_hint,
    })
}

/// Decode one opened file to the end, feeding the shared peak builder.
/// `progress` receives the local 0.0..=1.0 fraction for THIS file.
fn decode_into(
    o: &mut Opened,
    builder: &mut PeakBuilder,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<u64> {
    let start_frames = builder.total_frames();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(StillError::ScanCancelled);
        }
        let packet = match o.format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(StillError::Decode(e.to_string())),
        };
        if packet.track_id() != o.track_id {
            continue;
        }
        match o.decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let needed = decoded.capacity() as u64;
                let buf = match &mut sample_buf {
                    Some(b) if b.capacity() as u64 >= needed * spec.channels.count() as u64 => b,
                    _ => sample_buf.insert(SampleBuffer::new(needed, spec)),
                };
                buf.copy_interleaved_ref(decoded);
                builder.push_interleaved(buf.samples());
                if let Some(nf) = o.n_frames_hint {
                    if nf > 0 {
                        let done = builder.total_frames() - start_frames;
                        progress((done as f32 / nf as f32).min(1.0));
                    }
                }
            }
            // Skip over corrupt packets instead of failing the whole scan.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(StillError::Decode(e.to_string())),
        }
    }
    Ok(builder.total_frames() - start_frames)
}

/// Decode one layer's sequential clips into one pyramid. Clips within a
/// layer must share the layer's sample rate and channel count.
/// `sample_rate_lock`: Some(rate) enforces the session rate.
fn scan_group(
    paths: &[(PathBuf, Option<u64>)],
    sample_rate_lock: Option<u32>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, f32),
) -> Result<(ScannedLayer, u32, PeakPyramid)> {
    if paths.is_empty() {
        return Err(StillError::Decode("no audio file given".into()));
    }
    let mut builder: Option<PeakBuilder> = None;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut clips = Vec::with_capacity(paths.len());

    for (i, (path, pinned_start)) in paths.iter().enumerate() {
        let mut o = open(path)?;
        match &builder {
            None => {
                if let Some(rate) = sample_rate_lock {
                    if o.sample_rate != rate {
                        return Err(StillError::UnsupportedFormat(format!(
                            "{}: every layer must share the session sample rate ({} Hz expected — this file is {} Hz)",
                            path.display(),
                            rate,
                            o.sample_rate
                        )));
                    }
                }
                sample_rate = o.sample_rate;
                channels = o.channels;
                builder = Some(PeakBuilder::new(channels as usize));
            }
            Some(_) if o.sample_rate != sample_rate || o.channels != channels => {
                return Err(StillError::UnsupportedFormat(format!(
                    "{}: every clip must share the first clip's format ({} Hz, {} channel(s) expected — this file is {} Hz, {} channel(s))",
                    path.display(),
                    sample_rate,
                    channels,
                    o.sample_rate,
                    o.channels
                )));
            }
            Some(_) => {}
        }
        let b = builder.as_mut().expect("builder initialized above");
        // Take alignment: a pinned clip starts at its explicit timeline
        // position; the gap up to there is silence.
        if let Some(start) = pinned_start {
            let cur = b.total_frames();
            if *start > cur {
                b.push_silence(*start - cur);
            }
        }
        let start_sample = b.total_frames();
        let frames = decode_into(&mut o, b, cancel, |f| progress(i, f))?;
        if frames == 0 {
            return Err(StillError::Decode(format!(
                "{}: the file contains no decodable audio",
                path.display()
            )));
        }
        clips.push(ClipInfo {
            path: path.display().to_string(),
            name: path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            start_sample,
            duration_samples: frames,
        });
    }

    let (pyramid, frames) = builder.expect("at least one file").finish();
    Ok((
        ScannedLayer {
            clips,
            channels,
            duration_samples: frames,
        },
        sample_rate,
        pyramid,
    ))
}

/// Decode a whole multitrack session: each group of paths is one LAYER
/// (time-synchronized, starting at t = 0), each layer's paths are laid
/// back-to-back. Layers must share the sample rate; channel counts may
/// differ (a mono Zoom input next to the stereo mic is fine).
/// `progress` receives 0.0..=1.0 over all files of all layers.
pub fn scan_layers(
    groups: &[Vec<(PathBuf, Option<u64>)>],
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<(AudioInfo, Vec<PeakPyramid>)> {
    if groups.is_empty() || groups[0].is_empty() {
        return Err(StillError::Decode("no audio file given".into()));
    }
    let total_files: usize = groups.iter().map(|g| g.len()).sum();
    let mut done_files = 0usize;
    let mut sample_rate: Option<u32> = None;
    let mut layers = Vec::with_capacity(groups.len());
    let mut pyramids = Vec::with_capacity(groups.len());

    for group in groups {
        let (layer, rate, pyramid) = scan_group(group, sample_rate, cancel, |i, f| {
            progress(((done_files + i) as f32 + f) / total_files as f32)
        })?;
        done_files += group.len();
        sample_rate = Some(rate);
        layers.push(layer);
        pyramids.push(pyramid);
    }
    progress(1.0);

    let sample_rate = sample_rate.expect("at least one layer scanned");
    let duration_samples = layers.iter().map(|l| l.duration_samples).max().unwrap_or(0);
    let channels = layers.iter().map(|l| l.channels).max().unwrap_or(0);
    let first = &groups[0][0].0;
    let format_name = first
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| "AUDIO".into());
    Ok((
        AudioInfo {
            path: first.display().to_string(),
            clips: layers[0].clips.clone(),
            layers,
            duration_samples,
            sample_rate,
            channels,
            format: format_name,
            duration_seconds: duration_samples as f64 / sample_rate as f64,
        },
        pyramids,
    ))
}

/// Single-layer convenience wrapper around [`scan_layers`].
pub fn scan_files(
    paths: &[PathBuf],
    cancel: &AtomicBool,
    progress: impl FnMut(f32),
) -> Result<(AudioInfo, PeakPyramid)> {
    let group: Vec<(PathBuf, Option<u64>)> = paths.iter().map(|p| (p.clone(), None)).collect();
    let (info, mut pyramids) = scan_layers(&[group], cancel, progress)?;
    Ok((info, pyramids.remove(0)))
}

/// Single-file, non-cancellable convenience wrapper around [`scan_files`].
pub fn scan_file(path: &Path, progress: impl FnMut(f32)) -> Result<(AudioInfo, PeakPyramid)> {
    scan_files(
        std::slice::from_ref(&path.to_path_buf()),
        &AtomicBool::new(false),
        progress,
    )
}

/// In `samples`, find the index of the zero crossing nearest to `center`.
/// Returns `None` when the window contains no sign change.
pub fn nearest_zero_crossing(samples: &[f32], center: usize) -> Option<usize> {
    if samples.len() < 2 {
        return None;
    }
    let crossing_at = |i: usize| -> bool {
        let a = samples[i];
        let b = samples[i + 1];
        (a <= 0.0 && b > 0.0) || (a >= 0.0 && b < 0.0)
    };
    let center = center.min(samples.len() - 1);
    let mut best: Option<usize> = None;
    for d in 0..samples.len() {
        let mut found = None;
        if center + d + 1 < samples.len() && crossing_at(center + d) {
            found = Some(center + d + 1);
        } else if d <= center && center - d + 1 < samples.len() && crossing_at(center - d) {
            found = Some(center - d + 1);
        }
        if let Some(i) = found {
            best = Some(i);
            break;
        }
    }
    best
}

/// Decode a small mono window around the timeline `position` (mapped into
/// the clip that contains it) and return the timeline position of the
/// nearest zero crossing. Falls back to the original position when seeking
/// or decoding is not possible (never an error: snapping is best-effort).
pub fn snap_to_zero_crossing(clips: &[ClipInfo], position: u64, window_ms: u32) -> u64 {
    let clip = clips
        .iter()
        .find(|c| position >= c.start_sample && position < c.start_sample + c.duration_samples)
        .or_else(|| clips.last());
    let Some(clip) = clip else { return position };
    let local = position.saturating_sub(clip.start_sample);
    match snap_inner(Path::new(&clip.path), local, window_ms) {
        Ok(Some(p)) => clip.start_sample + p.min(clip.duration_samples),
        _ => position,
    }
}

fn snap_inner(path: &Path, position: u64, window_ms: u32) -> Result<Option<u64>> {
    let mut o = open(path)?;
    let sr = o.sample_rate as u64;
    let window = (window_ms as u64 * sr / 1000).max(64);
    let start = position.saturating_sub(window);
    let end = position + window;

    let seek_secs = start as f64 / sr as f64;
    let seeked = o
        .format
        .seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(seek_secs),
                track_id: Some(o.track_id),
            },
        )
        .map_err(|e| StillError::Decode(e.to_string()))?;
    o.decoder.reset();
    // actual_ts is in the track's timebase; for PCM-style audio this is frames.
    let mut cursor = seeked.actual_ts;
    if cursor > end {
        return Ok(None);
    }

    let mut mono: Vec<f32> = Vec::with_capacity((end - cursor.min(end)) as usize + 4096);
    let window_start = cursor;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match o.format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        if packet.track_id() != o.track_id {
            continue;
        }
        let decoded = match o.decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let spec = *decoded.spec();
        let ch = spec.channels.count().max(1);
        let needed = decoded.capacity() as u64;
        let buf = match &mut sample_buf {
            Some(b) if b.capacity() as u64 >= needed * ch as u64 => b,
            _ => sample_buf.insert(SampleBuffer::new(needed, spec)),
        };
        buf.copy_interleaved_ref(decoded);
        for frame in buf.samples().chunks_exact(ch) {
            let v: f32 = frame.iter().sum::<f32>() / ch as f32;
            mono.push(v);
        }
        cursor = window_start + mono.len() as u64;
        if cursor >= end {
            break;
        }
    }
    if mono.is_empty() || position < window_start {
        return Ok(None);
    }
    let center = (position - window_start) as usize;
    Ok(nearest_zero_crossing(&mono, center).map(|i| window_start + i as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_crossing_finds_nearest_sign_change() {
        //            0     1     2    3     4     5
        let s = [0.5, 0.4, -0.1, -0.2, 0.3, 0.4];
        // Crossings occur entering index 2 and index 4.
        assert_eq!(nearest_zero_crossing(&s, 1), Some(2));
        assert_eq!(nearest_zero_crossing(&s, 5), Some(4));
    }

    #[test]
    fn zero_crossing_none_when_constant_sign() {
        let s = [0.5, 0.4, 0.3, 0.2];
        assert_eq!(nearest_zero_crossing(&s, 2), None);
    }

    #[test]
    fn zero_crossing_handles_tiny_input() {
        assert_eq!(nearest_zero_crossing(&[], 0), None);
        assert_eq!(nearest_zero_crossing(&[0.1], 0), None);
    }

    fn clip(path: &str, start: u64, dur: u64) -> ClipInfo {
        ClipInfo {
            path: path.into(),
            name: path.into(),
            start_sample: start,
            duration_samples: dur,
        }
    }

    #[test]
    fn clip_segments_maps_ranges() {
        let clips = vec![clip("a", 0, 1000), clip("b", 1000, 500), clip("c", 1500, 1000)];
        // Fully inside one clip.
        assert_eq!(clip_segments(&clips, 100, 900), vec![(0, 100, 900)]);
        // Crossing one boundary.
        assert_eq!(
            clip_segments(&clips, 800, 1200),
            vec![(0, 800, 1000), (1, 0, 200)]
        );
        // Spanning all three clips.
        assert_eq!(
            clip_segments(&clips, 500, 2000),
            vec![(0, 500, 1000), (1, 0, 500), (2, 0, 500)]
        );
        // Exactly on a boundary: no empty segment.
        assert_eq!(clip_segments(&clips, 1000, 1500), vec![(1, 0, 500)]);
    }
}
