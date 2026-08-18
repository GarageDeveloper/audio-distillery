use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::peaks::PeakPyramid;
use crate::project::RegionSpan;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct SilenceParams {
    /// Level under which a bucket counts as silent, in dBFS (e.g. -40).
    pub threshold_db: f32,
    /// Minimum silence length for a gap to separate two tracks.
    pub min_silence_ms: u32,
    /// Sound spans shorter than this are not proposed as tracks.
    pub min_track_seconds: u32,
}

impl Default for SilenceParams {
    fn default() -> Self {
        Self {
            threshold_db: -40.0,
            min_silence_ms: 1500,
            min_track_seconds: 15,
        }
    }
}

/// Propose track regions: the sound spans that remain between qualifying
/// silences (leading/trailing silence and inter-track gaps are excluded —
/// they are exactly what the export ignores). Works on the already-computed
/// peak pyramid; resolution is one bucket (256 samples ≈ 6 ms), far finer
/// than any realistic gap, and the user fine-tunes edges anyway.
pub fn detect_track_regions(
    pyramid: &PeakPyramid,
    sample_rate: u32,
    duration_samples: u64,
    params: &SilenceParams,
) -> Vec<RegionSpan> {
    let Some(level) = pyramid.levels.first() else {
        return Vec::new();
    };
    let spb = level.samples_per_bucket as u64;
    let bucket_count = level.channels[0].len() / 2;
    let threshold = 10f32.powf(params.threshold_db / 20.0) * 127.0;
    let min_silence_samples = (params.min_silence_ms as u64 * sample_rate as u64) / 1000;
    let min_track_samples = params.min_track_seconds as u64 * sample_rate as u64;

    // A bucket is silent if every channel's abs peak is below the threshold.
    let is_silent = |b: usize| -> bool {
        level.channels.iter().all(|ch| {
            let mn = ch[b * 2] as f32;
            let mx = ch[b * 2 + 1] as f32;
            mn.abs().max(mx.abs()) < threshold
        })
    };

    // Collect qualifying silence runs (in samples).
    let mut silences: Vec<(u64, u64)> = Vec::new();
    let mut run_start: Option<usize> = None;
    for b in 0..=bucket_count {
        let silent = b < bucket_count && is_silent(b);
        match (silent, run_start) {
            (true, None) => run_start = Some(b),
            (false, Some(start)) => {
                let s = start as u64 * spb;
                let e = (b as u64 * spb).min(duration_samples);
                // A silence run touching either file edge always qualifies
                // (leading/trailing silence should never become a track).
                let at_edge = start == 0 || b == bucket_count;
                if e - s >= min_silence_samples || at_edge {
                    silences.push((s, e));
                }
                run_start = None;
            }
            _ => {}
        }
    }

    // Sound spans = complement of the silences over [0, duration].
    let mut spans = Vec::new();
    let mut cursor = 0u64;
    for (s, e) in &silences {
        if *s > cursor {
            spans.push(RegionSpan { start: cursor, end: *s });
        }
        cursor = cursor.max(*e);
    }
    if cursor < duration_samples {
        spans.push(RegionSpan { start: cursor, end: duration_samples });
    }

    spans
        .into_iter()
        .filter(|r| r.end - r.start >= min_track_samples)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peaks::PeakBuilder;

    const SR: u32 = 44_100;
    const SEC: u64 = SR as u64;

    /// Build a mono signal: segments of (seconds, loud?).
    fn signal(segments: &[(f32, bool)]) -> Vec<f32> {
        let mut out = Vec::new();
        for &(secs, loud) in segments {
            let n = (secs * SR as f32) as usize;
            let amp = if loud { 0.6 } else { 0.001 };
            for i in 0..n {
                out.push(amp * (i as f32 * 0.05).sin());
            }
        }
        out
    }

    fn detect(samples: &[f32], params: &SilenceParams) -> Vec<RegionSpan> {
        let mut b = PeakBuilder::new(1);
        b.push_interleaved(samples);
        let (p, frames) = b.finish();
        detect_track_regions(&p, SR, frames, params)
    }

    fn approx(v: u64, secs: f32) -> bool {
        (v as f64 - (secs as f64 * SR as f64)).abs() < 0.2 * SR as f64
    }

    #[test]
    fn finds_two_tracks_around_a_gap() {
        let samples = signal(&[(20.0, true), (2.0, false), (25.0, true)]);
        let regions = detect(&samples, &SilenceParams::default());
        assert_eq!(regions.len(), 2);
        assert!(approx(regions[0].start, 0.0));
        assert!(approx(regions[0].end, 20.0));
        assert!(approx(regions[1].start, 22.0));
        assert!(approx(regions[1].end, 47.0));
    }

    #[test]
    fn leading_and_trailing_silence_are_excluded() {
        let samples = signal(&[(3.0, false), (20.0, true), (4.0, false)]);
        let regions = detect(&samples, &SilenceParams::default());
        assert_eq!(regions.len(), 1);
        assert!(approx(regions[0].start, 3.0));
        assert!(approx(regions[0].end, 23.0));
    }

    #[test]
    fn short_silences_do_not_split_a_track() {
        let samples = signal(&[(20.0, true), (0.5, false), (20.0, true)]);
        let regions = detect(&samples, &SilenceParams::default());
        assert_eq!(regions.len(), 1);
        assert!(approx(regions[0].end, 40.5));
    }

    #[test]
    fn short_sound_spans_are_dropped() {
        let samples = signal(&[(20.0, true), (2.0, false), (5.0, true), (2.0, false), (20.0, true)]);
        let regions = detect(&samples, &SilenceParams::default());
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn continuous_audio_is_one_track() {
        let samples = signal(&[(30.0, true)]);
        let regions = detect(&samples, &SilenceParams::default());
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start, 0);
        assert_eq!(regions[0].end as f64 / SEC as f64, 30.0);
    }
}
