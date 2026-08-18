use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Resolution of the finest peak level. 256 samples ≈ 5.8 ms at 44.1 kHz,
/// enough for sample-ish visual accuracy at deep zoom while keeping memory
/// at ~2.5 MB per hour of stereo audio.
pub const BASE_SAMPLES_PER_BUCKET: u32 = 256;
/// Coarser levels are generated (×2 each) until one fits a whole screen.
pub const MAX_TOP_LEVEL_BUCKETS: usize = 2048;

/// One resolution level of the peak pyramid.
#[derive(Debug, Clone)]
pub struct PeakLevel {
    pub samples_per_bucket: u32,
    /// Per channel: interleaved `[min0, max0, min1, max1, ...]`, quantized to i8
    /// (-127..=127 maps to -1.0..=1.0).
    pub channels: Vec<Vec<i8>>,
}

/// Multi-resolution min/max peaks for a whole file, ordered fine → coarse.
#[derive(Debug, Clone, Default)]
pub struct PeakPyramid {
    pub levels: Vec<PeakLevel>,
}

/// A window of peaks returned to the display layer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PeakSlice {
    pub samples_per_bucket: u32,
    /// Sample position of the first returned bucket.
    #[ts(type = "number")]
    pub start_sample: u64,
    /// Per channel: interleaved `[min, max, ...]` in i8.
    pub channels: Vec<Vec<i8>>,
}

