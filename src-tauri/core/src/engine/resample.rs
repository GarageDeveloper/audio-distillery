//! Streaming sample-rate converter for MONITORING: the renderer always works
//! at the session rate, but the output device dictates its own (WASAPI
//! shared mode refuses anything but the device mix format — the macOS
//! transparency that hid this is a CoreAudio courtesy). Windowed-sinc
//! interpolation (32 taps, Kaiser β≈8): ~-80 dB stopband, far beyond what
//! playback monitoring needs. Export never goes through here.

/// Interleaved streaming resampler with continuous phase across blocks.
pub struct StreamResampler {
    ratio: f64,
    channels: usize,
    /// Fractional read position into the virtual input stream, relative to
    /// the first sample of `hist`.
    pos: f64,
    /// Last `TAPS` input frames kept for filter history (interleaved).
    hist: Vec<f32>,
    table: SincTable,
}

const TAPS: usize = 32;
/// Phase resolution of the precomputed table.
const PHASES: usize = 256;

struct SincTable {
    /// PHASES+1 rows of TAPS coefficients.
    coeffs: Vec<f32>,
}

fn bessel_i0(x: f64) -> f64 {
    // Series expansion, plenty for table building.
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..32 {
        term *= (x / (2.0 * k as f64)) * (x / (2.0 * k as f64));
        sum += term;
    }
    sum
}

impl SincTable {
    fn new(cutoff: f64) -> Self {
        let beta = 8.0;
        let denom = bessel_i0(beta);
        let half = TAPS as f64 / 2.0;
        let mut coeffs = Vec::with_capacity((PHASES + 1) * TAPS);
        for p in 0..=PHASES {
            let frac = p as f64 / PHASES as f64;
            let mut row = [0.0f64; TAPS];
            let mut sum = 0.0;
            for (t, c) in row.iter_mut().enumerate() {
                // Tap positions centered on the fractional read point.
                let x = t as f64 - (half - 1.0) - frac;
                let sinc = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * x * cutoff).sin() / (std::f64::consts::PI * x)
                };
                let w = 1.0 - (x / half).powi(2);
                let win = if w <= 0.0 {
                    0.0
                } else {
                    bessel_i0(beta * w.sqrt()) / denom
                };
                *c = sinc * win;
                sum += *c;
            }
            // Normalize for exact DC gain.
            for c in &mut row {
                coeffs.push((*c / sum) as f32);
            }
        }
        Self { coeffs }
    }

    #[inline]
    fn row(&self, frac: f64) -> &[f32] {
        let p = ((frac * PHASES as f64).round() as usize).min(PHASES);
        &self.coeffs[p * TAPS..(p + 1) * TAPS]
    }
}

impl StreamResampler {
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Self {
        let ratio = out_rate as f64 / in_rate as f64;
        // Downsampling must lower the cutoff below the target Nyquist.
        let cutoff = if ratio < 1.0 { ratio * 0.92 } else { 0.92 };
        Self {
            ratio,
            channels,
            pos: 0.0,
            hist: vec![0.0; TAPS * channels],
            table: SincTable::new(cutoff),
        }
    }

    /// Worst-case output length for `frames` input frames.
    pub fn max_out_frames(&self, frames: usize) -> usize {
        (frames as f64 * self.ratio).ceil() as usize + 2
    }

