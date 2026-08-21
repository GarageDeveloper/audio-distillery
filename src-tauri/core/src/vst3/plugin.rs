//! Vst3Plugin: one instantiated VST3 effect, processing interleaved f32
//! blocks on the engine thread through the `BlockProcessor` trait.
//!
//! Threading contract (same as AuPlugin): the FULL lifecycle — new(),
//! state get/set, drop — happens on the thread the caller chose (main
//! thread for the live chain via ChainHost, a worker thread for export);
//! only `process` runs on the audio thread. Teardown order is critical and
//! encoded in `Drop`: setProcessing(0) → setActive(0) →
//! setComponentHandler(null) → IConnectionPoint::disconnect → terminate
//! controller then component → release. The module itself is never
//! unloaded (see module.rs).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;

use vst3::Steinberg::Vst::ProcessContext_::StatesAndFlags_::{
    kPlaying, kSystemTimeValid, kTempoValid, kTimeSigValid,
};
use vst3::Steinberg::Vst::RestartFlags_::kLatencyChanged;
use vst3::Steinberg::Vst::SpeakerArr::{kMono, kStereo};
use vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, BusDirections_::kInput, BusDirections_::kOutput,
    IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentHandler, IComponentTrait,
    IConnectionPoint, IConnectionPointTrait, IEditController, IEditControllerTrait,
    IParameterChanges, MediaTypes_::kAudio, ProcessData, ProcessModes_::kRealtime, ProcessSetup,
    SpeakerArrangement, SymbolicSampleSizes_::kSample32, ProcessContext,
};
use vst3::Steinberg::{kResultOk, IPluginBaseTrait, TUID};
use vst3::{ComPtr, ComWrapper, Interface};

use crate::engine::render::BlockProcessor;
use crate::error::{Result, StillError};

use super::host::{ComponentHandler, NoParameterChanges};

