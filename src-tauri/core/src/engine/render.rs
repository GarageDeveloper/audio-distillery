//! The render side of the engine: decode → per-layer inserts → volume
//! automation → sum → master inserts → master ring buffer. Runs on its own
//! thread with ~90 ms of slack — this is where AU/VST3 plugins will process
//! (phase B), NOT in the hard-realtime device callback.

use crate::engine::decode::LayerDecoder;
use crate::engine::VolumeAutomation;

/// Block-processing insert. Future plugin hosts (AU, VST3) implement this;
/// the engine already routes every layer and the master bus through their
/// insert slots so plugins can attach per layer, per track (chain swapped at
/// region boundaries via the automation mechanism) or on the master without
/// re-architecture.
pub trait BlockProcessor: Send {
    /// In-place processing of interleaved f32 frames.
    fn process(&mut self, buffer: &mut [f32], channels: usize, sample_rate: u32);
    /// Reported latency, for future compensation.
    fn latency_samples(&self) -> u32 {
        0
    }
    /// Clear internal state (called on seek).
    fn reset(&mut self);
    /// Serialize the processor's full state (plugins: preset blob).
    fn save_state(&self) -> Option<Vec<u8>> {
        None
    }
    /// Live bypass without rebuilding the processor.
    fn set_bypassed(&mut self, _bypassed: bool) {}
    /// Raw native handle (AudioUnit pointer) for editor windows; 0 = none.
    fn raw_handle(&self) -> usize {
        0
    }
    /// Restore a previously saved state blob; false = unsupported/failed.
    fn restore_state(&mut self, _state: &[u8]) -> bool {
        false
    }
    /// Downcast hook for format-specific host features (native editors).
    fn as_any(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
}

/// Insert proxy sharing a processor between the MAIN thread (lifecycle:
/// creation, state, disposal — where AU plugins expect to live) and the
/// render thread (processing only). Render uses try_lock and passes the
/// dry signal for the rare blocks where a lifecycle operation holds the
/// lock — never blocking the audio path.
pub struct SharedInsert {
    pub inner: std::sync::Arc<std::sync::Mutex<Box<dyn BlockProcessor>>>,
}

impl BlockProcessor for SharedInsert {
    fn process(&mut self, buffer: &mut [f32], channels: usize, sample_rate: u32) {
        if let Ok(mut p) = self.inner.try_lock() {
            p.process(buffer, channels, sample_rate);
        }
    }
    fn latency_samples(&self) -> u32 {
        self.inner.try_lock().map(|p| p.latency_samples()).unwrap_or(0)
    }
    fn reset(&mut self) {
        if let Ok(mut p) = self.inner.try_lock() {
            p.reset();
        }
    }
}

pub const BLOCK_FRAMES: usize = 512;

/// Per-block volume smoothing to avoid zipper noise on fader moves and
/// region-boundary jumps: ramp linearly across the block towards the target.
struct SmoothedVolume {
    current: f32,
}

pub struct LayerLane {
    pub decoder: LayerDecoder,
    /// Insert chain for THIS layer (empty in phase A).
    pub inserts: Vec<Box<dyn BlockProcessor>>,
    smoothed: SmoothedVolume,
    scratch: Vec<f32>,
}

/// One insert chain on the master bus. `span: None` = always active (the
/// global mastering chain); `Some((start, end))` = a track's chain, active
/// only while the playhead is inside the span (gated per block, reset when
/// (re)entering so the chain starts clean at the track boundary).
pub struct InsertSection {
    pub span: Option<(u64, u64)>,
    pub inserts: Vec<Box<dyn BlockProcessor>>,
    was_active: bool,
}

impl InsertSection {
    pub fn new(span: Option<(u64, u64)>, inserts: Vec<Box<dyn BlockProcessor>>) -> Self {
        Self {
            span,
            inserts,
            was_active: false,
        }
    }

    fn active_at(&self, pos: u64) -> bool {
        match self.span {
            None => true,
            Some((start, end)) => pos >= start && pos < end,
        }
    }
}

/// Owns everything needed to render the session mix block by block.
pub struct Renderer {
    pub lanes: Vec<LayerLane>,
    /// Master-bus insert sections, in processing order: the active tracks'
    /// chains first, then the always-on mastering chain.
    pub master_sections: Vec<InsertSection>,
    pub automation: VolumeAutomation,
    pub sample_rate: u32,
    pub channels: usize,
    /// Next timeline sample to render.
    pub pos: u64,
    pub total_samples: u64,
}

impl Renderer {
    pub fn new(
        decoders: Vec<LayerDecoder>,
        automation: VolumeAutomation,
        sample_rate: u32,
        channels: usize,
        total_samples: u64,
    ) -> Self {
        let channels = channels.max(1);
        let lanes = decoders
            .into_iter()
            .map(|decoder| LayerLane {
                decoder,
                inserts: Vec::new(),
                smoothed: SmoothedVolume { current: 1.0 },
                scratch: vec![0.0; BLOCK_FRAMES * channels],
            })
            .collect();
        Self {
            lanes,
            master_sections: Vec::new(),
            automation,
            sample_rate,
            channels,
            pos: 0,
            total_samples,
        }
    }

