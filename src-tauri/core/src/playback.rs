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

enum Cmd {
    Load(Playlist, f64),
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

    /// Attach a new ordered list of source clips (read-only) forming one
    /// continuous timeline, and reset the position.
    pub fn load(&self, clips: Vec<(PathBuf, f64)>, total_seconds: f64) -> Result<()> {
        self.send(Cmd::Load(clips, total_seconds))
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

    let mut sink: Option<rodio::Sink> = None;
    let mut playlist: Playlist = Vec::new();

    // Build a sink queueing the clip containing `from` (seeked locally) and
    // every following clip — rodio plays queued sources back-to-back, which
    // gives us continuous playback across clip boundaries.
    let make_sink = |playlist: &Playlist, from: f64| -> std::result::Result<rodio::Sink, String> {
        let sink = rodio::Sink::try_new(&handle).map_err(|e| e.to_string())?;
        // A fresh sink starts in the playing state: pause it BEFORE queueing
        // any decoder, otherwise a few ms leak out audibly when seeking while
        // paused. Callers explicitly play() when playback should continue.
        sink.pause();
        let mut cursor = 0.0f64;
        let mut started = false;
        for (path, dur) in playlist {
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
        Ok(sink)
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Cmd::Load(list, duration)) => {
                if let Some(s) = sink.take() {
                    s.stop();
                }
                playlist = list;
                let mut sh = shared.lock().unwrap();
                sh.loaded = true;
                sh.playing = false;
                sh.base_secs = 0.0;
                sh.started = None;
                sh.duration = duration;
                sh.error = None;
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
                // Always rebuild the queue: a seek may land in another clip.
                let mut seek_ok = false;
                if !playlist.is_empty() {
                    match make_sink(&playlist, target) {
                        Ok(s) => {
                            if was_playing {
                                s.play();
                            }
                            if let Some(old) = sink.replace(s) {
                                old.stop();
                            }
                            seek_ok = true;
                        }
                        Err(e) => {
                            shared.lock().unwrap().error = Some(e);
                        }
                    }
                }
                if seek_ok {
                    let mut sh = shared.lock().unwrap();
                    sh.base_secs = target;
                    sh.started = if sh.playing { Some(Instant::now()) } else { None };
                }
            }
            Ok(Cmd::Pause) => {
                let mut sh = shared.lock().unwrap();
                let pos = current_pos(&sh);
                if let Some(s) = &sink {
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
                if let Some(s) = &sink {
                    if !s.empty() {
                        s.play();
                        ok = true;
                    }
                }
                if !ok && !playlist.is_empty() {
                    match make_sink(&playlist, base) {
                        Ok(s) => {
                            s.play();
                            if let Some(old) = sink.replace(s) {
                                old.stop();
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
                if let Some(s) = sink.take() {
                    s.stop();
                }
                let mut sh = shared.lock().unwrap();
                sh.playing = false;
                sh.base_secs = 0.0;
                sh.started = None;
            }
            Err(RecvTimeoutError::Timeout) => {
                // Detect natural end of playback.
                let ended = sink.as_ref().is_some_and(|s| s.empty());
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
