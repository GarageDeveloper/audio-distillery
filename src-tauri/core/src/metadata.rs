//! Album metadata: one FORMAT-AGNOSTIC model filled by the user, written to
//! each exported file by lofty, which maps it to the container's native tag
//! format (ID3v2 for MP3, MP4 atoms for M4A/AAC, Vorbis comments for FLAC,
//! RIFF INFO for WAV). Applied to EXPORTED files only — never to sources
//! (SPEC §3 bis).
//!
//! Every text field supports dynamic macros expanded per track:
//! `{title}` `{n}` `{ntotal}` `{disc}` `{dtotal}` `{album}` `{artist}`
//! `{album_artist}` `{date}` `{year}` `{source}`.

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{Tag, TagType};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Result, StillError};

/// User-facing album metadata (the abstract level). Empty fields are not
/// written. All fields may contain macros.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct AlbumMeta {
    pub album: String,
    /// Album-level artist ("Album artist" in players).
    pub album_artist: String,
    /// Track artist; defaults to the album artist when empty.
    pub artist: String,
    /// Free-form date ("2026", "2026-08-01"…). The year is derived from the
    /// first 4 digits for formats that want a separate year field.
    pub date: String,
    pub genre: String,
    pub comment: String,
    /// Track numbers (1-based, in playing order) that START a new disc.
    /// Empty = single disc. Example: [7, 13] → tracks 1-6 = disc 1,
    /// 7-12 = disc 2, 13+ = disc 3.
    pub disc_breaks: Vec<u32>,
}

/// Fully resolved tag values for ONE exported track (macros expanded,
/// numbering computed). This is what gets written to the file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct TrackTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub date: String,
    pub genre: String,
    pub comment: String,
    /// Track number WITHIN its disc (restarts at 1 on each disc).
    pub track: u32,
    /// Number of tracks on this track's disc.
    pub track_total: u32,
    pub disc: u32,
    pub disc_total: u32,
}

/// Per-track numbering derived from the disc breaks: (track-in-disc,
/// tracks-in-that-disc, disc, disc-total) for the album track `number`
/// (1-based, in playing order) out of `total` tracks.
pub fn disc_numbering(disc_breaks: &[u32], number: u32, total: u32) -> (u32, u32, u32, u32) {
    let mut breaks: Vec<u32> = disc_breaks
        .iter()
        .copied()
        .filter(|&b| b >= 2 && b <= total)
        .collect();
    breaks.sort_unstable();
    breaks.dedup();
    // Disc start positions: 1, then each break.
    let mut starts = vec![1u32];
    starts.extend(&breaks);
    let disc_total = starts.len() as u32;
    let disc_idx = starts.iter().rposition(|&s| s <= number).unwrap_or(0);
    let disc_start = starts[disc_idx];
    let disc_end = starts.get(disc_idx + 1).map(|&s| s - 1).unwrap_or(total);
    (
        number - disc_start + 1,
        disc_end - disc_start + 1,
        disc_idx as u32 + 1,
        disc_total,
    )
}

/// Context for macro expansion.
pub struct MacroContext<'a> {
    pub meta: &'a AlbumMeta,
    pub title: &'a str,
    pub source_stem: &'a str,
    /// (track-in-disc, tracks-in-disc, disc, disc-total)
    pub numbering: (u32, u32, u32, u32),
}

fn year_of(date: &str) -> String {
    let digits: String = date.chars().filter(|c| c.is_ascii_digit()).take(4).collect();
    if digits.len() == 4 {
        digits
    } else {
        String::new()
    }
}

/// Expand the dynamic macros in `template`. Numeric macros are zero-padded
/// to the width of their total (min 2), matching the file-naming style.
pub fn expand_macros(template: &str, ctx: &MacroContext) -> String {
    let (n, ntotal, disc, dtotal) = ctx.numbering;
    let width = ntotal.to_string().len().max(2);
    let artist = if ctx.meta.artist.is_empty() {
        &ctx.meta.album_artist
    } else {
        &ctx.meta.artist
    };
    template
        .replace("{title}", ctx.title)
        .replace("{titre}", ctx.title)
        .replace("{n}", &format!("{n:0width$}"))
        .replace("{ntotal}", &ntotal.to_string())
        .replace("{disc}", &disc.to_string())
        .replace("{dtotal}", &dtotal.to_string())
        .replace("{album}", &ctx.meta.album)
        .replace("{album_artist}", &ctx.meta.album_artist)
        .replace("{artist}", artist)
        .replace("{date}", &ctx.meta.date)
        .replace("{year}", &year_of(&ctx.meta.date))
        .replace("{source}", ctx.source_stem)
}

/// Resolve the final tag values for one track.
pub fn resolve_tags(
    meta: &AlbumMeta,
    title: &str,
    source_stem: &str,
    number: u32,
    total: u32,
) -> TrackTags {
    let numbering = disc_numbering(&meta.disc_breaks, number, total);
    let ctx = MacroContext {
        meta,
        title,
        source_stem,
        numbering,
    };
    let artist_template = if meta.artist.is_empty() {
        meta.album_artist.clone()
    } else {
        meta.artist.clone()
    };
    TrackTags {
        title: expand_macros(title, &ctx),
        artist: expand_macros(&artist_template, &ctx),
        album: expand_macros(&meta.album, &ctx),
        album_artist: expand_macros(&meta.album_artist, &ctx),
        date: meta.date.clone(),
        genre: meta.genre.clone(),
        comment: expand_macros(&meta.comment, &ctx),
        track: numbering.0,
        track_total: numbering.1,
        disc: numbering.2,
        disc_total: numbering.3,
    }
}