    pub fn seek(&mut self, target: u64) {
        let target = target.min(self.total_samples);
        for lane in &mut self.lanes {
            lane.decoder.seek(target);
            for p in &mut lane.inserts {
                p.reset();
            }
        }
        for section in &mut self.master_sections {
            for p in &mut section.inserts {
                p.reset();
            }
            section.was_active = false;
        }
        self.pos = target;
    }

    pub fn finished(&self) -> bool {
        self.pos >= self.total_samples
    }

    /// Idle pump: silence processed through the MASTER chain, without
    /// advancing the timeline. Real hosts render continuously even with the
    /// transport stopped — plugins (iZotope cores reloading on preset
    /// changes, reverb tails, meters) depend on that constant pumping.
    pub fn render_idle_block(&mut self, out: &mut [f32], frames: usize) -> usize {
        let frames = frames.min(BLOCK_FRAMES);
        let ch = self.channels;
        let out = &mut out[..frames * ch];
        out.fill(0.0);
        let pos = self.pos;
        for section in &mut self.master_sections {
            if section.active_at(pos) {
                for p in &mut section.inserts {
                    p.process(out, ch, self.sample_rate);
                }
            }
        }
        frames
    }

    /// Render up to `frames` frames into `out` (interleaved, session
    /// channels). Returns the frame count actually rendered (0 at the end).
    pub fn render_block(&mut self, out: &mut [f32], frames: usize) -> usize {
        if self.finished() {
            return 0;
        }
        let frames = frames
            .min(BLOCK_FRAMES)
            .min((self.total_samples - self.pos) as usize);
        let ch = self.channels;
        let out = &mut out[..frames * ch];
        out.fill(0.0);

        let secs = self.pos as f64 / self.sample_rate.max(1) as f64;
        let volumes = self.automation.volumes_at(secs).clone();

        for (li, lane) in self.lanes.iter_mut().enumerate() {
            let target = volumes.get(li).copied().unwrap_or(1.0);
            let start = lane.smoothed.current;
            lane.decoder.read(&mut lane.scratch[..frames * ch], frames);
            for p in &mut lane.inserts {
                p.process(&mut lane.scratch[..frames * ch], ch, self.sample_rate);
            }
            if (start - target).abs() < 1e-6 {
                if target != 0.0 {
                    for (o, s) in out.iter_mut().zip(&lane.scratch[..frames * ch]) {
                        *o += s * target;
                    }
                }
            } else {
                // Linear ramp across the block.
                for f in 0..frames {
                    let g = start + (target - start) * ((f + 1) as f32 / frames as f32);
                    for c in 0..ch {
                        out[f * ch + c] += lane.scratch[f * ch + c] * g;
                    }
                }
            }
            lane.smoothed.current = target;
        }

        let pos = self.pos;
        for section in &mut self.master_sections {
            let active = section.active_at(pos);
            if active && !section.was_active {
                // Entering the span: the chain starts clean.
                for p in &mut section.inserts {
                    p.reset();
                }
            }
            if active {
                for p in &mut section.inserts {
                    p.process(out, ch, self.sample_rate);
                }
            }
            section.was_active = active;
        }
        self.pos += frames as u64;
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::decode::PlayItem;
    use std::path::PathBuf;

    const SR: u32 = 44_100;

    fn write_wav(path: &PathBuf, secs: f32, amp: f32, channels: u16) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: SR,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for _ in 0..(secs * SR as f32) as usize {
            for _ in 0..channels {
                w.write_sample((amp * i16::MAX as f32) as i16).unwrap();
            }
        }
        w.finalize().unwrap();
    }

    fn automation(default: Vec<f32>, spans: Vec<(f64, f64, Vec<f32>)>) -> VolumeAutomation {
        VolumeAutomation { default, spans }
    }