    /// Clear state (seek): phase and history restart from silence.
    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.hist.fill(0.0);
    }

    /// Resample `input` (interleaved, `frames` frames) into `out`.
    /// Returns the number of output frames produced.
    pub fn process(&mut self, input: &[f32], frames: usize, out: &mut [f32]) -> usize {
        let ch = self.channels;
        // Working buffer = history + new input.
        let mut work = Vec::with_capacity((TAPS + frames) * ch);
        work.extend_from_slice(&self.hist);
        work.extend_from_slice(&input[..frames * ch]);
        let work_frames = TAPS + frames;

        let mut produced = 0;
        // A tap window starting at integer frame `i` needs frames i..i+TAPS.
        while (self.pos.floor() as usize) + TAPS <= work_frames {
            let base = self.pos.floor() as usize;
            let frac = self.pos - base as f64;
            let row = self.table.row(frac);
            for c in 0..ch {
                let mut acc = 0.0f32;
                for (t, k) in row.iter().enumerate() {
                    acc += k * work[(base + t) * ch + c];
                }
                out[produced * ch + c] = acc;
            }
            produced += 1;
            self.pos += 1.0 / self.ratio;
        }

        // Keep the last TAPS frames as history; rebase pos onto them.
        let keep_from = work_frames - TAPS;
        self.hist.copy_from_slice(&work[keep_from * ch..work_frames * ch]);
        self.pos -= keep_from as f64;
        produced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, frames: usize, ch: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; frames * ch];
        for f in 0..frames {
            let s = (2.0 * std::f64::consts::PI * freq * f as f64 / rate as f64).sin() as f32;
            for c in 0..ch {
                v[f * ch + c] = s;
            }
        }
        v
    }

    fn run(in_rate: u32, out_rate: u32, freq: f64) -> (Vec<f32>, usize) {
        let ch = 2;
        let mut rs = StreamResampler::new(in_rate, out_rate, ch);
        let blocks = 40;
        let frames = 512;
        let mut out_all = Vec::new();
        let mut out = vec![0.0f32; rs.max_out_frames(frames) * ch];
        let input = sine(in_rate, freq, frames * blocks, ch);
        for b in 0..blocks {
            let chunk = &input[b * frames * ch..(b + 1) * frames * ch];
            let got = rs.process(chunk, frames, &mut out);
            out_all.extend_from_slice(&out[..got * ch]);
        }
        let n = out_all.len() / ch;
        (out_all, n)
    }

    /// 44.1k → 48k: length ratio and amplitude preserved, no discontinuity.
    #[test]
    fn upsamples_44k1_to_48k() {
        let (out, n) = run(44_100, 48_000, 1000.0);
        let expected = (40 * 512) as f64 * 48_000.0 / 44_100.0;
        assert!((n as f64 - expected).abs() < 64.0, "{n} vs {expected}");
        // RMS of a full-scale sine ≈ 0.707 (skip the warmup).
        let tail = &out[out.len() / 2..];
        let rms = (tail.iter().map(|s| (*s as f64).powi(2)).sum::<f64>()
            / tail.len() as f64)
            .sqrt();
        assert!((rms - 0.707).abs() < 0.01, "rms {rms}");
        // Continuity: max sample-to-sample jump of a 1 kHz sine at 48k
        // stays far below a click.
        let max_jump = tail
            .chunks_exact(2)
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_jump < 0.2, "max jump {max_jump}");
    }

    /// 48k → 44.1k downsampling keeps amplitude too.
    #[test]
    fn downsamples_48k_to_44k1() {
        let (out, n) = run(48_000, 44_100, 1000.0);
        let expected = (40 * 512) as f64 * 44_100.0 / 48_000.0;
        assert!((n as f64 - expected).abs() < 64.0, "{n} vs {expected}");
        let tail = &out[out.len() / 2..];
        let rms = (tail.iter().map(|s| (*s as f64).powi(2)).sum::<f64>()
            / tail.len() as f64)
            .sqrt();
        assert!((rms - 0.707).abs() < 0.01, "rms {rms}");
    }

    /// Equal rates: bit-transparent apart from the fixed filter delay.
    #[test]
    fn unity_ratio_is_transparent() {
        let ch = 2;
        let mut rs = StreamResampler::new(48_000, 48_000, ch);
        let frames = 512;
        let input = sine(48_000, 997.0, frames * 8, ch);
        let mut out = vec![0.0f32; rs.max_out_frames(frames) * ch];
        let mut out_all = Vec::new();
        for b in 0..8 {
            let got = rs.process(&input[b * frames * ch..(b + 1) * frames * ch], frames, &mut out);
            out_all.extend_from_slice(&out[..got * ch]);
        }
        // Compare against the input shifted by the filter's group delay:
        // history primes TAPS zero frames and the window centers on tap
        // half-1+frac, so the net delay is TAPS/2 + 1 frames.
        let delay = TAPS / 2 + 1; // frames
        let n = 2000usize;
        let mut max_err = 0.0f32;
        for f in 500..500 + n {
            let e = (out_all[(f + delay) * ch] - input[f * ch]).abs();
            max_err = max_err.max(e);
        }
        assert!(max_err < 0.01, "max err {max_err}");
    }
}
