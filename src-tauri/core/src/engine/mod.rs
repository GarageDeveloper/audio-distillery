//! Realtime audio engine (phase A of the mastering roadmap).
//!
//! Topology:
//!   decode → [Renderer: per-layer inserts → automation → sum → master
//!   inserts] → master ring buffer → cpal device callback
//!
//! The device callback is minimal (pop frames, count consumption) so future
//! AU/VST3 plugin processing runs on the render thread with ~90 ms of slack.
//! The public `PlayerHandle` API is unchanged from the old rodio player, so
//! the Tauri command layer needs no structural change.
//!
//! Ring accounting protocol (all atomics, no locks in the callback):
//! - `ring_written` / `ring_read`: TOTAL frames ever pushed to / popped from
//!   the ring (reads include discards).
//! - Seek: `flush_upto = ring_written` marks everything in flight as stale;
//!   the callback discards until `ring_read == flush_upto`, then plays.
//! - Playhead = `seek_base + consumed` (consumed = frames actually played
//!   since the latest seek).

pub mod decode;
pub mod render;
pub mod resample;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Result, StillError};
use decode::{LayerDecoder, PlayItem};
use render::{Renderer, BLOCK_FRAMES};

/// Spec of one master-chain plugin, sent by the control side.
#[derive(Debug, Clone)]
pub struct MasterPluginSpec {
    pub id: u32,
    pub component: String,
    pub bypass: bool,
    pub state: Option<Vec<u8>>,
}

/// Snapshot of one chain plugin's live state.
pub type ChainStateSnapshot = Vec<(u32, Option<Vec<u8>>)>;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PlaybackState {
    pub playing: bool,
    pub position_seconds: f64,
    /// Output-device failure (no device, format refused…); playback is
    /// silent while this is set. Display it — silence must never be mute.
    pub device_error: Option<String>,
    pub ready: bool,
}

/// One playable item: a file (Some(path)) or a silent gap (None), with its
/// duration in seconds. Gaps keep take-aligned layers in sync.
type Playlist = Vec<(Option<PathBuf>, f64)>;

/// One layer to play: its sequential clips.
#[derive(Debug, Clone)]
pub struct LayerPlay {
    pub playlist: Playlist,
}

/// Timeline volume automation: default linear volume per layer, replaced by
/// per-span values inside track regions carrying overrides (seconds).
#[derive(Debug, Clone, Default)]
pub struct VolumeAutomation {
    pub default: Vec<f32>,
    pub spans: Vec<(f64, f64, Vec<f32>)>,
}

impl VolumeAutomation {
    pub fn volumes_at(&self, pos: f64) -> &Vec<f32> {
        for (s, e, v) in &self.spans {
            if pos >= *s && pos < *e {
                return v;
            }
        }
        &self.default
    }
}

enum Cmd {
    /// Install ready-made master-bus insert sections (built on the MAIN
    /// thread): `(span, chain)` pairs — `None` span = always active
    /// (mastering chain), `Some` = a track's chain gated to its region.
    /// The ack lets the caller drop its old Arc handles only after the
    /// engine released its own — disposal happens on the caller's thread.
    SetMasterInserts(
        Vec<(Option<(u64, u64)>, Vec<Box<dyn render::BlockProcessor>>)>,
        Sender<()>,
    ),
    /// Install per-lane insert chains, index-aligned with the session's
    /// layer order. Same ack protocol as SetMasterInserts.
    SetLaneInserts(Vec<Vec<Box<dyn render::BlockProcessor>>>, Sender<()>),
    Load {
        layers: Vec<LayerPlay>,
        total_seconds: f64,
        automation: VolumeAutomation,
        sample_rate: u32,
        channels: usize,
    },
    SetAutomation(VolumeAutomation),
    Pause,
    Resume,
    Seek(f64),
    Stop,
}