fn quantize(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

/// Streaming accumulator: feed interleaved f32 frames, get a pyramid.
pub struct PeakBuilder {
    channels: usize,
    current: Vec<(f32, f32)>,
    filled: u32,
    base: Vec<Vec<i8>>,
    total_frames: u64,
}

impl PeakBuilder {
    pub fn new(channels: usize) -> Self {
        assert!(channels > 0);
        Self {
            channels,
            current: vec![(f32::INFINITY, f32::NEG_INFINITY); channels],
            filled: 0,
            base: vec![Vec::new(); channels],
            total_frames: 0,
        }
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    pub fn push_interleaved(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            for (c, &s) in frame.iter().enumerate() {
                let b = &mut self.current[c];
                if s < b.0 {
                    b.0 = s;
                }
                if s > b.1 {
                    b.1 = s;
                }
            }
            self.filled += 1;
            self.total_frames += 1;
            if self.filled == BASE_SAMPLES_PER_BUCKET {
                self.flush_bucket();
            }
        }
    }

    fn flush_bucket(&mut self) {
        for c in 0..self.channels {
            let (mn, mx) = self.current[c];
            self.base[c].push(quantize(mn));
            self.base[c].push(quantize(mx));
            self.current[c] = (f32::INFINITY, f32::NEG_INFINITY);
        }
        self.filled = 0;
    }

    pub fn finish(mut self) -> (PeakPyramid, u64) {
        if self.filled > 0 {
            self.flush_bucket();
        }
        let mut levels = vec![PeakLevel {
            samples_per_bucket: BASE_SAMPLES_PER_BUCKET,
            channels: self.base,
        }];
        loop {
            let last = levels.last().unwrap();
            if last.channels[0].len() / 2 <= MAX_TOP_LEVEL_BUCKETS {
                break;
            }
            levels.push(downsample(last));
        }
        (PeakPyramid { levels }, self.total_frames)
    }
}

fn downsample(level: &PeakLevel) -> PeakLevel {
    let channels = level
        .channels
        .iter()
        .map(|ch| {
            let mut out = Vec::with_capacity(ch.len() / 2 + 2);
            let mut i = 0;
            while i + 4 <= ch.len() {
                out.push(ch[i].min(ch[i + 2]));
                out.push(ch[i + 1].max(ch[i + 3]));
                i += 4;
            }
            if i < ch.len() {
                out.push(ch[i]);
                out.push(ch[i + 1]);
            }
            out
        })
        .collect();
    PeakLevel {
        samples_per_bucket: level.samples_per_bucket * 2,
        channels,
    }
}

/// Smallest power-of-two multiple of the base resolution that fits `span`
/// into `max_buckets` buckets. Used so several pyramids (one per layer) can
/// be queried on a COMMON grid and merged bucket-wise.
pub fn target_spb(span: u64, max_buckets: u32) -> u64 {
    let max_buckets = max_buckets.max(1) as u64;
    let mut spb = BASE_SAMPLES_PER_BUCKET as u64;
    while span.div_ceil(spb) > max_buckets {
        spb *= 2;
    }
    spb
}

impl PeakPyramid {
    pub fn channel_count(&self) -> usize {
        self.levels.first().map_or(0, |l| l.channels.len())
    }

    /// Pick the finest level whose bucket count over `[start, end)` fits in
    /// `max_buckets`, and return the matching window of peaks.
    pub fn query(&self, start_sample: u64, end_sample: u64, max_buckets: u32) -> PeakSlice {
        let span = end_sample.saturating_sub(start_sample).max(1);
        self.query_with_spb(target_spb(span, max_buckets), start_sample, end_sample)
    }

    /// Query on an exact grid of `spb` samples per bucket (must be a
    /// power-of-two multiple of the base resolution). Aggregates from the
    /// finest stored level that divides `spb`.
    pub fn query_with_spb(&self, spb: u64, start_sample: u64, end_sample: u64) -> PeakSlice {
        let level = self
            .levels
            .iter()
            .rev()
            .find(|l| {
                let lspb = l.samples_per_bucket as u64;
                lspb <= spb && spb % lspb == 0
            })
            .or_else(|| self.levels.first())
            .expect("pyramid has at least one level");
        let lspb = level.samples_per_bucket as u64;
        let k = (spb / lspb).max(1);
        let bucket_count = (level.channels[0].len() / 2) as u64;
        let first = ((start_sample / spb) * k).min(bucket_count);
        let last = (end_sample.div_ceil(spb) * k).min(bucket_count);
        let channels = level
            .channels
            .iter()
            .map(|ch| {
                let window = &ch[(first as usize * 2)..(last as usize * 2)];
                if k == 1 {
                    return window.to_vec();
                }
                let mut out = Vec::with_capacity(window.len() / k as usize + 2);
                for group in window.chunks(k as usize * 2) {
                    let mut mn = i8::MAX;
                    let mut mx = i8::MIN;
                    for pair in group.chunks_exact(2) {
                        mn = mn.min(pair[0]);
                        mx = mx.max(pair[1]);
                    }
                    out.push(mn);
                    out.push(mx);
                }
                out
            })
            .collect();
        PeakSlice {
            samples_per_bucket: spb.min(u32::MAX as u64) as u32,
            start_sample: (first / k) * spb,
            channels,
        }
    }
}

/// Merge several layers' peaks into one display slice: each layer's values
/// are scaled by its linear gain and summed (then clamped), so the waveform
/// reflects the mix the user is dialing in. A mono layer contributes its
/// single channel to every output channel. Display-only approximation (the
/// true peak of a sum is not the sum of peaks).
pub fn merged_query(
    layers: &[(f32, &PeakPyramid)],
    out_channels: usize,
    start_sample: u64,
    end_sample: u64,
    max_buckets: u32,
) -> PeakSlice {
    let span = end_sample.saturating_sub(start_sample).max(1);
    let spb = target_spb(span, max_buckets);
    let slices: Vec<(f32, PeakSlice)> = layers
        .iter()
        .map(|(g, p)| (*g, p.query_with_spb(spb, start_sample, end_sample)))
        .collect();
    let start_aligned = (start_sample / spb) * spb;
    let bucket_count = slices
        .iter()
        .map(|(_, s)| s.channels.first().map_or(0, |c| c.len() / 2))
        .max()
        .unwrap_or(0);
    let mut channels = vec![vec![0i8; bucket_count * 2]; out_channels];
    for b in 0..bucket_count {
        for c in 0..out_channels {
            let mut mn = 0f32;
            let mut mx = 0f32;
            for (gain, slice) in &slices {
                let Some(ch) = slice
                    .channels
                    .get(c.min(slice.channels.len().saturating_sub(1)))
                else {
                    continue;
                };
                if b * 2 + 1 < ch.len() {
                    mn += ch[b * 2] as f32 * gain;
                    mx += ch[b * 2 + 1] as f32 * gain;
                }
            }
            channels[c][b * 2] = mn.clamp(-127.0, 127.0) as i8;
            channels[c][b * 2 + 1] = mx.clamp(-127.0, 127.0) as i8;
        }
    }
    PeakSlice {
        samples_per_bucket: spb.min(u32::MAX as u64) as u32,
        start_sample: start_aligned,
        channels,
    }
}

/// A single-level pyramid of the merged mix at base resolution — used by
/// silence detection so it sees the same mix as the user.
pub fn merged_base_pyramid(
    layers: &[(f32, &PeakPyramid)],
    out_channels: usize,
    duration_samples: u64,
) -> PeakPyramid {
    let base = BASE_SAMPLES_PER_BUCKET as u64;
    let buckets = duration_samples.div_ceil(base).max(1);
    let slice = merged_query(layers, out_channels, 0, duration_samples, buckets as u32);
    PeakPyramid {
        levels: vec![PeakLevel {
            samples_per_bucket: slice.samples_per_bucket,
            channels: slice.channels,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pyramid_from_mono(samples: &[f32]) -> (PeakPyramid, u64) {
        let mut b = PeakBuilder::new(1);
        b.push_interleaved(samples);
        b.finish()
    }

    #[test]
    fn builds_base_level_and_counts_frames() {
        let samples = vec![0.5f32; 1000];
        let (p, frames) = pyramid_from_mono(&samples);
        assert_eq!(frames, 1000);
        assert_eq!(p.levels[0].samples_per_bucket, BASE_SAMPLES_PER_BUCKET);
        // 1000 samples → 4 buckets (3 full + 1 partial)
        assert_eq!(p.levels[0].channels[0].len(), 8);
        assert_eq!(p.levels[0].channels[0][1], quantize(0.5));
    }

    #[test]
    fn min_max_are_tracked_per_bucket() {
        let mut samples = vec![0.0f32; 256];
        samples[10] = -0.8;
        samples[20] = 0.9;
        let (p, _) = pyramid_from_mono(&samples);
        let ch = &p.levels[0].channels[0];
        assert_eq!(ch[0], quantize(-0.8));
        assert_eq!(ch[1], quantize(0.9));
    }

    #[test]
    fn pyramid_downsamples_until_small() {
        // 2^20 samples → 4096 base buckets → needs one extra level to reach 2048
        let samples = vec![0.1f32; 1 << 20];
        let (p, _) = pyramid_from_mono(&samples);
        assert!(p.levels.len() >= 2);
        let top = p.levels.last().unwrap();
        assert!(top.channels[0].len() / 2 <= MAX_TOP_LEVEL_BUCKETS);
    }

    #[test]
    fn query_picks_matching_resolution() {
        let samples = vec![0.2f32; 1 << 20];
        let (p, frames) = pyramid_from_mono(&samples);
        // Whole file in 1000 buckets → must use a coarse level
        let s = p.query(0, frames, 1000);
        assert!(s.channels[0].len() / 2 <= 1000);
        assert!(s.samples_per_bucket > BASE_SAMPLES_PER_BUCKET);
        // Tiny window → finest level
        let s = p.query(0, 10_000, 1000);
        assert_eq!(s.samples_per_bucket, BASE_SAMPLES_PER_BUCKET);
    }

    #[test]
    fn query_clamps_out_of_range() {
        let samples = vec![0.2f32; 10_000];
        let (p, frames) = pyramid_from_mono(&samples);
        let s = p.query(frames + 5000, frames + 9000, 100);
        assert!(s.channels[0].is_empty());
    }

    #[test]
    fn stereo_channels_are_independent() {
        // L = 0.5, R = -0.5
        let mut samples = Vec::new();
        for _ in 0..512 {
            samples.push(0.5);
            samples.push(-0.5);
        }
        let mut b = PeakBuilder::new(2);
        b.push_interleaved(&samples);
        let (p, frames) = b.finish();
        assert_eq!(frames, 512);
        assert_eq!(p.levels[0].channels.len(), 2);
        assert_eq!(p.levels[0].channels[0][1], quantize(0.5));
        assert_eq!(p.levels[0].channels[1][0], quantize(-0.5));
    }
}
