//! Audio Unit (AU) hosting — macOS. Phase B of the mastering roadmap.
//!
//! Effects ('aufx') are discovered through the AudioComponent API,
//! instantiated with the session format (non-interleaved f32) and processed
//! block by block via `AudioUnitRender` on the engine's render thread, where
//! they plug into the existing `BlockProcessor` insert slots (master bus
//! today; the same type works for per-layer/per-track inserts later).
//! Plugin state is captured/restored through `kAudioUnitProperty_ClassInfo`
//! (binary plist), which is what the `.still` project persists.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One installed effect component, as shown in the plugin browser.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct AuComponentInfo {
    /// Stable identifier "type:subtype:manufacturer" (fourcc strings).
    pub id: String,
    pub name: String,
}

#[cfg(target_os = "macos")]
pub use macos::{list_effects, AuPlugin};

#[cfg(not(target_os = "macos"))]
pub fn list_effects() -> Vec<AuComponentInfo> {
    Vec::new()
}

#[cfg(target_os = "macos")]
mod macos {
    use super::AuComponentInfo;
    use crate::engine::render::BlockProcessor;
    use crate::error::{Result, StillError};
    use coreaudio_sys::*;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    fn fourcc(s: &str) -> u32 {
        let b = s.as_bytes();
        ((b[0] as u32) << 24) | ((b[1] as u32) << 16) | ((b[2] as u32) << 8) | b[3] as u32
    }

    fn fourcc_str(v: u32) -> String {
        let b = [
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ];
        String::from_utf8_lossy(&b).to_string()
    }

