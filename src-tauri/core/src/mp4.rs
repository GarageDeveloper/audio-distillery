//! Minimal MP4/M4A box walker: reads the audio track's edit list — where
//! the real audio starts (after encoder priming, e.g. 1024 frames for AAC)
//! and how long it lasts (before the encoder's trailing padding).
//!
//! Symphonia parses `elst` but does not apply it, while FFmpeg (which renders
//! the exports) does. Without this, an M4A waveform would sit ~23 ms late
//! relative to the exported audio. Recent FFmpeg versions also drop the
//! trailing padding and older ones don't: applying the duration here makes
//! the timeline the true audio length either way. Read-only, like every
//! source access.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The audio track's edit, in frames at the track's sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Edit {
    /// Frames to skip at the start of the stream (priming).
    pub(crate) lead_skip: u64,
    /// Frames to play after the skip (None = up to the end of the stream).
    pub(crate) frames: Option<u64>,
}

/// The audio track's first non-empty edit. None when absent, unreadable,
/// or not an MP4 container.
pub(crate) fn audio_edit(path: &Path, sample_rate: u32) -> Option<Edit> {
    let mut f = File::open(path).ok()?;
    let len = f.seek(SeekFrom::End(0)).ok()?;
    let moov = find_box(&mut f, 0, len, b"moov")?;
    // Edit durations are in the MOVIE timescale, media times in the track's.
    let movie_timescale = find_box(&mut f, moov.0, moov.1, b"mvhd")
        .and_then(|mvhd| timescale_at(&mut f, mvhd.0))?;
    let mut pos = moov.0;
    while let Some((body, end, kind)) = next_box(&mut f, pos, moov.1) {
        pos = end;
        if &kind != b"trak" {
            continue;
        }
        let Some(mdia) = find_box(&mut f, body, end, b"mdia") else { continue };
        if handler_type(&mut f, mdia).as_ref() != Some(b"soun") {
            continue;
        }
        let (mdhd, _) = find_box(&mut f, mdia.0, mdia.1, b"mdhd")?;
        let media_timescale = timescale_at(&mut f, mdhd)?;
        let edts = find_box(&mut f, body, end, b"edts")?;
        let elst = find_box(&mut f, edts.0, edts.1, b"elst")?;
        let (duration, media_time) = elst_first_edit(&mut f, elst.0)?;
        let to_frames = |v: u64, scale: u32| {
            (v as u128 * sample_rate as u128 / scale.max(1) as u128) as u64
        };
        return Some(Edit {
            lead_skip: to_frames(media_time.max(0) as u64, media_timescale),
            // A zero duration means "the rest of the media".
            frames: (duration > 0).then(|| to_frames(duration, movie_timescale)),
        });
    }
    None
}

/// Box header at `pos` (bounded by `limit`): (body start, box end, type).
fn next_box(f: &mut File, pos: u64, limit: u64) -> Option<(u64, u64, [u8; 4])> {
    if pos + 8 > limit {
        return None;
    }
    f.seek(SeekFrom::Start(pos)).ok()?;
    let mut h = [0u8; 8];
    f.read_exact(&mut h).ok()?;
    let size32 = u32::from_be_bytes(h[0..4].try_into().ok()?) as u64;
    let kind: [u8; 4] = h[4..8].try_into().ok()?;
    let (body, size) = match size32 {
        0 => (pos + 8, limit - pos),
        1 => {
            let mut b = [0u8; 8];
            f.read_exact(&mut b).ok()?;
            (pos + 16, u64::from_be_bytes(b))
        }
        s => (pos + 8, s),
    };
    let end = pos.checked_add(size)?;
    if size < body - pos || end > limit {
        return None;
    }
    Some((body, end, kind))
}

/// First child box of type `kind` in [start, end): (body start, box end).
fn find_box(f: &mut File, start: u64, end: u64, kind: &[u8; 4]) -> Option<(u64, u64)> {
    let mut pos = start;
    while let Some((body, box_end, k)) = next_box(f, pos, end) {
        if &k == kind {
            return Some((body, box_end));
        }
        pos = box_end;
    }
    None
}

fn read_at<const N: usize>(f: &mut File, pos: u64) -> Option<[u8; N]> {
    f.seek(SeekFrom::Start(pos)).ok()?;
    let mut b = [0u8; N];
    f.read_exact(&mut b).ok()?;
    Some(b)
}

/// `hdlr` handler type ("soun" for audio) of a `mdia` box.
fn handler_type(f: &mut File, mdia: (u64, u64)) -> Option<[u8; 4]> {
    let (body, _) = find_box(f, mdia.0, mdia.1, b"hdlr")?;
    // version/flags (4) + pre_defined (4), then handler_type.
    read_at::<4>(f, body + 8)
}

/// Timescale (units per second) of a `mvhd` or `mdhd` box — same layout.
fn timescale_at(f: &mut File, body: u64) -> Option<u32> {
    let version = read_at::<1>(f, body)?[0];
    // version/flags (4) + creation/modification times (2×4 or 2×8).
    let off = if version == 1 { 4 + 16 } else { 4 + 8 };
    let scale = u32::from_be_bytes(read_at::<4>(f, body + off)?);
    (scale > 0).then_some(scale)
}

/// (segment_duration, media_time) of the first non-empty edit (media_time
/// -1 marks an empty edit).
fn elst_first_edit(f: &mut File, body: u64) -> Option<(u64, i64)> {
    let version = read_at::<1>(f, body)?[0];
    let count = u32::from_be_bytes(read_at::<4>(f, body + 4)?);
    let entry_len = if version == 1 { 20 } else { 12 };
    for i in 0..count.min(16) as u64 {
        let e = body + 8 + i * entry_len;
        let (duration, media_time) = if version == 1 {
            (
                u64::from_be_bytes(read_at::<8>(f, e)?),
                i64::from_be_bytes(read_at::<8>(f, e + 8)?),
            )
        } else {
            (
                u32::from_be_bytes(read_at::<4>(f, e)?) as u64,
                i32::from_be_bytes(read_at::<4>(f, e + 4)?) as i64,
            )
        };
        if media_time != -1 {
            return Some((duration, media_time));
        }
    }
    None
}
