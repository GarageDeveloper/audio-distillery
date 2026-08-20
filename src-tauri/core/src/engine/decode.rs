//! Layer decoding for the realtime engine: one `LayerDecoder` per layer
//! walks its playlist (files and silent take-gaps) and produces interleaved
//! f32 frames at the SESSION channel count, sample-accurately seekable.

use std::path::PathBuf;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{SeekMode, SeekTo};
use symphonia::core::units::Time;

use crate::audio::{open, Opened};

/// One playlist item: a file or a silent gap, with its length in samples.
#[derive(Debug, Clone)]
pub enum PlayItem {
    File { path: PathBuf, samples: u64 },
    Silence { samples: u64 },
}

/// Decodes one layer's playlist as a continuous stream of frames.
pub struct LayerDecoder {
    items: Vec<PlayItem>,
    out_channels: usize,
    /// Timeline position (samples) of the next frame to produce.
    pos: u64,
    /// Total length of the playlist.
    total: u64,
    // Current item state.
    item_idx: usize,
    /// Samples already produced within the current item.
    item_pos: u64,
    opened: Option<Opened>,
    sample_buf: Option<SampleBuffer<f32>>,
    /// Decoded-but-not-yet-consumed frames of the current file (interleaved
    /// at the SOURCE channel count).
    pending: Vec<f32>,
    pending_off: usize,
    src_channels: usize,
}

impl LayerDecoder {
    pub fn new(items: Vec<PlayItem>, out_channels: usize) -> Self {
        let total = items
            .iter()
            .map(|i| match i {
                PlayItem::File { samples, .. } | PlayItem::Silence { samples } => *samples,
            })
            .sum();
        Self {
            items,
            out_channels: out_channels.max(1),
            pos: 0,
            total,
            item_idx: 0,
            item_pos: 0,
            opened: None,
            sample_buf: None,
            pending: Vec::new(),
            pending_off: 0,
            src_channels: 1,
        }
    }

    pub fn total_samples(&self) -> u64 {
        self.total
    }

    pub fn finished(&self) -> bool {
        self.pos >= self.total
    }

    /// Jump to an absolute timeline position (sample-accurate: coarse
    /// container seek, then decode-and-discard up to the exact frame).
    pub fn seek(&mut self, target: u64) {
        let target = target.min(self.total);
        self.opened = None;
        self.sample_buf = None;
        self.pending.clear();
        self.pending_off = 0;
        let mut acc = 0u64;
        for (i, item) in self.items.iter().enumerate() {
            let len = match item {
                PlayItem::File { samples, .. } | PlayItem::Silence { samples } => *samples,
            };
            if target < acc + len {
                self.item_idx = i;
                self.item_pos = target - acc;
                self.pos = target;
                return;
            }
            acc += len;
        }
        self.item_idx = self.items.len();
        self.item_pos = 0;
        self.pos = self.total;
    }

    /// Open the current file item and position it at `self.item_pos`.
    fn ensure_open(&mut self) -> bool {
        if self.opened.is_some() {
            return true;
        }
        let PlayItem::File { path, .. } = &self.items[self.item_idx] else {
            return false;
        };
        let Ok(mut o) = open(path) else {
            return false;
        };
        self.src_channels = o.channels.max(1) as usize;
        if self.item_pos > 0 {
            let secs = self.item_pos as f64 / o.sample_rate.max(1) as f64;
            let seeked = o.format.seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: Time::from(secs),
                    track_id: Some(o.track_id),
                },
            );
            o.decoder.reset();
            // Decode-and-discard from the packet boundary to the exact frame.
            let landed = seeked.map(|s| s.actual_ts).unwrap_or(0);
            let mut to_skip = self.item_pos.saturating_sub(landed);
            self.opened = Some(o);
            while to_skip > 0 {
                let Some(frames) = self.decode_packet() else { break };
                let n = (frames.len() / self.src_channels) as u64;
                if n <= to_skip {
                    to_skip -= n;
                } else {
                    self.pending = frames;
                    self.pending_off = (to_skip as usize) * self.src_channels;
                    to_skip = 0;
                }
            }
        } else {
            self.opened = Some(o);
        }
        true
    }

    /// Decode the next packet of the open file; returns interleaved frames
    /// at the source channel count (empty vec = skipped packet).
    fn decode_packet(&mut self) -> Option<Vec<f32>> {
        let o = self.opened.as_mut()?;
        loop {
            let packet = match o.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return None
                }
                Err(SymError::ResetRequired) => return None,
                Err(_) => return None,
            };
            if packet.track_id() != o.track_id {
                continue;
            }
            match o.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let needed = decoded.capacity() as u64;
                    let buf = match &mut self.sample_buf {
                        Some(b)
                            if b.capacity() as u64
                                >= needed * spec.channels.count() as u64 =>
                        {
                            b
                        }
                        _ => self.sample_buf.insert(SampleBuffer::new(needed, spec)),
                    };
                    buf.copy_interleaved_ref(decoded);
                    return Some(buf.samples().to_vec());
                }
                Err(SymError::DecodeError(_)) => continue,
                Err(_) => return None,
            }
        }
    }

    /// Produce exactly `frames` frames into `out` (interleaved at the
    /// session channel count), zero-filling past the end of the playlist.
    /// Returns the number of NON-silent source frames written (diagnostic).
    pub fn read(&mut self, out: &mut [f32], frames: usize) {
        debug_assert_eq!(out.len(), frames * self.out_channels);
        out.fill(0.0);
        let mut done = 0usize;
        while done < frames && self.item_idx < self.items.len() {
            let (item_len, is_file) = match &self.items[self.item_idx] {
                PlayItem::File { samples, .. } => (*samples, true),
                PlayItem::Silence { samples } => (*samples, false),
            };
            if self.item_pos >= item_len {
                self.item_idx += 1;
                self.item_pos = 0;
                self.opened = None;
                self.pending.clear();
                self.pending_off = 0;
                continue;
            }
            let want = ((item_len - self.item_pos) as usize).min(frames - done);
            if !is_file {
                // Silence: frames are already zeroed.
                self.item_pos += want as u64;
                self.pos += want as u64;
                done += want;
                continue;
            }
            if !self.ensure_open() {
                // Unreadable file: substitute silence rather than stalling.
                self.item_pos += want as u64;
                self.pos += want as u64;
                done += want;
                continue;
            }
            let mut produced = 0usize;
            while produced < want {
                if self.pending_off >= self.pending.len() {
                    match self.decode_packet() {
                        Some(frames_buf) => {
                            self.pending = frames_buf;
                            self.pending_off = 0;
                        }
                        None => break, // file shorter than declared → silence
                    }
                }
                let src = self.src_channels;
                let avail = (self.pending.len() - self.pending_off) / src;
                let take = avail.min(want - produced);
                for f in 0..take {
                    let s = self.pending_off + f * src;
                    let d = (done + produced + f) * self.out_channels;
                    for c in 0..self.out_channels {
                        // Mono→multi duplicates; extra source channels fold
                        // into the last output channel? No: map 1:1, wrap
                        // source when narrower (mono upmix), drop extras.
                        out[d + c] = self.pending[s + c.min(src - 1)];
                    }
                }
                self.pending_off += take * src;
                produced += take;
            }
            // Whatever the decoder actually yielded, account the full `want`
            // (missing tail = silence) to keep the timeline exact.
            self.item_pos += want as u64;
            self.pos += want as u64;
            done += want;
        }
    }
}
