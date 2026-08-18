use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Result, StillError};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PlaybackState {
    pub playing: bool,
    pub position_seconds: f64,
    pub ready: bool,
}

/// One playable clip: file path + duration in seconds.
type Playlist = Vec<(PathBuf, f64)>;

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
    fn volumes_at(&self, pos: f64) -> &Vec<f32> {
        for (s, e, v) in &self.spans {
            if pos >= *s && pos < *e {
                return v;
            }
        }
        &self.default
    }
}

enum Cmd {
    Load(Vec<LayerPlay>, f64, VolumeAutomation),
    SetAutomation(VolumeAutomation),
    Pause,
    Resume,
    Seek(f64),
    Stop,
}

#[derive(Default)]
struct Shared {
    playing: bool,
    base_secs: f64,
    started: Option<Instant>,
    duration: f64,
    loaded: bool,
    error: Option<String>,
}

/// Handle to the audio playback thread. The rodio output stream is not `Send`,
/// so a dedicated thread owns it and receives commands over a channel; the
/// playhead position is derived from a shared clock without round-trips.
/// Layers play through one sink each (all sharing the output stream), started
/// together and re-synced on every seek.
pub struct PlayerHandle {
    tx: Mutex<Sender<Cmd>>,
    shared: Arc<Mutex<Shared>>,
}

impl PlayerHandle {
    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let shared2 = shared.clone();
        std::thread::Builder::new()
            .name("still-playback".into())
            .spawn(move || audio_thread(rx, shared2))
            .expect("failed to spawn playback thread");
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
            .map_err(|_| StillError::Playback("playback thread is not running".into()))
    }

    /// Attach the session's layers (read-only files) and reset the position.
    pub fn load(
        &self,
        layers: Vec<LayerPlay>,
        total_seconds: f64,
        automation: VolumeAutomation,
    ) -> Result<()> {
        self.send(Cmd::Load(layers, total_seconds, automation))
    }

    /// Replace the volume automation (faders, mutes, solos, per-track
    /// overrides). Applies immediately and follows the playhead afterwards.
    pub fn set_automation(&self, automation: VolumeAutomation) -> Result<()> {
        self.send(Cmd::SetAutomation(automation))
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
        let s = self.shared.lock().unwrap();
        let mut pos = s.base_secs;
        if let (true, Some(started)) = (s.playing, s.started) {
            pos += started.elapsed().as_secs_f64();
        }
        if s.duration > 0.0 {
            pos = pos.min(s.duration);
        }
        PlaybackState {
            playing: s.playing,
            position_seconds: pos,
            ready: s.loaded && s.error.is_none(),
        }
    }
}

fn current_pos(s: &Shared) -> f64 {
    let mut pos = s.base_secs;
    if let (true, Some(started)) = (s.playing, s.started) {
        pos += started.elapsed().as_secs_f64();
    }
    if s.duration > 0.0 {
        pos = pos.min(s.duration);
    }
    pos
}