/// State shared between control side, render thread and device callback.
struct Shared {
    playing: AtomicBool,
    /// Same playing state as an Arc the plugin transport callbacks can own.
    playing_flag: Arc<AtomicBool>,
    loaded: AtomicBool,
    seek_base: AtomicU64,
    consumed: AtomicU64,
    ring_written: AtomicU64,
    ring_read: AtomicU64,
    flush_upto: AtomicU64,
    sample_rate: AtomicU64,
    /// Output DEVICE rate (ring frames are device-rate frames).
    device_rate: AtomicU64,
    total_samples: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            playing_flag: Arc::new(AtomicBool::new(false)),
            loaded: AtomicBool::new(false),
            seek_base: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            ring_written: AtomicU64::new(0),
            ring_read: AtomicU64::new(0),
            flush_upto: AtomicU64::new(0),
            sample_rate: AtomicU64::new(44_100),
            device_rate: AtomicU64::new(44_100),
            total_samples: AtomicU64::new(0),
            error: Mutex::new(None),
        }
    }
}

/// Handle to the audio engine. Public API identical to the previous player.
pub struct PlayerHandle {
    tx: Mutex<Sender<Cmd>>,
    shared: Arc<Shared>,
}

impl PlayerHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::new());
        let shared2 = shared.clone();
        std::thread::Builder::new()
            .name("still-engine".into())
            .spawn(move || render_thread(rx, shared2))
            .expect("failed to spawn engine thread");
        Self {
            tx: Mutex::new(tx),
            shared,
        }
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.tx
            .lock()
            .unwrap()
            .send(cmd)
            .map_err(|_| StillError::Playback("engine thread is not running".into()))
    }

    /// Attach the session's layers (read-only files) and reset the position.
    pub fn load_session(
        &self,
        layers: Vec<LayerPlay>,
        total_seconds: f64,
        automation: VolumeAutomation,
        sample_rate: u32,
        channels: usize,
    ) -> Result<()> {
        self.send(Cmd::Load {
            layers,
            total_seconds,
            automation,
            sample_rate,
            channels,
        })
    }

    pub fn set_automation(&self, automation: VolumeAutomation) -> Result<()> {
        self.send(Cmd::SetAutomation(automation))
    }

    /// Install the master-bus insert sections (proxies built on the main
    /// thread; the engine only processes). `None` span = always active;
    /// `Some((start, end))` = active only inside that sample span. Waits
    /// for the engine to release its old inserts so their disposal runs on
    /// the CALLER's thread.
    pub fn set_master_inserts(
        &self,
        sections: Vec<(Option<(u64, u64)>, Vec<Box<dyn render::BlockProcessor>>)>,
    ) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Cmd::SetMasterInserts(sections, tx))?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| StillError::Playback("engine did not answer".into()))
    }

    /// Install the per-layer insert chains, index-aligned with the layer
    /// order used by `load_session`. Same disposal protocol as
    /// `set_master_inserts`.
    pub fn set_lane_inserts(
        &self,
        lanes: Vec<Vec<Box<dyn render::BlockProcessor>>>,
    ) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Cmd::SetLaneInserts(lanes, tx))?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| StillError::Playback("engine did not answer".into()))
    }

    /// The engine's playing flag, shared with plugin transport callbacks.
    pub fn playing_flag(&self) -> Arc<AtomicBool> {
        self.shared.playing_flag.clone()
    }

    pub fn play(&self) -> Result<()> {
        self.send(Cmd::Resume)
    }

    pub fn pause(&self) -> Result<()> {
        self.send(Cmd::Pause)
    }

    pub fn seek(&self, seconds: f64) -> Result<()> {
        self.send(Cmd::Seek(seconds.max(0.0)))
    }

    pub fn stop(&self) -> Result<()> {
        self.send(Cmd::Stop)
    }

    pub fn state(&self) -> PlaybackState {
        let s = &self.shared;
        let sr = s.sample_rate.load(Ordering::Relaxed).max(1) as f64;
        let dr = s.device_rate.load(Ordering::Relaxed).max(1) as f64;
        // seek_base is in SESSION samples; consumed counts DEVICE frames.
        let pos = s.seek_base.load(Ordering::Relaxed) as f64 / sr
            + s.consumed.load(Ordering::Relaxed) as f64 / dr;
        let total = s.total_samples.load(Ordering::Relaxed) as f64 / sr;
        let device_error = s.error.lock().unwrap().clone();
        PlaybackState {
            playing: s.playing.load(Ordering::Relaxed),
            position_seconds: if total > 0.0 { pos.min(total) } else { pos },
            ready: s.loaded.load(Ordering::Relaxed) && device_error.is_none(),
            device_error,
        }
    }
}