/// True when nothing would be written (all text empty → skip tagging;
/// track numbers alone are still worth writing when an album is set).
pub fn is_empty_meta(meta: &AlbumMeta) -> bool {
    meta.album.is_empty()
        && meta.album_artist.is_empty()
        && meta.artist.is_empty()
        && meta.date.is_empty()
        && meta.genre.is_empty()
        && meta.comment.is_empty()
}

/// Write the resolved tags into an EXPORTED file (never a source). lofty
/// picks the container's primary tag format and does the native mapping.
pub fn write_tags(path: &Path, tags: &TrackTags) -> Result<()> {
    let mut tagged = Probe::open(path)
        .map_err(|e| StillError::Io(std::io::Error::other(e.to_string())))?
        .read()
        .map_err(|e| {
            StillError::InvalidProject(format!(
                "{}: cannot read for tagging: {e}",
                path.display()
            ))
        })?;

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged.primary_tag_mut().expect("tag inserted above");

    let set = |tag: &mut Tag, key: ItemKey, value: &str| {
        if !value.is_empty() {
            tag.insert_text(key, value.to_string());
        }
    };
    set(tag, ItemKey::TrackTitle, &tags.title);
    set(tag, ItemKey::TrackArtist, &tags.artist);
    set(tag, ItemKey::AlbumTitle, &tags.album);
    set(tag, ItemKey::AlbumArtist, &tags.album_artist);
    set(tag, ItemKey::RecordingDate, &tags.date);
    set(tag, ItemKey::Year, &year_of(&tags.date));
    set(tag, ItemKey::Genre, &tags.genre);
    set(tag, ItemKey::Comment, &tags.comment);
    tag.set_track(tags.track);
    tag.set_track_total(tags.track_total);
    // RIFF INFO (WAV) has no disc fields; lofty simply skips unsupported
    // keys, so writing them is always safe.
    if tag_type != TagType::RiffInfo {
        tag.set_disk(tags.disc);
        tag.set_disk_total(tags.disc_total);
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| {
            StillError::InvalidProject(format!("{}: cannot write tags: {e}", path.display()))
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> AlbumMeta {
        AlbumMeta {
            album: "Live at the Barn".into(),
            album_artist: "The Copper Stills".into(),
            artist: "".into(),
            date: "2026-08-01".into(),
            genre: "Rock".into(),
            comment: "".into(),
            disc_breaks: vec![7, 13],
        }
    }

    #[test]
    fn disc_numbering_splits_at_breaks() {
        let b = vec![7, 13];
        // 18 tracks: 1-6 disc 1, 7-12 disc 2, 13-18 disc 3.
        assert_eq!(disc_numbering(&b, 1, 18), (1, 6, 1, 3));
        assert_eq!(disc_numbering(&b, 6, 18), (6, 6, 1, 3));
        assert_eq!(disc_numbering(&b, 7, 18), (1, 6, 2, 3));
        assert_eq!(disc_numbering(&b, 12, 18), (6, 6, 2, 3));
        assert_eq!(disc_numbering(&b, 13, 18), (1, 6, 3, 3));
        assert_eq!(disc_numbering(&b, 18, 18), (6, 6, 3, 3));
    }

    #[test]
    fn disc_numbering_defaults_to_single_disc() {
        assert_eq!(disc_numbering(&[], 5, 12), (5, 12, 1, 1));
        // Out-of-range/duplicate/unsorted breaks are cleaned up.
        assert_eq!(disc_numbering(&[99, 1, 7, 7], 8, 12), (2, 6, 2, 2));
    }

    #[test]
    fn macros_expand_everywhere() {
        let m = meta();
        let numbering = disc_numbering(&m.disc_breaks, 8, 18);
        let ctx = MacroContext {
            meta: &m,
            title: "Ashes",
            source_stem: "concert",
            numbering,
        };
        assert_eq!(
            expand_macros("{album} — Disc {disc}/{dtotal}", &ctx),
            "Live at the Barn — Disc 2/3"
        );
        assert_eq!(expand_macros("{n} of {ntotal}", &ctx), "02 of 6");
        assert_eq!(expand_macros("{year}", &ctx), "2026");
        // Empty artist falls back to the album artist.
        assert_eq!(expand_macros("{artist}", &ctx), "The Copper Stills");
    }

    #[test]
    fn resolve_tags_fills_numbering_and_fallbacks() {
        let m = meta();
        let t = resolve_tags(&m, "Boreal", "concert", 13, 18);
        assert_eq!(t.track, 1);
        assert_eq!(t.track_total, 6);
        assert_eq!(t.disc, 3);
        assert_eq!(t.disc_total, 3);
        assert_eq!(t.artist, "The Copper Stills");
        assert_eq!(t.album, "Live at the Barn");
    }

    #[test]
    fn dynamic_album_per_disc() {
        let mut m = meta();
        m.album = "Anthology (Disc {disc})".into();
        let t1 = resolve_tags(&m, "A", "s", 2, 18);
        let t3 = resolve_tags(&m, "B", "s", 14, 18);
        assert_eq!(t1.album, "Anthology (Disc 1)");
        assert_eq!(t3.album, "Anthology (Disc 3)");
    }
}
