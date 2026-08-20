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
    SetMasterChain(Vec<MasterPluginSpec>, Sender<Vec<String>>),
    SetPluginBypass(u32, bool),
    GetPluginHandle(u32, Sender<usize>),
    GetChainStates(Sender<ChainStateSnapshot>),
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
    loaded: AtomicBool,
    seek_base: AtomicU64,
    consumed: AtomicU64,
    ring_written: AtomicU64,
    ring_read: AtomicU64,
    flush_upto: AtomicU64,
    sample_rate: AtomicU64,
    total_samples: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            loaded: AtomicBool::new(false),
            seek_base: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            ring_written: AtomicU64::new(0),
            ring_read: AtomicU64::new(0),
            flush_upto: AtomicU64::new(0),
            sample_rate: AtomicU64::new(44_100),
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

    /// Replace the master-bus mastering chain. Returns per-plugin errors
    /// (empty = every plugin instantiated fine).
    pub fn set_master_chain(&self, specs: Vec<MasterPluginSpec>) -> Result<Vec<String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Cmd::SetMasterChain(specs, tx))?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| StillError::Playback("engine did not answer".into()))
    }

    /// Live bypass toggle (no chain rebuild, keeps plugin state).
    pub fn set_plugin_bypass(&self, id: u32, bypassed: bool) -> Result<()> {
        self.send(Cmd::SetPluginBypass(id, bypassed))
    }

    /// Native handle (AudioUnit pointer) of a chain plugin, for its editor
    /// window. 0 when the plugin is unknown or failed to instantiate.
    pub fn plugin_handle(&self, id: u32) -> Result<usize> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Cmd::GetPluginHandle(id, tx))?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| StillError::Playback("engine did not answer".into()))
    }

    /// Capture the live state blobs of the chain plugins (for project save).
    pub fn get_chain_states(&self) -> Result<ChainStateSnapshot> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Cmd::GetChainStates(tx))?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| StillError::Playback("engine did not answer".into()))
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
        let pos = (s.seek_base.load(Ordering::Relaxed)
            + s.consumed.load(Ordering::Relaxed)) as f64
            / sr;
        let total = s.total_samples.load(Ordering::Relaxed) as f64 / sr;
        PlaybackState {
            playing: s.playing.load(Ordering::Relaxed),
            position_seconds: if total > 0.0 { pos.min(total) } else { pos },
            ready: s.loaded.load(Ordering::Relaxed) && s.error.lock().unwrap().is_none(),
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

fn render_thread(rx: Receiver<Cmd>, shared: Arc<Shared>) {
    let mut renderer: Option<Renderer> = None;
    // Chain wanted by the control side; applied to the renderer on load and
    // whenever it changes (kept here so a reload re-instantiates it).
    let mut chain_specs: Vec<MasterPluginSpec> = Vec::new();
    let mut producer: Option<rtrb::Producer<f32>> = None;
    let mut _stream: Option<cpal::Stream> = None;
    let mut device_channels = 2usize;
    let mut block = vec![0.0f32; BLOCK_FRAMES * 8];

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
                shared.seek_base.store(0, Ordering::Relaxed);
                shared.consumed.store(0, Ordering::Relaxed);
                shared.ring_written.store(0, Ordering::Relaxed);
                shared.ring_read.store(0, Ordering::Relaxed);
                shared.flush_upto.store(0, Ordering::Relaxed);
                *shared.error.lock().unwrap() = None;

                match open_stream(&shared, sample_rate) {
                    Ok((stream, prod, dev_ch)) => {
                        _stream = Some(stream);
                        producer = Some(prod);
                        device_channels = dev_ch;
                    }
                    Err(e) => {
                        *shared.error.lock().unwrap() = Some(e);
                        producer = None;
                        _stream = None;
                    }
                }
                shared.loaded.store(true, Ordering::Relaxed);
                if let Some(r) = &mut renderer {
                    let _ = apply_chain(r, &chain_specs);
                }
            }
            Ok(Cmd::SetMasterChain(specs, reply)) => {
                chain_specs = specs;
                let errors = match &mut renderer {
                    Some(r) => apply_chain(r, &chain_specs),
                    None => Vec::new(), // applied at next load
                };
                let _ = reply.send(errors);
            }
            Ok(Cmd::SetPluginBypass(id, bypassed)) => {
                if let Some(pos) = chain_specs.iter().position(|s| s.id == id) {
                    chain_specs[pos].bypass = bypassed;
                    if let Some(r) = &mut renderer {
                        if r.master_inserts.len() == chain_specs.len() {
                            r.master_inserts[pos].set_bypassed(bypassed);
                        }
                    }
                }
            }
            Ok(Cmd::GetPluginHandle(id, reply)) => {
                let handle = chain_specs
                    .iter()
                    .position(|s| s.id == id)
                    .and_then(|pos| {
                        renderer.as_ref().and_then(|r| {
                            (r.master_inserts.len() == chain_specs.len())
                                .then(|| r.master_inserts[pos].raw_handle())
                        })
                    })
                    .unwrap_or(0);
                let _ = reply.send(handle);
            }
            Ok(Cmd::GetChainStates(reply)) => {
                let snapshot = match &renderer {
                    Some(r) => snapshot_chain(r, &chain_specs),
                    None => chain_specs.iter().map(|s| (s.id, s.state.clone())).collect(),
                };
                let _ = reply.send(snapshot);
            }
            Ok(Cmd::SetAutomation(a)) => {
                if let Some(r) = &mut renderer {
                    r.automation = a;
                }
            }
            Ok(Cmd::Pause) => {
                shared.playing.store(false, Ordering::Relaxed);
            }
            Ok(Cmd::Resume) => {
                if let Some(r) = &mut renderer {
                    if r.finished() {
                        seek_engine(&shared, r, 0);
                    }
                    shared.playing.store(true, Ordering::Relaxed);
                }
            }
            Ok(Cmd::Seek(secs)) => {
                if let Some(r) = &mut renderer {
                    let target = (secs * r.sample_rate as f64).round() as u64;
                    seek_engine(&shared, r, target);
                }
            }
            Ok(Cmd::Stop) => {
                if let Some(r) = &mut renderer {
                    seek_engine(&shared, r, 0);
                }
                shared.playing.store(false, Ordering::Relaxed);
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
                loop {
                    if p.slots() < BLOCK_FRAMES * device_channels {
                        break;
                    }
                    let got = r.render_block(&mut block, BLOCK_FRAMES);
                    if got == 0 {
                        break;
                    }
                    // Interleave the session channels onto the device layout
                    // (duplicate mono, zero extra device channels beyond 2).
                    for f in 0..got {
                        for dc in 0..device_channels {
                            let v = if dc < 2 {
                                block[f * ch + dc.min(ch - 1)]
                            } else {
                                0.0
                            };
                            let _ = p.push(v);
                        }
                    }
                    shared.ring_written.fetch_add(got as u64, Ordering::Relaxed);
                }
                if r.finished()
                    && shared.ring_read.load(Ordering::Relaxed)
                        >= shared.ring_written.load(Ordering::Relaxed)
                {
                    shared.playing.store(false, Ordering::Relaxed);
                }
            }
            (Some(r), None) => {
                // No output device (CI, headless): advance a silent clock so
                // transport state stays coherent.
                let got = r.render_block(&mut block, BLOCK_FRAMES);
                if got == 0 {
                    shared.playing.store(false, Ordering::Relaxed);
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

/// (Re)build the master insert chain on the renderer from the specs.
#[cfg(target_os = "macos")]
fn apply_chain(r: &mut Renderer, specs: &[MasterPluginSpec]) -> Vec<String> {
    use crate::aunit::AuPlugin;
    let mut errors = Vec::new();
    let mut inserts: Vec<Box<dyn render::BlockProcessor>> = Vec::new();
    for spec in specs {
        match AuPlugin::new(&spec.component, r.sample_rate, r.channels) {
            Ok(mut p) => {
                if let Some(state) = &spec.state {
                    if let Err(e) = p.set_state(state) {
                        errors.push(format!("{}: {e}", spec.component));
                    }
                }
                p.bypass = spec.bypass;
                inserts.push(Box::new(p));
            }
            Err(e) => errors.push(format!("{}: {e}", spec.component)),
        }
    }
    r.master_inserts = inserts;
    errors
}

#[cfg(not(target_os = "macos"))]
fn apply_chain(_r: &mut Renderer, specs: &[MasterPluginSpec]) -> Vec<String> {
    if specs.is_empty() {
        Vec::new()
    } else {
        vec!["Audio Unit hosting is only available on macOS".into()]
    }
}

/// Capture the live plugin states. Instances were built in spec order,
/// skipping failed ones; a failed plugin keeps its previously saved state.
fn snapshot_chain(r: &Renderer, specs: &[MasterPluginSpec]) -> ChainStateSnapshot {
    let mut out = Vec::new();
    let live = r.master_inserts.len() == specs.len();
    for (i, spec) in specs.iter().enumerate() {
        let state = if live {
            r.master_inserts[i].save_state().or_else(|| spec.state.clone())
        } else {
            spec.state.clone()
        };
        out.push((spec.id, state));
    }
    out
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
) -> std::result::Result<(cpal::Stream, rtrb::Producer<f32>, usize), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no audio output device".to_string())?;
    let default_cfg = device.default_output_config().map_err(|e| e.to_string())?;
    let device_channels = (default_cfg.channels() as usize).clamp(1, 8);
    let config = cpal::StreamConfig {
        channels: device_channels as u16,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    // ~90 ms of buffered audio between render thread and callback.
    let capacity = ((sample_rate as usize / 11).next_power_of_two()) * device_channels;
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
    Ok((stream, producer, device_channels))
}
