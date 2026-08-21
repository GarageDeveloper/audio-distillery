//! Vst3Plugin: one instantiated VST3 effect in the chain.
//! Stage B lands the real lifecycle; this placeholder keeps the module
//! compiling while the scanner ships first.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::engine::render::BlockProcessor;
use crate::error::{Result, StillError};

pub struct Vst3Plugin {
    pub bypass: bool,
}

impl Vst3Plugin {
    pub fn new(
        component_id: &str,
        _sample_rate: u32,
        _channels: usize,
        _playing: Arc<AtomicBool>,
    ) -> Result<Self> {
        Err(StillError::Playback(format!(
            "VST3 instantiation is not implemented yet ({component_id})"
        )))
    }
}

impl BlockProcessor for Vst3Plugin {
    fn process(&mut self, _buffer: &mut [f32], _channels: usize, _sample_rate: u32) {}
    fn reset(&mut self) {}
}