    #[test]
    fn mixes_two_layers_with_volumes() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        write_wav(&a, 1.0, 0.5, 2);
        write_wav(&b, 1.0, 0.25, 1); // mono → upmixed
        let n = SR as u64;
        let d1 = LayerDecoder::new(vec![PlayItem::File { path: a, samples: n }], 2);
        let d2 = LayerDecoder::new(vec![PlayItem::File { path: b, samples: n }], 2);
        let mut r = Renderer::new(
            vec![d1, d2],
            automation(vec![1.0, 1.0], vec![]),
            SR,
            2,
            n,
        );
        let mut out = vec![0.0f32; BLOCK_FRAMES * 2];
        // Skip the first block (volume ramp from init) then check the sum.
        r.render_block(&mut out, BLOCK_FRAMES);
        let got = r.render_block(&mut out, BLOCK_FRAMES);
        assert_eq!(got, BLOCK_FRAMES);
        // 0.5 + 0.25 = 0.75 on both channels.
        assert!((out[10] - 0.75).abs() < 0.01, "{}", out[10]);
        assert!((out[11] - 0.75).abs() < 0.01, "{}", out[11]);
    }

    #[test]
    fn take_gaps_render_silence_and_keep_alignment() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        write_wav(&a, 0.5, 0.5, 1);
        let half = SR as u64 / 2;
        let d = LayerDecoder::new(
            vec![
                PlayItem::File { path: a.clone(), samples: half },
                PlayItem::Silence { samples: half },
                PlayItem::File { path: a, samples: half },
            ],
            1,
        );
        let total = half * 3;
        let mut r = Renderer::new(vec![d], automation(vec![1.0], vec![]), SR, 1, total);
        // Seek into the middle of the gap: pure silence.
        r.seek(half + half / 2);
        let mut out = vec![0.0f32; BLOCK_FRAMES];
        r.render_block(&mut out, BLOCK_FRAMES);
        assert!(out.iter().all(|&v| v == 0.0));
        // Seek into the second file: signal again (skip ramp block).
        r.seek(2 * half + 100);
        r.render_block(&mut out, BLOCK_FRAMES);
        r.render_block(&mut out, BLOCK_FRAMES);
        assert!((out[5] - 0.5).abs() < 0.01, "{}", out[5]);
    }

    #[test]
    fn automation_spans_apply_by_position() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        write_wav(&a, 2.0, 0.5, 1);
        let n = 2 * SR as u64;
        let d = LayerDecoder::new(vec![PlayItem::File { path: a, samples: n }], 1);
        // Track region [1s, 2s) mutes the layer.
        let auto = automation(vec![1.0], vec![(1.0, 2.0, vec![0.0])]);
        let mut r = Renderer::new(vec![d], auto, SR, 1, n);
        let mut out = vec![0.0f32; BLOCK_FRAMES];
        // At 0.5 s: audible.
        r.seek(SR as u64 / 2);
        r.render_block(&mut out, BLOCK_FRAMES);
        r.render_block(&mut out, BLOCK_FRAMES);
        assert!((out[0] - 0.5).abs() < 0.01);
        // At 1.5 s: override span → silence (after the ramp block).
        r.seek(3 * SR as u64 / 2);
        r.render_block(&mut out, BLOCK_FRAMES);
        r.render_block(&mut out, BLOCK_FRAMES);
        assert!(out.iter().all(|&v| v.abs() < 1e-3));
    }

    #[test]
    fn master_insert_processes_the_sum() {
        struct Half;
        impl BlockProcessor for Half {
            fn process(&mut self, buffer: &mut [f32], _ch: usize, _sr: u32) {
                for v in buffer {
                    *v *= 0.5;
                }
            }
            fn reset(&mut self) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        write_wav(&a, 1.0, 0.8, 1);
        let n = SR as u64;
        let d = LayerDecoder::new(vec![PlayItem::File { path: a, samples: n }], 1);
        let mut r = Renderer::new(vec![d], automation(vec![1.0], vec![]), SR, 1, n);
        r.master_sections
            .push(InsertSection::new(None, vec![Box::new(Half)]));
        let mut out = vec![0.0f32; BLOCK_FRAMES];
        r.render_block(&mut out, BLOCK_FRAMES);
        r.render_block(&mut out, BLOCK_FRAMES);
        assert!((out[0] - 0.4).abs() < 0.01, "{}", out[0]);
    }

    #[test]
    fn renders_exactly_to_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        write_wav(&a, 0.1, 0.5, 1);
        let n = (0.1 * SR as f32) as u64;
        let d = LayerDecoder::new(vec![PlayItem::File { path: a, samples: n }], 1);
        let mut r = Renderer::new(vec![d], automation(vec![1.0], vec![]), SR, 1, n);
        let mut out = vec![0.0f32; BLOCK_FRAMES];
        let mut rendered = 0u64;
        loop {
            let got = r.render_block(&mut out, BLOCK_FRAMES);
            if got == 0 {
                break;
            }
            rendered += got as u64;
        }
        assert_eq!(rendered, n);
        assert!(r.finished());
    }

    /// Fake insert counting process/reset calls and tagging the signal.
    struct Probe {
        processed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        resets: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        add: f32,
    }
    impl BlockProcessor for Probe {
        fn process(&mut self, buffer: &mut [f32], _ch: usize, _sr: u32) {
            self.processed
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            for s in buffer.iter_mut() {
                *s += self.add;
            }
        }
        fn reset(&mut self) {
            self.resets
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn probe(add: f32) -> (
        Box<dyn BlockProcessor>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let processed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resets = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        (
            Box::new(Probe {
                processed: processed.clone(),
                resets: resets.clone(),
                add,
            }),
            processed,
            resets,
        )
    }

    /// A spanned section processes only inside its span, resets when
    /// entering it, and an unspanned section runs everywhere.
    #[test]
    fn sections_gate_by_span_and_reset_on_entry() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        write_wav(&a, 1.0, 0.0, 1); // silence: only the probes' `add` shows
        let n = SR as u64;
        let d = LayerDecoder::new(vec![PlayItem::File { path: a.clone(), samples: n }], 1);
        let mut r = Renderer::new(vec![d], automation(vec![1.0], vec![]), SR, 1, n);

        let span = (BLOCK_FRAMES as u64 * 2, BLOCK_FRAMES as u64 * 4);
        let (track_probe, track_proc, track_resets) = probe(0.25);
        let (master_probe, master_proc, _) = probe(0.0);
        r.master_sections.push(InsertSection::new(Some(span), vec![track_probe]));
        r.master_sections.push(InsertSection::new(None, vec![master_probe]));

        let mut out = vec![0.0f32; BLOCK_FRAMES];
        let mut per_block = Vec::new();
        for _ in 0..6 {
            out.fill(0.0);
            r.render_block(&mut out, BLOCK_FRAMES);
            per_block.push(out[0]);
        }
        // Blocks 0-1 before the span, 2-3 inside, 4-5 after.
        assert!(per_block[0].abs() < 1e-6 && per_block[1].abs() < 1e-6);
        assert!((per_block[2] - 0.25).abs() < 1e-6 && (per_block[3] - 0.25).abs() < 1e-6);
        assert!(per_block[4].abs() < 1e-6 && per_block[5].abs() < 1e-6);
        assert_eq!(track_proc.load(Ordering::Relaxed), 2);
        assert_eq!(track_resets.load(Ordering::Relaxed), 1, "reset on span entry");
        assert_eq!(master_proc.load(Ordering::Relaxed), 6, "master always runs");

        // Seek back before the span: re-entering must reset again.
        r.seek(0);
        for _ in 0..3 {
            out.fill(0.0);
            r.render_block(&mut out, BLOCK_FRAMES);
        }
        assert_eq!(track_resets.load(Ordering::Relaxed), 3, "seek + re-entry");
    }

    /// Per-lane inserts run pre-fader on their own lane only.
    #[test]
    fn lane_inserts_touch_only_their_layer() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.wav");
        let b = dir.path().join("b.wav");
        write_wav(&a, 0.5, 0.0, 1);
        write_wav(&b, 0.5, 0.0, 1);
        let n = (0.5 * SR as f32) as u64;
        let da = LayerDecoder::new(vec![PlayItem::File { path: a, samples: n }], 1);
        let db = LayerDecoder::new(vec![PlayItem::File { path: b, samples: n }], 1);
        // Layer 0 gain 1.0, layer 1 muted (0.0).
        let mut r = Renderer::new(vec![da, db], automation(vec![1.0, 0.0], vec![]), SR, 1, n);
        let (p0, c0, _) = probe(0.5);
        let (p1, c1, _) = probe(1.0);
        r.lanes[0].inserts = vec![p0];
        r.lanes[1].inserts = vec![p1];
        let mut out = vec![0.0f32; BLOCK_FRAMES];
        // Block 0 ramps the initial 1.0 smoothed volume down; block 1 is
        // settled: lane 1 muted post-insert, its +1.0 never reaches the mix.
        r.render_block(&mut out, BLOCK_FRAMES);
        out.fill(0.0);
        r.render_block(&mut out, BLOCK_FRAMES);
        assert!((out[0] - 0.5).abs() < 1e-6, "{}", out[0]);
        assert_eq!(c0.load(Ordering::Relaxed), 2);
        assert_eq!(c1.load(Ordering::Relaxed), 2, "muted lanes keep pumping");
    }
}