extern "C" {
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

/// RAII autorelease pool: plugin code may autorelease ObjC objects on the
/// render thread, which has no pool of its own.
struct Pool(*mut c_void);
impl Pool {
    fn new() -> Self {
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

fn debug_on() -> bool {
    std::env::var("STILL_VST3_DEBUG").is_ok_and(|v| v != "0")
}

const MAX_BLOCK: usize = 4096;

pub struct Vst3Plugin {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// The controller is a distinct object (split architecture) and owes its
    /// own terminate; single-component plugins expose it via cast only.
    owns_separate_controller: bool,
    /// Kept alive for the plugin's lifetime (the controller holds a raw ptr).
    _handler: ComWrapper<ComponentHandler>,
    restart_flags: Arc<AtomicI32>,
    _no_param_changes: ComWrapper<NoParameterChanges>,
    param_changes_ptr: *mut IParameterChanges,

    channels: usize,
    sample_rate: u32,
    playing: Arc<AtomicBool>,
    pub bypass: bool,
    latency: u32,
    processing: bool,
    active: bool,
    /// Samples rendered since load/seek (drives projectTimeSamples).
    rendered: u64,
    error_count: u32,

    // Planar scratch buffers with stable pointer arrays for AudioBusBuffers.
    in_bufs: Vec<Vec<f32>>,
    out_bufs: Vec<Vec<f32>>,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,

    component_id: String,
}

// COM pointers cross threads only under the lifecycle contract documented
// in the module header; the engine only ever calls process().
unsafe impl Send for Vst3Plugin {}

fn cid_to_tuid(cid: &[u8; 16]) -> TUID {
    std::array::from_fn(|i| cid[i] as std::ffi::c_char)
}

/// Concurrent instantiation/disposal from the same module SIGSEGVs with
/// real plugins (observed: two Neutron instances racing in createInstance).
/// One process-wide lifecycle lock serializes new() and drop(); processing
/// stays fully parallel.
static LIFECYCLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lifecycle_guard() -> std::sync::MutexGuard<'static, ()> {
    LIFECYCLE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Vst3Plugin {
    pub fn new(
        component_id: &str,
        sample_rate: u32,
        channels: usize,
        playing: Arc<AtomicBool>,
    ) -> Result<Self> {
        let channels = channels.clamp(1, 2);
        let cid = super::parse_id(component_id).ok_or_else(|| {
            StillError::Playback(format!("invalid VST3 component id \"{component_id}\""))
        })?;
        let bundle = super::scan::bundle_for_cid(&cid).ok_or_else(|| {
            StillError::Playback(
                "This VST3 plugin is not installed on this machine (or has not been scanned yet)."
                    .into(),
            )
        })?;
        let _lifecycle = lifecycle_guard();
        let module = super::module::module_for(&bundle)?;
        let factory = module.factory();

        let tuid = cid_to_tuid(&cid);
        let component: ComPtr<IComponent> = unsafe {
            use vst3::Steinberg::IPluginFactoryTrait;
            let mut obj: *mut c_void = std::ptr::null_mut();
            let res = factory.createInstance(
                tuid.as_ptr(),
                IComponent::IID.as_ptr() as *const std::ffi::c_char,
                &mut obj,
            );
            if res != kResultOk || obj.is_null() {
                return Err(StillError::Playback(format!(
                    "VST3 createInstance failed ({res}) for {}",
                    bundle.display()
                )));
            }
            ComPtr::from_raw(obj as *mut IComponent).ok_or_else(|| {
                StillError::Playback("VST3 createInstance returned a null component".into())
            })?
        };

        unsafe {
            let res = component.initialize(super::host::host_application_funknown());
            if res != kResultOk {
                return Err(StillError::Playback(format!(
                    "VST3 component initialize failed ({res})"
                )));
            }
        }

        let processor = component.cast::<IAudioProcessor>().ok_or_else(|| {
            unsafe {
                component.terminate();
            }
            StillError::Playback("This VST3 plugin does not provide audio processing.".into())
        })?;

        // Controller: same object (cast) or a separate class instance.
        let mut owns_separate_controller = false;
        let controller: Option<ComPtr<IEditController>> =
            component.cast::<IEditController>().or_else(|| unsafe {
                use vst3::Steinberg::IPluginFactoryTrait;
                let mut ctrl_cid: TUID = [0; 16];
                if component.getControllerClassId(&mut ctrl_cid) != kResultOk {
                    return None;
                }
                let mut obj: *mut c_void = std::ptr::null_mut();
                let res = factory.createInstance(
                    ctrl_cid.as_ptr(),
                    IEditController::IID.as_ptr() as *const std::ffi::c_char,
                    &mut obj,
                );
                if res != kResultOk || obj.is_null() {
                    return None;
                }
                let ctrl = ComPtr::from_raw(obj as *mut IEditController)?;
                if ctrl.initialize(super::host::host_application_funknown()) != kResultOk {
                    return None;
                }
                owns_separate_controller = true;
                Some(ctrl)
            });

        // Component handler + component↔controller connection.
        let restart_flags = Arc::new(AtomicI32::new(0));
        let handler = ComWrapper::new(ComponentHandler {
            restart_flags: restart_flags.clone(),
        });
        if let Some(ctrl) = &controller {
            unsafe {
                let hptr = handler
                    .as_com_ref::<IComponentHandler>()
                    .map(|r| r.as_ptr())
                    .unwrap_or(std::ptr::null_mut());
                ctrl.setComponentHandler(hptr);
            }
            let comp_cp = component.cast::<IConnectionPoint>();
            let ctrl_cp = ctrl.cast::<IConnectionPoint>();
            if let (Some(a), Some(b)) = (&comp_cp, &ctrl_cp) {
                if a.as_ptr() != b.as_ptr() {
                    unsafe {
                        a.connect(b.as_ptr());
                        b.connect(a.as_ptr());
                    }
                }
            }
        }

        // Bus arrangements: force our layout on the MAIN buses with the
        // stereo → mono → plugin-defaults fallback, then activate bus 0
        // each way (aux/sidechain buses stay inactive).
        unsafe {
            let n_in = component.getBusCount(kAudio as i32, kInput as i32).max(0) as usize;
            let n_out = component.getBusCount(kAudio as i32, kOutput as i32).max(0) as usize;
            if n_out == 0 {
                component.terminate();
                return Err(StillError::Playback(
                    "This VST3 plugin has no audio output bus.".into(),
                ));
            }
            let want: SpeakerArrangement = if channels == 1 { kMono } else { kStereo };
            let mut ins: Vec<SpeakerArrangement> = vec![want; n_in.max(1)];
            let mut outs: Vec<SpeakerArrangement> = vec![want; n_out];
            let mut ok = processor.setBusArrangements(
                ins.as_mut_ptr(),
                n_in as i32,
                outs.as_mut_ptr(),
                n_out as i32,
            ) == kResultOk;
            if !ok && channels == 2 {
                ins.fill(kMono);
                outs.fill(kMono);
                ok = processor.setBusArrangements(
                    ins.as_mut_ptr(),
                    n_in as i32,
                    outs.as_mut_ptr(),
                    n_out as i32,
                ) == kResultOk;
            }
            if !ok && debug_on() {
                eprintln!("[vst3] {component_id}: keeping plugin default bus arrangement");
            }
            if n_in > 0 {
                component.activateBus(kAudio as i32, kInput as i32, 0, 1);
            }
            component.activateBus(kAudio as i32, kOutput as i32, 0, 1);
        }

        unsafe {
            if processor.canProcessSampleSize(kSample32 as i32) != kResultOk && debug_on() {
                eprintln!("[vst3] {component_id}: kSample32 not officially supported, trying anyway");
            }
            let mut setup = ProcessSetup {
                processMode: kRealtime as i32,
                symbolicSampleSize: kSample32 as i32,
                maxSamplesPerBlock: MAX_BLOCK as i32,
                sampleRate: sample_rate as f64,
            };
            if processor.setupProcessing(&mut setup) != kResultOk {
                component.terminate();
                return Err(StillError::Playback(format!(
                    "VST3 setupProcessing failed ({} Hz / {} ch)",
                    sample_rate, channels
                )));
            }
            if component.setActive(1) != kResultOk {
                component.terminate();
                return Err(StillError::Playback("VST3 setActive failed".into()));
            }
            processor.setProcessing(1);
        }

        let latency = unsafe { processor.getLatencySamples() };

        let no_param_changes = ComWrapper::new(NoParameterChanges);
        let param_changes_ptr = no_param_changes
            .as_com_ref::<IParameterChanges>()
            .map(|r| r.as_ptr())
            .unwrap_or(std::ptr::null_mut());

        let in_bufs = vec![vec![0.0f32; MAX_BLOCK]; channels];
        let out_bufs = vec![vec![0.0f32; MAX_BLOCK]; channels];

        let mut plugin = Vst3Plugin {
            component,
            processor,
            controller,
            owns_separate_controller,
            _handler: handler,
            restart_flags,
            _no_param_changes: no_param_changes,
            param_changes_ptr,
            channels,
            sample_rate,
            playing,
            bypass: false,
            latency,
            processing: true,
            active: true,
            rendered: 0,
            error_count: 0,
            in_bufs,
            out_bufs,
            in_ptrs: Vec::new(),
            out_ptrs: Vec::new(),
            component_id: component_id.to_string(),
        };
        plugin.rebuild_ptrs();
        if debug_on() {
            eprintln!(
                "[vst3] {component_id}: up ({} ch, {} Hz, latency {latency})",
                plugin.channels, plugin.sample_rate
            );
        }
        Ok(plugin)
    }

    fn rebuild_ptrs(&mut self) {
        self.in_ptrs = self.in_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        self.out_ptrs = self.out_bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
    }

    /// Capture component + controller states into one packed blob.
    pub fn get_state(&self) -> Option<Vec<u8>> {
        use vst3::Steinberg::IBStream;
        let comp = {
            let stream = ComWrapper::new(super::stream::MemoryStream::new());
            let ptr = stream.to_com_ptr::<IBStream>()?;
            if unsafe { self.component.getState(ptr.as_ptr()) } != kResultOk {
                return None;
            }
            drop(ptr);
            stream.take_data()
        };
        let ctrl = self
            .controller
            .as_ref()
            .and_then(|c| {
                let stream = ComWrapper::new(super::stream::MemoryStream::new());
                let ptr = stream.to_com_ptr::<IBStream>()?;
                (unsafe { c.getState(ptr.as_ptr()) } == kResultOk).then(|| {
                    drop(ptr);
                    stream.take_data()
                })
            })
            .unwrap_or_default();
        Some(super::stream::pack_state(&comp, &ctrl))
    }

    /// Restore a packed blob. VST3 protocol: component.setState, THEN
    /// controller.setComponentState (with the COMPONENT chunk), then the
    /// controller's own setState.
    pub fn set_state(&mut self, blob: &[u8]) -> bool {
        use vst3::Steinberg::IBStream;
        let Some((comp, ctrl)) = super::stream::unpack_state(blob) else {
            return false;
        };
        let ok = unsafe {
            let stream = ComWrapper::new(super::stream::MemoryStream::with_data(&comp));
            let Some(ptr) = stream.to_com_ptr::<IBStream>() else {
                return false;
            };
            self.component.setState(ptr.as_ptr()) == kResultOk
        };
        if let Some(c) = &self.controller {
            unsafe {
                let stream = ComWrapper::new(super::stream::MemoryStream::with_data(&comp));
                if let Some(ptr) = stream.to_com_ptr::<IBStream>() {
                    c.setComponentState(ptr.as_ptr());
                }
                if !ctrl.is_empty() {
                    let stream = ComWrapper::new(super::stream::MemoryStream::with_data(&ctrl));
                    if let Some(ptr) = stream.to_com_ptr::<IBStream>() {
                        c.setState(ptr.as_ptr());
                    }
                }
            }
        }
        ok
    }

    /// Create the plugin's native editor view (NOT yet attached). Main
    /// thread only. The caller sizes a window from `size()`, then calls
    /// `attach` with the container NSView.
    pub fn create_editor(&mut self) -> Result<Vst3Editor> {
        use vst3::Steinberg::IPlugViewTrait;
        use vst3::Steinberg::Vst::ViewType::kEditor;
        let ctrl = self.controller.as_ref().ok_or_else(|| {
            StillError::Playback("This plugin has no controller (no editor).".into())
        })?;
        let raw = unsafe { ((*(*ctrl.as_ptr()).vtbl).createView)(ctrl.as_ptr(), kEditor) };
        let view = unsafe { ComPtr::from_raw(raw) }.ok_or_else(|| {
            StillError::Playback("This plugin did not provide an editor view.".into())
        })?;
        unsafe {
            if view.isPlatformTypeSupported(vst3::Steinberg::kPlatformTypeNSView) != kResultOk {
                return Err(StillError::Playback(
                    "This plugin's editor does not support macOS windows.".into(),
                ));
            }
        }
        let mut rect = vst3::Steinberg::ViewRect {
            left: 0,
            top: 0,
            right: 560,
            bottom: 420,
        };
        unsafe {
            view.getSize(&mut rect);
        }
        let frame = ComWrapper::new(super::host::PlugFrame::new());
        unsafe {
            let fptr = frame
                .as_com_ref::<vst3::Steinberg::IPlugFrame>()
                .map(|r| r.as_ptr())
                .unwrap_or(std::ptr::null_mut());
            view.setFrame(fptr);
        }
        Ok(Vst3Editor {
            view,
            frame,
            width: (rect.right - rect.left).max(200),
            height: (rect.bottom - rect.top).max(120),
            attached: false,
        })
    }

    /// React to restartComponent flags from the controller thread.
    fn drain_restart_flags(&mut self) {
        let flags = self.restart_flags.swap(0, Ordering::AcqRel);
        if flags == 0 {
            return;
        }
        if flags & kLatencyChanged as i32 != 0 {
            self.latency = unsafe { self.processor.getLatencySamples() };
        }
        if debug_on() {
            eprintln!("[vst3] {}: restartComponent flags {flags:#x}", self.component_id);
        }
    }
}

impl BlockProcessor for Vst3Plugin {
    fn process(&mut self, buffer: &mut [f32], channels: usize, sample_rate: u32) {
        if self.bypass {
            return;
        }
        let channels = channels.clamp(1, 2).min(self.channels);
        let frames = (buffer.len() / channels).min(MAX_BLOCK);
        if frames == 0 {
            return;
        }
        let _pool = Pool::new();
        self.drain_restart_flags();

        // Deinterleave into the planar input scratch.
        for ch in 0..self.channels {
            let src_ch = ch.min(channels - 1);
            let dst = &mut self.in_bufs[ch];
            for f in 0..frames {
                dst[f] = buffer[f * channels + src_ch];
            }
            self.out_bufs[ch][..frames].fill(0.0);
        }

        let mut input_bus = AudioBusBuffers {
            numChannels: self.channels as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.in_ptrs.as_mut_ptr(),
            },
        };
        let mut output_bus = AudioBusBuffers {
            numChannels: self.channels as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.out_ptrs.as_mut_ptr(),
            },
        };

        let playing = self.playing.load(Ordering::Relaxed);
        let mut ctx: ProcessContext = unsafe { std::mem::zeroed() };
        // projectTimeSamples is unconditionally valid (no flag exists for it).
        ctx.state = (kTempoValid | kTimeSigValid | kSystemTimeValid) as u32
            | if playing { kPlaying as u32 } else { 0 };
        ctx.sampleRate = sample_rate as f64;
        ctx.projectTimeSamples = self.rendered as i64;
        ctx.systemTime = 0;
        ctx.tempo = 120.0;
        ctx.timeSigNumerator = 4;
        ctx.timeSigDenominator = 4;

        let mut data = ProcessData {
            processMode: kRealtime as i32,
            symbolicSampleSize: kSample32 as i32,
            numSamples: frames as i32,
            numInputs: 1,
            numOutputs: 1,
            inputs: &mut input_bus,
            outputs: &mut output_bus,
            inputParameterChanges: self.param_changes_ptr,
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: std::ptr::null_mut(),
            outputEvents: std::ptr::null_mut(),
            processContext: &mut ctx,
        };

        let res = unsafe { self.processor.process(&mut data) };
        self.rendered += frames as u64;
        if res != kResultOk {
            // Leave the dry interleaved buffer untouched.
            self.error_count += 1;
            if debug_on() && self.error_count.is_power_of_two() {
                eprintln!(
                    "[vst3] {}: process error {res} (count {})",
                    self.component_id, self.error_count
                );
            }
            return;
        }

        // Re-interleave the processed output.
        for ch in 0..channels {
            let src = &self.out_bufs[ch.min(self.channels - 1)];
            for f in 0..frames {
                buffer[f * channels + ch] = src[f];
            }
        }
    }

    fn latency_samples(&self) -> u32 {
        self.latency
    }

    fn reset(&mut self) {
        unsafe {
            self.processor.setProcessing(0);
            self.processor.setProcessing(1);
        }
        self.rendered = 0;
    }

    fn set_bypassed(&mut self, bypassed: bool) {
        self.bypass = bypassed;
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.get_state()
    }

    fn restore_state(&mut self, state: &[u8]) -> bool {
        self.set_state(state)
    }

    fn as_any(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// A live native editor view. ALL methods are main-thread only; the value
/// is Send so the app layer can store it in its registry between
/// main-thread hops.
pub struct Vst3Editor {
    view: ComPtr<vst3::Steinberg::IPlugView>,
    frame: ComWrapper<super::host::PlugFrame>,
    width: i32,
    height: i32,
    attached: bool,
}

unsafe impl Send for Vst3Editor {}

impl Vst3Editor {
    /// Preferred size reported by the plugin before attachment.
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// Attach the view to a container NSView (main thread).
    pub fn attach(&mut self, ns_view: *mut c_void) -> Result<()> {
        use vst3::Steinberg::IPlugViewTrait;
        if self.attached {
            return Ok(());
        }
        let res = unsafe { self.view.attached(ns_view, vst3::Steinberg::kPlatformTypeNSView) };
        if res != kResultOk {
            return Err(StillError::Playback(format!(
                "VST3 editor attach failed ({res})"
            )));
        }
        self.attached = true;
        Ok(())
    }

    /// Plugin-requested resize waiting to be applied (drained by the app's
    /// main-thread pump).
    pub fn take_pending_resize(&self) -> Option<(i32, i32)> {
        self.frame.take_pending_resize()
    }

    /// Tell the view its new size after the window was resized.
    pub fn on_size(&mut self, width: i32, height: i32) {
        use vst3::Steinberg::IPlugViewTrait;
        let mut rect = vst3::Steinberg::ViewRect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        unsafe {
            self.view.onSize(&mut rect);
        }
        self.width = width;
        self.height = height;
    }
}

impl Drop for Vst3Editor {
    /// Teardown protocol: setFrame(null) → removed() → release (ComPtr).
    fn drop(&mut self) {
        use vst3::Steinberg::IPlugViewTrait;
        unsafe {
            self.view.setFrame(std::ptr::null_mut());
            if self.attached {
                self.view.removed();
            }
        }
    }
}

impl Drop for Vst3Plugin {
    fn drop(&mut self) {
        let _lifecycle = lifecycle_guard();
        unsafe {
            if self.processing {
                self.processor.setProcessing(0);
            }
            if self.active {
                self.component.setActive(0);
            }
            if let Some(ctrl) = &self.controller {
                ctrl.setComponentHandler(std::ptr::null_mut());
                let comp_cp = self.component.cast::<IConnectionPoint>();
                let ctrl_cp = ctrl.cast::<IConnectionPoint>();
                if let (Some(a), Some(b)) = (&comp_cp, &ctrl_cp) {
                    if a.as_ptr() != b.as_ptr() {
                        a.disconnect(b.as_ptr());
                        b.disconnect(a.as_ptr());
                    }
                }
                if self.owns_separate_controller {
                    ctrl.terminate();
                }
            }
            self.component.terminate();
        }
        if debug_on() {
            eprintln!("[vst3] {}: disposed", self.component_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Locate an installed Neutron 5 Equalizer VST3 bundle, if any.
    fn neutron_eq_bundle() -> Option<PathBuf> {
        let root = Path::new("/Library/Audio/Plug-Ins/VST3");
        root.read_dir().ok()?.flatten().map(|e| e.path()).find(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy();
                    n.contains("Neutron 5") && n.contains("EQ") || n.contains("Neutron 5 Equalizer")
                })
                .unwrap_or(false)
        })
    }

    /// Register just the Neutron bundle via a symlink dir, then return the
    /// Equalizer's component id.
    fn setup_neutron() -> Option<(tempfile::TempDir, String)> {
        let bundle = neutron_eq_bundle()?;
        let tmp = tempfile::tempdir().ok()?;
        let link = tmp.path().join(bundle.file_name()?);
        std::os::unix::fs::symlink(&bundle, &link).ok()?;
        super::super::scan::scan_dirs(&[tmp.path().to_path_buf()]);
        let id = super::super::scan::list_effects()
            .into_iter()
            .find(|p| p.name.contains("Equalizer") || p.name.contains("EQ"))?
            .id;
        Some((tmp, id))
    }

    /// Full lifecycle + processing on a NON-main thread (the cargo test
    /// thread), which also validates the export worker-thread path.
    /// Graceful skip when Neutron 5 isn't installed.
    #[test]
    fn neutron_vst3_lifecycle_and_process() {
        let Some((_tmp, id)) = setup_neutron() else {
            eprintln!("skipped: Neutron 5 Equalizer VST3 not installed");
            return;
        };
        let playing = Arc::new(AtomicBool::new(true));
        let mut p = Vst3Plugin::new(&id, 48000, 2, playing).expect("instantiate Neutron VST3");
        assert!(p.latency_samples() < 48000);

        // 512-frame stereo sine block through the plugin.
        let frames = 512usize;
        let mut buf = vec![0.0f32; frames * 2];
        for f in 0..frames {
            let v = (f as f32 * 0.05).sin() * 0.5;
            buf[f * 2] = v;
            buf[f * 2 + 1] = v;
        }
        let dry = buf.clone();
        for _ in 0..8 {
            p.process(&mut buf, 2, 48000);
        }
        assert!(buf.iter().all(|s| s.is_finite()), "non-finite output");

        // Bypass must leave the buffer untouched.
        p.set_bypassed(true);
        let mut b2 = dry.clone();
        p.process(&mut b2, 2, 48000);
        assert_eq!(b2, dry);
        p.set_bypassed(false);

        // reset() (seek) then more processing must stay clean.
        p.reset();
        p.process(&mut buf, 2, 48000);
        assert!(buf.iter().all(|s| s.is_finite()));

        // Ordered drop must not hang or crash.
        drop(p);
    }

    /// State roundtrip across two instances (graceful skip without Neutron).
    #[test]
    fn neutron_vst3_state_roundtrip() {
        let Some((_tmp, id)) = setup_neutron() else {
            eprintln!("skipped: Neutron 5 Equalizer VST3 not installed");
            return;
        };
        let playing = Arc::new(AtomicBool::new(true));
        let p1 = Vst3Plugin::new(&id, 48000, 2, playing.clone()).expect("instantiate");
        let blob = p1.get_state().expect("get_state");
        assert!(super::super::stream::unpack_state(&blob).is_some());
        drop(p1);

        let mut p2 = Vst3Plugin::new(&id, 48000, 2, playing).expect("instantiate 2");
        assert!(p2.set_state(&blob), "set_state failed");
        // Still processes cleanly after a restore.
        let mut buf = vec![0.1f32; 512 * 2];
        p2.process(&mut buf, 2, 48000);
        assert!(buf.iter().all(|s| s.is_finite()));
        // Garbage must be rejected without side effects.
        assert!(!p2.set_state(b"not a packed state"));
    }
}
