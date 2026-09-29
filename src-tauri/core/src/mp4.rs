//! Minimal MP4/M4A box walker: reads the audio track's edit-list start
//! offset (encoder priming, e.g. 1024 frames for AAC).
//!
//! Symphonia parses `elst` but does not apply it, while FFmpeg (which renders
//! the exports) does. Without this, an M4A waveform would sit ~23 ms late
//! relative to the exported audio. Read-only, like every source access.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Frames (at `sample_rate`) the audio track's edit list skips at the start
/// of the stream. 0 when absent, unreadable, or not an MP4 container.
pub(crate) fn lead_skip_frames(path: &Path, sample_rate: u32) -> u64 {
    let Ok(mut f) = File::open(path) else { return 0 };
    let Ok(len) = f.seek(SeekFrom::End(0)) else { return 0 };
    let Some(moov) = find_box(&mut f, 0, len, b"moov") else { return 0 };
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
        let Some(timescale) = mdhd_timescale(&mut f, mdia) else { return 0 };
        let media_time = find_box(&mut f, body, end, b"edts")
            .and_then(|edts| find_box(&mut f, edts.0, edts.1, b"elst"))
            .and_then(|elst| elst_media_time(&mut f, elst.0))
            .unwrap_or(0);
        if media_time <= 0 || timescale == 0 {
            return 0;
        }
        return (media_time as u128 * sample_rate as u128 / timescale as u128) as u64;
    }
    0
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

/// `mdhd` timescale (units per second of the track's media timeline).
fn mdhd_timescale(f: &mut File, mdia: (u64, u64)) -> Option<u32> {
    let (body, _) = find_box(f, mdia.0, mdia.1, b"mdhd")?;
    let version = read_at::<1>(f, body)?[0];
    // version/flags (4) + creation/modification times (2×4 or 2×8).
    let off = if version == 1 { 4 + 16 } else { 4 + 8 };
    Some(u32::from_be_bytes(read_at::<4>(f, body + off)?))
}

/// `media_time` of the first non-empty edit (-1 marks an empty edit).
fn elst_media_time(f: &mut File, body: u64) -> Option<i64> {
    let version = read_at::<1>(f, body)?[0];
    let count = u32::from_be_bytes(read_at::<4>(f, body + 4)?);
    let entry_len = if version == 1 { 20 } else { 12 };
    for i in 0..count.min(16) as u64 {
        let e = body + 8 + i * entry_len;
        let media_time = if version == 1 {
            i64::from_be_bytes(read_at::<8>(f, e + 8)?)
        } else {
            i32::from_be_bytes(read_at::<4>(f, e + 4)?) as i64
        };
        if media_time != -1 {
            return Some(media_time);
        }
    }
    None
}
