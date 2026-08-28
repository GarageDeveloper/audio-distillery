//! The TARGET timeline: the album as it will be delivered — tracks in
//! order with the resolved gaps between them. Everything here is DERIVED
//! from the recipe (regions + gap settings); nothing is persisted, and
//! the working (source) timeline is never touched.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::audio::AudioInfo;
use crate::project::{ProjectState, TrackInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct AlbumTrack {
    /// Region id (stable key).
    pub id: u32,
    pub number: u32,
    pub title: String,
    /// Start position in ALBUM time (samples at the session rate).
    #[ts(type = "number")]
    pub start_sample: u64,
    #[ts(type = "number")]
    pub length_samples: u64,
    /// Resolved gap before this track (0 for track 1), in ms.
    pub gap_before_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct AlbumLayout {
    pub tracks: Vec<AlbumTrack>,
    #[ts(type = "number")]
    pub total_samples: u64,
}

pub fn gap_samples(ms: u32, sample_rate: u32) -> u64 {
    ms as u64 * sample_rate as u64 / 1000
}

/// Lay the tracks out in album time: each track preceded by its resolved
/// gap, first track at 0.
pub fn album_layout(tracks: &[TrackInfo], sample_rate: u32) -> AlbumLayout {
    let mut pos = 0u64;
    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        pos += gap_samples(t.gap_before_effective_ms, sample_rate);
        let len = t.end_sample.saturating_sub(t.start_sample);
        out.push(AlbumTrack {
            id: t.id,
            number: t.number,
            title: t.title.clone(),
            start_sample: pos,
            length_samples: len,
            gap_before_ms: t.gap_before_effective_ms,
        });
        pos += len;
    }
    AlbumLayout {
        tracks: out,
        total_samples: pos,
    }
}

impl ProjectState {
    /// The derived album layout for the current recipe.
    pub fn album_layout(&self) -> AlbumLayout {
        album_layout(&self.tracks(), self.info.sample_rate)
    }
}

/// One item of a layer's ALBUM program: silence, or a slice of a source
/// file starting at `source_offset` samples into the file.
#[derive(Debug, Clone, PartialEq)]
pub enum AlbumItem {
    Gap {
        samples: u64,
    },
    Slice {
        path: String,
        source_offset: u64,
        samples: u64,
    },
}

/// Per-layer album playlists: for each track in album order, its gap
/// then the layer's clip segments intersecting the track's SOURCE span —
/// silence wherever the layer has no clip under the track (take gaps).
/// `spans` are the tracks' source spans, in the same order as the
/// layout's tracks.
pub fn album_playlists(
    info: &AudioInfo,
    layout: &AlbumLayout,
    spans: &[(u64, u64)],
) -> Vec<Vec<AlbumItem>> {
    let mut out = Vec::with_capacity(info.layers.len());
    for layer in &info.layers {
        let mut items: Vec<AlbumItem> = Vec::new();
        let push_gap = |items: &mut Vec<AlbumItem>, samples: u64| {
            if samples == 0 {
                return;
            }
            if let Some(AlbumItem::Gap { samples: g }) = items.last_mut() {
                *g += samples;
            } else {
                items.push(AlbumItem::Gap { samples });
            }
        };
        for (track, (s, e)) in layout.tracks.iter().zip(spans) {
            push_gap(
                &mut items,
                gap_samples(track.gap_before_ms, info.sample_rate),
            );
            // Walk the layer's clips across [s, e), filling holes with
            // silence so every lane stays exactly track-length.
            let mut cursor = *s;
            for clip in &layer.clips {
                let cs = clip.start_sample;
                let ce = cs + clip.duration_samples;
                if ce <= cursor || cs >= *e {
                    continue;
                }
                let seg_start = cursor.max(cs);
                if seg_start > cursor {
                    push_gap(&mut items, seg_start - cursor);
                }
                let seg_end = ce.min(*e);
                items.push(AlbumItem::Slice {
                    path: clip.path.clone(),
                    source_offset: seg_start - cs,
                    samples: seg_end - seg_start,
                });
                cursor = seg_end;
            }
            if cursor < *e {
                push_gap(&mut items, *e - cursor);
            }
        }
        out.push(items);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{ClipInfo, ScannedLayer};

    const SR: u32 = 44_100;
    const SEC: u64 = SR as u64;

    fn info_two_clips() -> AudioInfo {
        let clips = vec![
            ClipInfo {
                path: "/a.wav".into(),
                name: "a.wav".into(),
                start_sample: 0,
                duration_samples: 10 * SEC,
            },
            ClipInfo {
                path: "/b.wav".into(),
                name: "b.wav".into(),
                start_sample: 10 * SEC,
                duration_samples: 10 * SEC,
            },
        ];
        AudioInfo {
            path: "/a.wav".into(),
            clips: clips.clone(),
            layers: vec![ScannedLayer {
                clips,
                channels: 2,
                duration_samples: 20 * SEC,
            }],
            duration_samples: 20 * SEC,
            sample_rate: SR,
            channels: 2,
            format: "WAV".into(),
            duration_seconds: 20.0,
        }
    }

    fn track(id: u32, number: u32, start: u64, end: u64, gap: u32) -> TrackInfo {
        TrackInfo {
            id,
            number,
            title: format!("T{number}"),
            start_sample: start,
            end_sample: end,
            duration_seconds: (end - start) as f64 / SR as f64,
            gain_overrides: Default::default(),
            mute_overrides: Default::default(),
            solo_overrides: Default::default(),
            layer_volumes: vec![1.0],
            isrc: String::new(),
            gap_before_ms: None,
            gap_before_effective_ms: gap,
            inserts: Vec::new(),
        }
    }

    #[test]
    fn album_layout_positions() {
        let tracks = vec![
            track(1, 1, SEC, 4 * SEC, 0),
            track(2, 2, 5 * SEC, 8 * SEC, 2_000),
            track(3, 3, 9 * SEC, 12 * SEC, 0), // segue override
        ];
        let l = album_layout(&tracks, SR);
        assert_eq!(l.tracks[0].start_sample, 0);
        assert_eq!(l.tracks[1].start_sample, 3 * SEC + 2 * SEC);
        assert_eq!(l.tracks[2].start_sample, 8 * SEC);
        assert_eq!(l.total_samples, 11 * SEC);
    }

    #[test]
    fn album_playlists_slices_and_gaps() {
        let info = info_two_clips();
        // Track 1 crosses the clip boundary; track 2 lives in clip 2.
        let tracks = vec![
            track(1, 1, 8 * SEC, 12 * SEC, 0),
            track(2, 2, 14 * SEC, 16 * SEC, 2_000),
        ];
        let layout = album_layout(&tracks, SR);
        let spans: Vec<(u64, u64)> =
            tracks.iter().map(|t| (t.start_sample, t.end_sample)).collect();
        let pls = album_playlists(&info, &layout, &spans);
        assert_eq!(pls.len(), 1);
        assert_eq!(
            pls[0],
            vec![
                AlbumItem::Slice { path: "/a.wav".into(), source_offset: 8 * SEC, samples: 2 * SEC },
                AlbumItem::Slice { path: "/b.wav".into(), source_offset: 0, samples: 2 * SEC },
                AlbumItem::Gap { samples: 2 * SEC },
                AlbumItem::Slice { path: "/b.wav".into(), source_offset: 4 * SEC, samples: 2 * SEC },
            ]
        );
        // Total program length per lane == album total.
        let sum: u64 = pls[0]
            .iter()
            .map(|i| match i {
                AlbumItem::Gap { samples } | AlbumItem::Slice { samples, .. } => *samples,
            })
            .sum();
        assert_eq!(sum, layout.total_samples);
    }
}