    fn cfstring_to_string(s: CFStringRef) -> String {
        unsafe {
            let len = CFStringGetLength(s);
            let max = CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) + 1;
            let mut buf = vec![0u8; max as usize];
            if CFStringGetCString(s, buf.as_mut_ptr() as *mut i8, max, kCFStringEncodingUTF8)
                != 0
            {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                String::from_utf8_lossy(&buf[..end]).to_string()
            } else {
                String::new()
            }
        }
    }

    /// Enumerate every installed audio EFFECT unit ('aufx').
    pub fn list_effects() -> Vec<AuComponentInfo> {
        let mut out = Vec::new();
        unsafe {
            let desc = AudioComponentDescription {
                componentType: fourcc("aufx"),
                componentSubType: 0,
                componentManufacturer: 0,
                componentFlags: 0,
                componentFlagsMask: 0,
            };
            let mut comp: AudioComponent = AudioComponentFindNext(null_mut(), &desc);
            while !comp.is_null() {
                let mut full = AudioComponentDescription {
                    componentType: 0,
                    componentSubType: 0,
                    componentManufacturer: 0,
                    componentFlags: 0,
                    componentFlagsMask: 0,
                };
                let mut name: CFStringRef = null();
                if AudioComponentGetDescription(comp, &mut full) == 0
                    && AudioComponentCopyName(comp, &mut name as *mut _ as *mut _) == 0
                    && !name.is_null()
                {
                    out.push(AuComponentInfo {
                        id: format!(
                            "{}:{}:{}",
                            fourcc_str(full.componentType),
                            fourcc_str(full.componentSubType),
                            fourcc_str(full.componentManufacturer)
                        ),
                        name: cfstring_to_string(name),
                    });
                    CFRelease(name as *const _);
                }
                comp = AudioComponentFindNext(comp, &desc);
            }
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out
    }

    /// Host transport info exposed to the plugin (iZotope-style analyzers
    /// gate their UI on the host reporting a playing transport).
    pub struct TransportStore {
        pub playing: Arc<AtomicBool>,
        pub sample_pos: AtomicU64,
    }

    unsafe extern "C" fn transport_state_cb(
        in_ref_con: *mut std::os::raw::c_void,
        out_is_playing: *mut Boolean,
        out_transport_changed: *mut Boolean,
        out_current_sample: *mut Float64,
        out_is_cycling: *mut Boolean,
        out_cycle_start: *mut Float64,
        out_cycle_end: *mut Float64,
    ) -> OSStatus {
        let store = &*(in_ref_con as *const TransportStore);
        if !out_is_playing.is_null() {
            *out_is_playing = store.playing.load(Ordering::Relaxed) as Boolean;
        }
        if !out_transport_changed.is_null() {
            *out_transport_changed = 0;
        }
        if !out_current_sample.is_null() {
            *out_current_sample = store.sample_pos.load(Ordering::Relaxed) as f64;
        }
        if !out_is_cycling.is_null() {
            *out_is_cycling = 0;
        }
        if !out_cycle_start.is_null() {
            *out_cycle_start = 0.0;
        }
        if !out_cycle_end.is_null() {
            *out_cycle_end = 0.0;
        }
        0
    }

    /// Planar input the render callback feeds to the AU when it pulls.
    struct InputStore {
        planar: Vec<Vec<f32>>,
        frames: usize,
    }

    unsafe extern "C" fn input_cb(
        in_ref_con: *mut std::os::raw::c_void,
        _flags: *mut AudioUnitRenderActionFlags,
        _ts: *const AudioTimeStamp,
        _bus: u32,
        in_frames: u32,
        io_data: *mut AudioBufferList,
    ) -> OSStatus {
        let store = &*(in_ref_con as *const InputStore);
        let abl = &mut *io_data;
        let buffers = std::slice::from_raw_parts_mut(
            abl.mBuffers.as_mut_ptr(),
            abl.mNumberBuffers as usize,
        );
        for (c, buf) in buffers.iter_mut().enumerate() {
            let dst =
                std::slice::from_raw_parts_mut(buf.mData as *mut f32, in_frames as usize);
            let src = store
                .planar
                .get(c.min(store.planar.len().saturating_sub(1)));
            for (i, d) in dst.iter_mut().enumerate() {
                *d = src
                    .and_then(|s| s.get(i.min(store.frames.saturating_sub(1))))
                    .copied()
                    .unwrap_or(0.0);
            }
            buf.mDataByteSize = in_frames * 4;
        }
        0
    }

    /// A hosted AU effect, usable as a `BlockProcessor` insert.
    pub struct AuPlugin {
        unit: AudioUnit,
        input: Box<InputStore>,
        transport: Box<TransportStore>,
        out_planar: Vec<Vec<f32>>,
        channels: usize,
        sample_rate: u32,
        pub bypass: bool,
        rendered: u64,
        /// Consecutive render failures, for self-healing (a plugin that
        /// reconfigures its DSP on preset load can transiently error).
        render_errors: u32,
    }

    // The raw AudioUnit pointer is only ever used from the thread that owns
    // the processor (creation + render both happen on the engine thread).
    unsafe impl Send for AuPlugin {}

    fn check(status: OSStatus, what: &str) -> Result<()> {
        if status == 0 {
            Ok(())
        } else {
            Err(StillError::Playback(format!("{what} failed (OSStatus {status})")))
        }
    }

    fn asbd(sample_rate: u32, _channels: usize) -> AudioStreamBasicDescription {
        AudioStreamBasicDescription {
            mSampleRate: sample_rate as f64,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsFloat
                | kAudioFormatFlagsNativeEndian
                | kAudioFormatFlagIsPacked
                | kAudioFormatFlagIsNonInterleaved,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: _channels as u32,
            mBitsPerChannel: 32,
            mReserved: 0,
        }
    }

    impl AuPlugin {
        /// Instantiate `component_id` ("aufx:xxxx:yyyy") at the session
        /// format and prepare it for block rendering. `playing` feeds the
        /// host transport callback the plugin UIs poll.
        pub fn new(
            component_id: &str,
            sample_rate: u32,
            channels: usize,
            playing: Arc<AtomicBool>,
        ) -> Result<Self> {
            let parts: Vec<&str> = component_id.split(':').collect();
            if parts.len() != 3 || parts.iter().any(|p| p.len() != 4) {
                return Err(StillError::Playback(format!(
                    "invalid audio unit id: {component_id}"
                )));
            }
            let channels = channels.clamp(1, 2);
            unsafe {
                let desc = AudioComponentDescription {
                    componentType: fourcc(parts[0]),
                    componentSubType: fourcc(parts[1]),
                    componentManufacturer: fourcc(parts[2]),
                    componentFlags: 0,
                    componentFlagsMask: 0,
                };
                let comp = AudioComponentFindNext(null_mut(), &desc);
                if comp.is_null() {
                    return Err(StillError::Playback(format!(
                        "audio unit not installed: {component_id}"
                    )));
                }
                let mut unit: AudioUnit = null_mut();
                check(AudioComponentInstanceNew(comp, &mut unit), "instantiate")?;

                let fmt = asbd(sample_rate, channels);
                let sz = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
                check(
                    AudioUnitSetProperty(
                        unit,
                        kAudioUnitProperty_StreamFormat,
                        kAudioUnitScope_Input,
                        0,
                        &fmt as *const _ as *const _,
                        sz,
                    ),
                    "set input format",
                )?;
                check(
                    AudioUnitSetProperty(
                        unit,
                        kAudioUnitProperty_StreamFormat,
                        kAudioUnitScope_Output,
                        0,
                        &fmt as *const _ as *const _,
                        sz,
                    ),
                    "set output format",
                )?;
                let max_frames: u32 = 4096;
                check(
                    AudioUnitSetProperty(
                        unit,
                        kAudioUnitProperty_MaximumFramesPerSlice,
                        kAudioUnitScope_Global,
                        0,
                        &max_frames as *const _ as *const _,
                        4,
                    ),
                    "set max frames",
                )?;

                let plugin = Self {
                    unit,
                    input: Box::new(InputStore {
                        planar: vec![vec![0.0; 4096]; channels],
                        frames: 0,
                    }),
                    transport: Box::new(TransportStore {
                        playing,
                        sample_pos: AtomicU64::new(0),
                    }),
                    out_planar: vec![vec![0.0; 4096]; channels],
                    channels,
                    sample_rate,
                    bypass: false,
                    rendered: 0,
                    render_errors: 0,
                };
                let cb = AURenderCallbackStruct {
                    inputProc: Some(input_cb),
                    inputProcRefCon: &*plugin.input as *const InputStore as *mut _,
                };
                check(
                    AudioUnitSetProperty(
                        plugin.unit,
                        kAudioUnitProperty_SetRenderCallback,
                        kAudioUnitScope_Input,
                        0,
                        &cb as *const _ as *const _,
                        std::mem::size_of::<AURenderCallbackStruct>() as u32,
                    ),
                    "set render callback",
                )?;
                // Host transport callbacks: plugin UIs (spectrum analyzers,
                // meters) poll these to know the host is rolling.
                let host_cb = HostCallbackInfo {
                    hostUserData: &*plugin.transport as *const TransportStore as *mut _,
                    beatAndTempoProc: None,
                    musicalTimeLocationProc: None,
                    transportStateProc: Some(transport_state_cb),
                    transportStateProc2: None,
                };
                let _ = AudioUnitSetProperty(
                    plugin.unit,
                    kAudioUnitProperty_HostCallbacks,
                    kAudioUnitScope_Global,
                    0,
                    &host_cb as *const _ as *const _,
                    std::mem::size_of::<HostCallbackInfo>() as u32,
                );
                check(AudioUnitInitialize(plugin.unit), "initialize")?;
                Ok(plugin)
            }
        }

        /// Capture the full plugin state as a binary plist blob.
        pub fn get_state(&self) -> Option<Vec<u8>> {
            unsafe {
                let mut plist: CFPropertyListRef = null();
                let mut sz = std::mem::size_of::<CFPropertyListRef>() as u32;
                if AudioUnitGetProperty(
                    self.unit,
                    kAudioUnitProperty_ClassInfo,
                    kAudioUnitScope_Global,
                    0,
                    &mut plist as *mut _ as *mut _,
                    &mut sz,
                ) != 0
                    || plist.is_null()
                {
                    return None;
                }
                let data = CFPropertyListCreateData(
                    kCFAllocatorDefault,
                    plist,
                    kCFPropertyListBinaryFormat_v1_0 as CFPropertyListFormat,
                    0,
                    null_mut(),
                );
                CFRelease(plist);
                if data.is_null() {
                    return None;
                }
                let len = CFDataGetLength(data) as usize;
                let mut out = vec![0u8; len];
                CFDataGetBytes(
                    data,
                    CFRange {
                        location: 0,
                        length: len as CFIndex,
                    },
                    out.as_mut_ptr(),
                );
                CFRelease(data as *const _);
                Some(out)
            }
        }

        /// Restore a previously captured state blob.
        pub fn set_state(&mut self, blob: &[u8]) -> Result<()> {
            unsafe {
                let data = CFDataCreate(
                    kCFAllocatorDefault,
                    blob.as_ptr(),
                    blob.len() as CFIndex,
                );
                if data.is_null() {
                    return Err(StillError::Playback("invalid plugin state".into()));
                }
                let plist = CFPropertyListCreateWithData(
                    kCFAllocatorDefault,
                    data,
                    kCFPropertyListImmutable as CFOptionFlags,
                    null_mut(),
                    null_mut(),
                );
                CFRelease(data as *const _);
                if plist.is_null() {
                    return Err(StillError::Playback("unreadable plugin state".into()));
                }
                let status = AudioUnitSetProperty(
                    self.unit,
                    kAudioUnitProperty_ClassInfo,
                    kAudioUnitScope_Global,
                    0,
                    &plist as *const _ as *const _,
                    std::mem::size_of::<CFPropertyListRef>() as u32,
                );
                CFRelease(plist);
                check(status, "restore state")
            }
        }

        pub fn latency_seconds(&self) -> f64 {
            unsafe {
                let mut latency: f64 = 0.0;
                let mut sz = 8u32;
                AudioUnitGetProperty(
                    self.unit,
                    kAudioUnitProperty_Latency,
                    kAudioUnitScope_Global,
                    0,
                    &mut latency as *mut _ as *mut _,
                    &mut sz,
                );
                latency
            }
        }
    }

    impl Drop for AuPlugin {
        fn drop(&mut self) {
            unsafe {
                AudioUnitUninitialize(self.unit);
                AudioComponentInstanceDispose(self.unit);
            }
        }
    }

    impl BlockProcessor for AuPlugin {
        fn process(&mut self, buffer: &mut [f32], channels: usize, _sample_rate: u32) {
            if self.bypass {
                return;
            }
            let ch = self.channels.min(channels.max(1));
            let frames = buffer.len() / channels;
            if frames == 0 {
                return;
            }
            // Deinterleave into the callback's input store.
            for c in 0..self.channels {
                let src_c = c.min(channels - 1);
                let plane = &mut self.input.planar[c];
                for f in 0..frames {
                    plane[f] = buffer[f * channels + src_c];
                }
            }
            self.input.frames = frames;

            unsafe {
                let mut flags: AudioUnitRenderActionFlags = 0;
                let ts = AudioTimeStamp {
                    mSampleTime: self.rendered as f64,
                    mHostTime: 0,
                    mRateScalar: 0.0,
                    mWordClockTime: 0,
                    mSMPTETime: std::mem::zeroed(),
                    mFlags: kAudioTimeStampSampleTimeValid,
                    mReserved: 0,
                };
                // Build the output AudioBufferList over our planar buffers.
                let mut storage =
                    vec![0u8; 8 + std::mem::size_of::<AudioBuffer>() * self.channels];
                let abl = storage.as_mut_ptr() as *mut AudioBufferList;
                (*abl).mNumberBuffers = self.channels as u32;
                let bufs = std::slice::from_raw_parts_mut(
                    (*abl).mBuffers.as_mut_ptr(),
                    self.channels,
                );
                for (c, b) in bufs.iter_mut().enumerate() {
                    b.mNumberChannels = 1;
                    b.mDataByteSize = (frames * 4) as u32;
                    b.mData = self.out_planar[c].as_mut_ptr() as *mut _;
                }
                let status =
                    AudioUnitRender(self.unit, &mut flags, &ts, 0, frames as u32, abl);
                if status != 0 {
                    // A plugin reconfiguring its DSP (preset load from its
                    // own UI) can transiently fail; self-heal instead of
                    // silently going dry forever: reset first, then a full
                    // re-initialize if errors persist.
                    self.render_errors += 1;
                    if self.render_errors == 3 {
                        AudioUnitReset(self.unit, kAudioUnitScope_Global, 0);
                    } else if self.render_errors == 6 {
                        AudioUnitUninitialize(self.unit);
                        AudioUnitInitialize(self.unit);
                    } else if self.render_errors == 7 {
                        eprintln!(
                            "audio unit render keeps failing (OSStatus {status}) — passing dry signal"
                        );
                    }
                    return; // leave the dry signal untouched this block
                }
                self.render_errors = 0;
            }
            self.rendered += frames as u64;
            self.transport
                .sample_pos
                .store(self.rendered, Ordering::Relaxed);
            // Interleave back.
            for c in 0..channels {
                let src = &self.out_planar[c.min(ch - 1)];
                for f in 0..frames {
                    buffer[f * channels + c] = src[f];
                }
            }
        }

        fn latency_samples(&self) -> u32 {
            (self.latency_seconds() * self.sample_rate as f64).round() as u32
        }

        fn reset(&mut self) {
            unsafe {
                AudioUnitReset(self.unit, kAudioUnitScope_Global, 0);
            }
            self.rendered = 0;
        }

        fn save_state(&self) -> Option<Vec<u8>> {
            self.get_state()
        }

        fn set_bypassed(&mut self, bypassed: bool) {
            self.bypass = bypassed;
        }

        fn raw_handle(&self) -> usize {
            self.unit as usize
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn lists_apple_effects() {
            let effects = list_effects();
            assert!(
                effects.iter().any(|e| e.id.ends_with(":appl")),
                "expected Apple stock effects, got {} entries",
                effects.len()
            );
        }

        #[test]
        fn hosts_a_stock_effect_and_roundtrips_state() {
            // AULowpass ships with macOS.
            let mut p = AuPlugin::new(
                "aufx:lpas:appl",
                44_100,
                2,
                Arc::new(AtomicBool::new(true)),
            )
            .expect("instantiate");
            // A 10 kHz-ish square through a lowpass must come out altered.
            let frames = 512;
            let mut buf: Vec<f32> = (0..frames * 2)
                .map(|i| if (i / 8) % 2 == 0 { 0.5 } else { -0.5 })
                .collect();
            let original = buf.clone();
            p.process(&mut buf, 2, 44_100);
            p.process(&mut buf, 2, 44_100);
            assert_ne!(buf, original, "lowpass left the signal untouched");

            // State capture / restore round-trips.
            let state = p.get_state().expect("state");
            assert!(!state.is_empty());
            p.set_state(&state).expect("restore");

            // Bypass leaves the signal untouched.
            p.bypass = true;
            let mut buf2 = original.clone();
            p.process(&mut buf2, 2, 44_100);
            assert_eq!(buf2, original);
        }
    }
}