/// Convert seconds-based playlists to sample-based decode items.
fn to_items(playlist: &Playlist, sample_rate: u32) -> Vec<PlayItem> {
    playlist
        .iter()
        .map(|(path, secs)| {
            let samples = (secs * sample_rate as f64).round() as u64;
            match path {
                Some(p) => PlayItem::File {
                    path: p.clone(),
                    samples,
                },
                None => PlayItem::Silence { samples },
            }
        })
        .collect()
}

/// Assign per-lane chains, index-aligned; missing entries clear the lane.
fn install_lanes(r: &mut Renderer, mut lanes: Vec<Vec<Box<dyn render::BlockProcessor>>>) {
    for (i, lane) in r.lanes.iter_mut().enumerate() {
        lane.inserts = if i < lanes.len() {
            std::mem::take(&mut lanes[i])
        } else {
            Vec::new()
        };
    }
}

fn render_thread(rx: Receiver<Cmd>, shared: Arc<Shared>) {
    let mut renderer: Option<Renderer> = None;
    // Inserts survive a session reload (they're main-thread-owned proxies).
    let mut pending_sections: Vec<(Option<(u64, u64)>, Vec<Box<dyn render::BlockProcessor>>)> =
        Vec::new();
    let mut pending_lanes: Vec<Vec<Box<dyn render::BlockProcessor>>> = Vec::new();
    let mut producer: Option<rtrb::Producer<f32>> = None;
    let mut _stream: Option<cpal::Stream> = None;
    let mut device_channels = 2usize;
    let mut block = vec![0.0f32; BLOCK_FRAMES * 8];
    // Session → device rate conversion (None when the rates match).
    let mut resampler: Option<resample::StreamResampler> = None;
    let mut rs_out = vec![0.0f32; BLOCK_FRAMES * 8 * 2];

    loop {
        let timeout = if shared.playing.load(Ordering::Relaxed) {
            Duration::from_millis(5)
        } else {
            Duration::from_millis(100)
        };
        match rx.recv_timeout(timeout) {
            Ok(Cmd::Load {
                layers,
                total_seconds,
                automation,
                sample_rate,
                channels,
            }) => {
                let channels = channels.clamp(1, 2);
                let total_samples = (total_seconds * sample_rate as f64).round() as u64;
                let decoders: Vec<LayerDecoder> = layers
                    .iter()
                    .map(|l| LayerDecoder::new(to_items(&l.playlist, sample_rate), channels))
                    .collect();
                renderer = Some(Renderer::new(
                    decoders,
                    automation,
                    sample_rate,
                    channels,
                    total_samples,
                ));
                shared.sample_rate.store(sample_rate as u64, Ordering::Relaxed);
                shared.total_samples.store(total_samples, Ordering::Relaxed);
                shared.playing.store(false, Ordering::Relaxed);
                shared.playing_flag.store(false, Ordering::Relaxed);
                shared.seek_base.store(0, Ordering::Relaxed);
                shared.consumed.store(0, Ordering::Relaxed);
                shared.ring_written.store(0, Ordering::Relaxed);
                shared.ring_read.store(0, Ordering::Relaxed);
                shared.flush_upto.store(0, Ordering::Relaxed);
                *shared.error.lock().unwrap() = None;

                match open_stream(&shared, sample_rate) {
                    Ok((stream, prod, dev_ch, dev_rate)) => {
                        _stream = Some(stream);
                        producer = Some(prod);
                        device_channels = dev_ch;
                        shared.device_rate.store(dev_rate as u64, Ordering::Relaxed);
                        resampler = if dev_rate != sample_rate {
                            Some(resample::StreamResampler::new(
                                sample_rate,
                                dev_rate,
                                channels,
                            ))
                        } else {
                            None
                        };
                        if std::env::var("STILL_AUDIO_DEBUG").is_ok() {
                            eprintln!(
                                "[audio] session {sample_rate} Hz -> device {dev_rate} Hz ({} ch), resampling: {}",
                                dev_ch,
                                resampler.is_some()
                            );
                        }
                    }
                    Err(e) => {
                        *shared.error.lock().unwrap() = Some(e);
                        producer = None;
                        _stream = None;
                        shared.device_rate.store(sample_rate as u64, Ordering::Relaxed);
                        resampler = None;
                    }
                }
                shared.loaded.store(true, Ordering::Relaxed);
                if let Some(r) = &mut renderer {
                    r.master_sections = std::mem::take(&mut pending_sections)
                        .into_iter()
                        .map(|(span, inserts)| render::InsertSection::new(span, inserts))
                        .collect();
                    install_lanes(r, std::mem::take(&mut pending_lanes));
                }
            }
            Ok(Cmd::SetMasterInserts(sections, reply)) => {
                match &mut renderer {
                    Some(r) => {
                        // Old proxies dropped BEFORE the ack so the caller
                        // (main thread) owns the last Arc at disposal time.
                        r.master_sections = sections
                            .into_iter()
                            .map(|(span, inserts)| render::InsertSection::new(span, inserts))
                            .collect();
                    }
                    None => pending_sections = sections,
                }
                let _ = reply.send(());
            }
            Ok(Cmd::SetLaneInserts(lanes, reply)) => {
                match &mut renderer {
                    Some(r) => install_lanes(r, lanes),
                    None => pending_lanes = lanes,
                }
                let _ = reply.send(());
            }
            Ok(Cmd::SetAutomation(a)) => {
                if let Some(r) = &mut renderer {
                    r.automation = a;
                }
            }
            Ok(Cmd::Pause) => {
                shared.playing.store(false, Ordering::Relaxed);
                shared.playing_flag.store(false, Ordering::Relaxed);
            }
            Ok(Cmd::Resume) => {
                if let Some(r) = &mut renderer {
                    if r.finished() {
                        seek_engine(&shared, r, 0);
                    }
                    shared.playing.store(true, Ordering::Relaxed);
                    shared.playing_flag.store(true, Ordering::Relaxed);
                }
            }
            Ok(Cmd::Seek(secs)) => {
                if let Some(r) = &mut renderer {
                    let target = (secs * r.sample_rate as f64).round() as u64;
                    seek_engine(&shared, r, target);
                    if let Some(rs) = &mut resampler {
                        rs.reset();
                    }
                }
            }
            Ok(Cmd::Stop) => {
                if let Some(r) = &mut renderer {
                    seek_engine(&shared, r, 0);
                    if let Some(rs) = &mut resampler {
                        rs.reset();
                    }
                }
                shared.playing.store(false, Ordering::Relaxed);
                shared.playing_flag.store(false, Ordering::Relaxed);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if !shared.playing.load(Ordering::Relaxed) {
            continue;
        }
        match (&mut renderer, &mut producer) {
            (Some(r), Some(p)) => {
                let ch = r.channels;
                // Worst-case device frames one block can produce.
                let max_out = resampler
                    .as_ref()
                    .map(|rs| rs.max_out_frames(BLOCK_FRAMES))
                    .unwrap_or(BLOCK_FRAMES);
                loop {
                    if p.slots() < max_out * device_channels {
                        break;
                    }
                    let got = r.render_block(&mut block, BLOCK_FRAMES);
                    if got == 0 {
                        break;
                    }
                    // Session rate → device rate when they differ.
                    let (frames, src): (usize, &[f32]) = match &mut resampler {
                        Some(rs) => {
                            let need = rs.max_out_frames(got) * ch;
                            if rs_out.len() < need {
                                rs_out.resize(need, 0.0);
                            }
                            let n = rs.process(&block, got, &mut rs_out);
                            (n, &rs_out[..])
                        }
                        None => (got, &block[..]),
                    };
                    // Interleave the session channels onto the device layout
                    // (duplicate mono, zero extra device channels beyond 2).
                    for f in 0..frames {
                        for dc in 0..device_channels {
                            let v = if dc < 2 {
                                src[f * ch + dc.min(ch - 1)]
                            } else {
                                0.0
                            };
                            let _ = p.push(v);
                        }
                    }
                    shared.ring_written.fetch_add(frames as u64, Ordering::Relaxed);
                }
                if r.finished()
                    && shared.ring_read.load(Ordering::Relaxed)
                        >= shared.ring_written.load(Ordering::Relaxed)
                {
                    shared.playing.store(false, Ordering::Relaxed);
                shared.playing_flag.store(false, Ordering::Relaxed);
                }
            }
            (Some(r), None) => {
                // No output device (CI, headless): advance a silent clock so
                // transport state stays coherent.
                let got = r.render_block(&mut block, BLOCK_FRAMES);
                if got == 0 {
                    shared.playing.store(false, Ordering::Relaxed);
                shared.playing_flag.store(false, Ordering::Relaxed);
                } else {
                    shared.consumed.fetch_add(got as u64, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs_f64(
                        got as f64 / r.sample_rate.max(1) as f64,
                    ));
                }
            }
            _ => {}
        }
    }
}

/// Reposition the renderer and mark all in-flight ring frames as stale.
fn seek_engine(shared: &Arc<Shared>, r: &mut Renderer, target: u64) {
    r.seek(target);
    shared.seek_base.store(target, Ordering::Relaxed);
    shared.consumed.store(0, Ordering::Relaxed);
    shared
        .flush_upto
        .store(shared.ring_written.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Open the cpal output stream. Returns (stream, producer, device_channels).
fn open_stream(
    shared: &Arc<Shared>,
    sample_rate: u32,
) -> std::result::Result<(cpal::Stream, rtrb::Producer<f32>, usize, u32), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output device".to_string())?;
    let default_cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let device_channels = (default_cfg.channels() as usize).clamp(1, 8);
    // Open at the DEVICE's own rate: WASAPI shared mode refuses anything
    // but its mix format (CoreAudio silently resampled for us, which is
    // why forcing the session rate only ever worked on macOS). The render
    // side resamples session → device when the rates differ.
    let device_rate = default_cfg.sample_rate().0.max(8_000);
    let config = cpal::StreamConfig {
        channels: device_channels as u16,
        sample_rate: cpal::SampleRate(device_rate),
        buffer_size: cpal::BufferSize::Default,
    };
    let _ = sample_rate;

    // ~90 ms of buffered audio between render thread and callback.
    let capacity = ((device_rate as usize / 11).next_power_of_two()) * device_channels;
    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(capacity);
    let shared_cb = shared.clone();
    let dc = device_channels;

    let stream = device
        .build_output_stream(
            &config,
            move |out: &mut [f32], _| {
                // Discard frames rendered before the latest seek.
                let flush = shared_cb.flush_upto.load(Ordering::Relaxed);
                let mut read = shared_cb.ring_read.load(Ordering::Relaxed);
                while read < flush && consumer.slots() >= dc {
                    for _ in 0..dc {
                        let _ = consumer.pop();
                    }
                    read += 1;
                }
                if !shared_cb.playing.load(Ordering::Relaxed) {
                    shared_cb.ring_read.store(read, Ordering::Relaxed);
                    out.fill(0.0);
                    return;
                }
                let mut played = 0u64;
                for frame in out.chunks_mut(dc) {
                    if consumer.slots() >= dc {
                        for slot in frame.iter_mut() {
                            *slot = consumer.pop().unwrap_or(0.0);
                        }
                        read += 1;
                        played += 1;
                    } else {
                        frame.fill(0.0); // underrun
                    }
                }
                shared_cb.ring_read.store(read, Ordering::Relaxed);
                if played > 0 {
                    shared_cb.consumed.fetch_add(played, Ordering::Relaxed);
                }
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| format!("cannot open the audio output: {e}"))?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, producer, device_channels, device_rate))
}