fn audio_thread(rx: Receiver<Cmd>, shared: Arc<Mutex<Shared>>) {
    let stream = rodio::OutputStream::try_default();
    let (_stream, handle) = match stream {
        Ok(x) => x,
        Err(e) => {
            shared.lock().unwrap().error = Some(format!("no audio output device: {e}"));
            // Keep draining commands so senders never block/panic.
            while rx.recv().is_ok() {}
            return;
        }
    };

    let mut sinks: Vec<rodio::Sink> = Vec::new();
    let mut layers: Vec<LayerPlay> = Vec::new();
    let mut automation = VolumeAutomation::default();
    let mut applied: Vec<f32> = Vec::new();

    // Build one PAUSED sink per layer, each queueing the clip containing
    // `from` (seeked locally) plus every following clip. Pausing before
    // queueing avoids audible leaks when seeking while stopped; all sinks
    // are then started together for layer sync.
    let make_sinks =
        |layers: &[LayerPlay], from: f64| -> std::result::Result<Vec<rodio::Sink>, String> {
            let mut out = Vec::with_capacity(layers.len());
            for layer in layers {
                let sink = rodio::Sink::try_new(&handle).map_err(|e| e.to_string())?;
                sink.pause();
                let mut cursor = 0.0f64;
                let mut started = false;
                for (path, dur) in &layer.playlist {
                    if !started && from >= cursor + dur {
                        cursor += dur;
                        continue;
                    }
                    let file = File::open(path).map_err(|e| e.to_string())?;
                    let decoder =
                        rodio::Decoder::new(BufReader::new(file)).map_err(|e| e.to_string())?;
                    sink.append(decoder);
                    if !started {
                        let local = (from - cursor).max(0.0);
                        if local > 0.0 {
                            let _ = sink.try_seek(Duration::from_secs_f64(local));
                        }
                        started = true;
                    }
                }
                // A layer shorter than `from` yields an empty sink: fine, it
                // just keeps indices aligned for live volume changes.
                out.push(sink);
            }
            Ok(out)
        };

    let stop_all = |sinks: &mut Vec<rodio::Sink>| {
        for s in sinks.drain(..) {
            s.stop();
        }
    };

    // Apply the automation volumes for the given position (skips no-ops).
    let apply_volumes =
        |sinks: &[rodio::Sink], automation: &VolumeAutomation, applied: &mut Vec<f32>, pos: f64| {
            let vols = automation.volumes_at(pos);
            if applied.as_slice() == vols.as_slice() {
                return;
            }
            for (i, s) in sinks.iter().enumerate() {
                s.set_volume(vols.get(i).copied().unwrap_or(1.0));
            }
            *applied = vols.clone();
        };

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Cmd::Load(list, duration, auto)) => {
                stop_all(&mut sinks);
                layers = list;
                automation = auto;
                applied.clear();
                let mut sh = shared.lock().unwrap();
                sh.loaded = true;
                sh.playing = false;
                sh.base_secs = 0.0;
                sh.started = None;
                sh.duration = duration;
                sh.error = None;
            }
            Ok(Cmd::SetAutomation(auto)) => {
                automation = auto;
                applied.clear();
                let pos = current_pos(&shared.lock().unwrap());
                apply_volumes(&sinks, &automation, &mut applied, pos);
            }
            Ok(Cmd::Seek(pos)) => {
                let was_playing = shared.lock().unwrap().playing;
                let target = {
                    let sh = shared.lock().unwrap();
                    if sh.duration > 0.0 {
                        pos.min(sh.duration)
                    } else {
                        pos
                    }
                };
                // Always rebuild the queues: a seek may land in another clip,
                // and rebuilding re-syncs the layers.
                if !layers.is_empty() {
                    match make_sinks(&layers, target) {
                        Ok(new_sinks) => {
                            stop_all(&mut sinks);
                            sinks = new_sinks;
                            applied.clear();
                            apply_volumes(&sinks, &automation, &mut applied, target);
                            if was_playing {
                                for s in &sinks {
                                    s.play();
                                }
                            }
                            let mut sh = shared.lock().unwrap();
                            sh.base_secs = target;
                            sh.started =
                                if sh.playing { Some(Instant::now()) } else { None };
                        }
                        Err(e) => {
                            shared.lock().unwrap().error = Some(e);
                        }
                    }
                }
            }
            Ok(Cmd::Pause) => {
                let mut sh = shared.lock().unwrap();
                let pos = current_pos(&sh);
                for s in &sinks {
                    s.pause();
                }
                sh.base_secs = pos;
                sh.started = None;
                sh.playing = false;
            }
            Ok(Cmd::Resume) => {
                let (loaded, base, dur) = {
                    let sh = shared.lock().unwrap();
                    (sh.loaded, sh.base_secs, sh.duration)
                };
                if !loaded {
                    continue;
                }
                let base = if dur > 0.0 && base >= dur { 0.0 } else { base };
                let mut ok = false;
                if sinks.iter().any(|s| !s.empty()) {
                    for s in &sinks {
                        s.play();
                    }
                    ok = true;
                }
                if !ok && !layers.is_empty() {
                    match make_sinks(&layers, base) {
                        Ok(new_sinks) => {
                            stop_all(&mut sinks);
                            sinks = new_sinks;
                            applied.clear();
                            apply_volumes(&sinks, &automation, &mut applied, base);
                            for s in &sinks {
                                s.play();
                            }
                            ok = true;
                        }
                        Err(e) => {
                            shared.lock().unwrap().error = Some(e);
                        }
                    }
                }
                if ok {
                    let mut sh = shared.lock().unwrap();
                    sh.base_secs = base;
                    sh.started = Some(Instant::now());
                    sh.playing = true;
                }
            }
            Ok(Cmd::Stop) => {
                stop_all(&mut sinks);
                let mut sh = shared.lock().unwrap();
                sh.playing = false;
                sh.base_secs = 0.0;
                sh.started = None;
            }
            Err(RecvTimeoutError::Timeout) => {
                // Follow the playhead through override regions (~10 Hz).
                {
                    let sh = shared.lock().unwrap();
                    if sh.playing {
                        let pos = current_pos(&sh);
                        drop(sh);
                        apply_volumes(&sinks, &automation, &mut applied, pos);
                    }
                }
                // Detect natural end of playback: every layer drained.
                let ended = !sinks.is_empty() && sinks.iter().all(|s| s.empty());
                if ended {
                    let mut sh = shared.lock().unwrap();
                    if sh.playing {
                        sh.playing = false;
                        sh.base_secs = sh.duration;
                        sh.started = None;
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}
