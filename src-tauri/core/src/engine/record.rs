//! Multitrack input tracking (#8): record n mono layers from an audio
//! interface, tape-machine style. Reliability over features: the
//! callback only copies samples into a ring buffer; a writer thread
//! streams them to one 24-bit WAV per lane, fixing the headers up every
//! couple of seconds so a crash or a full disk never eats the take
//! beyond its last flush. Recording only ever creates NEW files
//! (ARCHITECTURE.md §3 bis).
//!
//! Inputs map incrementally: device input `first_input` feeds lane 1,
//! `first_input + 1` feeds lane 2, … All lanes share the device clock,
//! so the files land sample-synchronized — exactly what the synced
//! multitrack import expects.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Result, StillError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct InputDeviceInfo {
    /// Audio host exposing the device ("CoreAudio", "WASAPI", "ASIO", …).
    /// On Windows a pro interface may appear under both WASAPI (often as
    /// stereo endpoints) and ASIO (full channel count).
    pub host: String,
    pub name: String,
    /// Channel count of the device's default (mix) input format.
    pub channels: u16,
    pub sample_rate: u32,
    pub is_default: bool,
    /// Human names of each input, index-aligned with 1..=channels
    /// ("Input N" when the platform/driver names nothing).
    pub input_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RecordLane {
    /// Device input feeding this lane, 1-based.
    pub input: u16,
    /// Layer name — becomes the file name (and thus the layer's display
    /// name once loaded). "" = named after the input.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RecordConfig {
    /// Audio host of the device; "" = the platform's default host.
    #[serde(default)]
    pub host: String,
    /// Device name; "" = system default input.
    pub device: String,
    /// Lanes in file order; any input, any order, non-contiguous is fine.
    pub lanes: Vec<RecordLane>,
    /// Base folder; a fresh "Take N" subfolder is created inside.
    pub dest_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct RecordStatus {
    pub recording: bool,
    pub elapsed_seconds: f64,
    pub sample_rate: u32,
    /// Per-lane linear peak since the previous status poll.
    pub levels: Vec<f32>,
    /// Frames lost to ring overruns — must stay 0 on a healthy machine.
    #[ts(type = "number")]
    pub dropped_frames: u64,
    /// The take folder being written.
    pub folder: String,
    pub error: Option<String>,
}

/// Resolve a host by its cpal name; "" = the platform default.
fn host_by_name(name: &str) -> Option<cpal::Host> {
    if name.is_empty() {
        return Some(cpal::default_host());
    }
    cpal::available_hosts()
        .into_iter()
        .find(|id| id.name() == name)
        .and_then(|id| cpal::host_from_id(id).ok())
}

fn host_input_devices(host: &cpal::Host) -> Vec<InputDeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host_name = host.id().name().to_string();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices
        .filter_map(|d| {
            let name = d.name().ok()?;
            let cfg = d.default_input_config().ok()?;
            Some(InputDeviceInfo {
                host: host_name.clone(),
                is_default: name == default_name,
                channels: cfg.channels(),
                sample_rate: cfg.sample_rate().0,
                input_names: input_channel_names(&name, cfg.channels()),
                name,
            })
        })
        .collect()
}

/// Non-default hosts worth enumerating (ASIO on Windows). Loading ASIO
/// drivers is heavy and can touch the hardware, so the fresh list is
/// cached for the background watcher to reuse.
#[cfg(feature = "asio")]
fn extra_host_devices(fresh: bool) -> Vec<InputDeviceInfo> {
    static CACHE: Mutex<Option<Vec<InputDeviceInfo>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if !fresh {
        if let Some(list) = cache.as_ref() {
            return list.clone();
        }
    }
    let default_id = cpal::default_host().id();
    let list: Vec<InputDeviceInfo> = cpal::available_hosts()
        .into_iter()
        .filter(|id| *id != default_id)
        .filter_map(|id| cpal::host_from_id(id).ok())
        .flat_map(|h| host_input_devices(&h))
        .collect();
    *cache = Some(list.clone());
    list
}

#[cfg(not(feature = "asio"))]
fn extra_host_devices(_fresh: bool) -> Vec<InputDeviceInfo> {
    Vec::new()
}

/// Enumerate input devices at their default (mix) format — the Windows
/// resampler lesson applies to inputs too: we always open the device at
/// its own rate. The default host is listed first, then extra hosts
/// (ASIO) when compiled in.
pub fn list_input_devices() -> Vec<InputDeviceInfo> {
    list_input_devices_inner(true)
}

fn list_input_devices_inner(fresh_extra: bool) -> Vec<InputDeviceInfo> {
    let mut out = host_input_devices(&cpal::default_host());
    out.extend(extra_host_devices(fresh_extra));
    out
}

/// Watch the input-device topology on a dedicated thread and call
/// `notify` with the fresh list whenever it CHANGES (first call
/// included). On macOS a CoreAudio property listener wakes the thread
/// the instant a device is plugged or unplugged; a slow re-check keeps
/// every platform honest. Enumeration never touches the caller's
/// thread, so the UI stays fluid.
pub fn watch_input_devices(notify: impl Fn(Vec<InputDeviceInfo>) + Send + 'static) {
    std::thread::Builder::new()
        .name("still-devwatch".into())
        .spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            #[cfg(target_os = "macos")]
            coreaudio_names::install_devices_listener(tx);
            #[cfg(target_os = "windows")]
            wasapi_watch::install_devices_listener(tx);
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            drop(tx);
            let event_driven = cfg!(any(target_os = "macos", target_os = "windows"));
            let fallback = if event_driven {
                std::time::Duration::from_secs(30)
            } else {
                std::time::Duration::from_secs(3)
            };
            let mut last: Option<Vec<InputDeviceInfo>> = None;
            loop {
                // Never touch the HAL while a take is rolling: nothing is
                // allowed to compete with the recording.
                if WATCH_PAUSED.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    while rx.try_recv().is_ok() {}
                    continue;
                }
                // The watcher reuses the cached ASIO snapshot: loading
                // ASIO drivers repeatedly is not a background activity.
                let list = list_input_devices_inner(false);
                if last.as_ref() != Some(&list) {
                    last = Some(list.clone());
                    notify(list);
                }
                match rx.recv_timeout(fallback) {
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // No listener wired: plain slow polling.
                        std::thread::sleep(fallback);
                    }
                    _ => {
                        // Coalesce bursts (unplug fires several events).
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        while rx.try_recv().is_ok() {}
                    }
                }
            }
        })
        .expect("spawn device watcher");
}

/// Human names of a device's input channels. On macOS CoreAudio exposes
/// per-element names (interfaces label them "MIC 1", "SPDIF L", …);
/// everywhere else — and whenever the driver names nothing — fall back
/// to "Input N".
fn input_channel_names(device_name: &str, channels: u16) -> Vec<String> {
    let _ = &device_name; // only read on macOS
    #[cfg(target_os = "macos")]
    {
        if let Some(names) = coreaudio_names::channel_names(device_name, channels) {
            return names;
        }
    }
    (1..=channels).map(|n| format!("Input {n}")).collect()
}

/// Minimal CoreAudio property queries (the framework is already linked
/// through cpal). Only reads: device list, names, input element names.
#[cfg(target_os = "macos")]
mod coreaudio_names {
    use std::ffi::c_void;

    #[repr(C)]
    struct PropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectAddPropertyListener(
            object: u32,
            address: *const PropertyAddress,
            listener: extern "C" fn(u32, u32, *const PropertyAddress, *mut c_void) -> i32,
            client_data: *mut c_void,
        ) -> i32;
        fn AudioObjectGetPropertyDataSize(
            object: u32,
            address: *const PropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
        ) -> i32;
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const PropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringGetCString(s: *const c_void, buf: *mut u8, len: isize, encoding: u32) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    const SYSTEM_OBJECT: u32 = 1; // kAudioObjectSystemObject
    const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
    const fn fourcc(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }
    const DEVICES: u32 = fourcc(b"dev#"); // kAudioHardwarePropertyDevices
    const NAME: u32 = fourcc(b"lnam"); // kAudioObjectPropertyName
    const ELEMENT_NAME: u32 = fourcc(b"lchn"); // kAudioObjectPropertyElementName
    const SCOPE_GLOBAL: u32 = fourcc(b"glob");
    const SCOPE_INPUT: u32 = fourcc(b"inpt");
    const ELEMENT_MAIN: u32 = 0;

    unsafe fn cf_to_string(cf: *const c_void) -> Option<String> {
        if cf.is_null() {
            return None;
        }
        let mut buf = [0u8; 512];
        let ok = CFStringGetCString(cf, buf.as_mut_ptr(), buf.len() as isize, UTF8);
        CFRelease(cf);
        if ok == 0 {
            return None;
        }
        let end = buf.iter().position(|b| *b == 0).unwrap_or(0);
        Some(String::from_utf8_lossy(&buf[..end]).to_string())
    }

    unsafe fn object_string(object: u32, selector: u32, scope: u32, element: u32) -> Option<String> {
        let addr = PropertyAddress { selector, scope, element };
        let mut cf: *const c_void = std::ptr::null();
        let mut size = std::mem::size_of::<*const c_void>() as u32;
        let status = AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            &mut cf as *mut _ as *mut c_void,
        );
        if status != 0 {
            return None;
        }
        cf_to_string(cf)
    }

    /// Signal `tx` whenever the system's device list changes. The HAL
    /// delivers notifications on its own thread; the sender lives behind
    /// a Mutex (leaked once per process) to stay Sync.
    pub fn install_devices_listener(tx: std::sync::mpsc::Sender<()>) {
        extern "C" fn on_change(
            _object: u32,
            _count: u32,
            _addresses: *const PropertyAddress,
            data: *mut c_void,
        ) -> i32 {
            let tx = unsafe { &*(data as *const std::sync::Mutex<std::sync::mpsc::Sender<()>>) };
            if let Ok(tx) = tx.lock() {
                let _ = tx.send(());
            }
            0
        }
        let addr = PropertyAddress {
            selector: DEVICES,
            scope: SCOPE_GLOBAL,
            element: ELEMENT_MAIN,
        };
        let leaked: &'static _ = Box::leak(Box::new(std::sync::Mutex::new(tx)));
        unsafe {
            AudioObjectAddPropertyListener(
                SYSTEM_OBJECT,
                &addr,
                on_change,
                leaked as *const _ as *mut c_void,
            );
        }
    }

    pub fn channel_names(device_name: &str, channels: u16) -> Option<Vec<String>> {
        unsafe {
            // Enumerate device IDs, match by name.
            let addr = PropertyAddress {
                selector: DEVICES,
                scope: SCOPE_GLOBAL,
                element: ELEMENT_MAIN,
            };
            let mut size = 0u32;
            if AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &addr, 0, std::ptr::null(), &mut size)
                != 0
            {
                return None;
            }
            let count = size as usize / std::mem::size_of::<u32>();
            let mut ids = vec![0u32; count];
            if AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &addr,
                0,
                std::ptr::null(),
                &mut size,
                ids.as_mut_ptr() as *mut c_void,
            ) != 0
            {
                return None;
            }
            let device = ids.into_iter().find(|id| {
                object_string(*id, NAME, SCOPE_GLOBAL, ELEMENT_MAIN).as_deref()
                    == Some(device_name)
            })?;
            let names: Vec<String> = (1..=channels as u32)
                .map(|el| {
                    object_string(device, ELEMENT_NAME, SCOPE_INPUT, el)
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| format!("Input {el}"))
                })
                .collect();
            Some(names)
        }
    }
}

/// Event-driven device-change notifications on Windows: an
/// `IMMNotificationClient` registered with the MMDevice enumerator wakes
/// the watcher on add/remove/state/default changes. Lives on its own
/// MTA thread; enumerator + callback stay alive for the process.
#[cfg(target_os = "windows")]
mod wasapi_watch {
    use std::sync::mpsc::Sender;
    use std::sync::Mutex;
    use windows::core::{implement, Result, PCWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
        IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    #[implement(IMMNotificationClient)]
    struct Client {
        tx: Mutex<Sender<()>>,
    }

    impl Client {
        fn ping(&self) {
            if let Ok(tx) = self.tx.lock() {
                let _ = tx.send(());
            }
        }
    }

    impl IMMNotificationClient_Impl for Client_Impl {
        fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
            self.ping();
            Ok(())
        }
        fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
            self.ping();
            Ok(())
        }
        fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
            self.ping();
            Ok(())
        }
        fn OnDefaultDeviceChanged(
            &self,
            _flow: EDataFlow,
            _role: ERole,
            _id: &PCWSTR,
        ) -> Result<()> {
            self.ping();
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }

    pub fn install_devices_listener(tx: Sender<()>) {
        std::thread::Builder::new()
            .name("still-mmnotify".into())
            .spawn(move || unsafe {
                if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                    return;
                }
                let Ok(enumerator): windows::core::Result<IMMDeviceEnumerator> =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                else {
                    return;
                };
                let client: IMMNotificationClient = Client { tx: Mutex::new(tx) }.into();
                if enumerator
                    .RegisterEndpointNotificationCallback(&client)
                    .is_err()
                {
                    return;
                }
                // Keep the registration alive forever.
                loop {
                    std::thread::park();
                }
            })
            .expect("spawn MM notification thread");
    }
}

// ---------------------------------------------------------------------------
// Streaming mono 24-bit WAV writer with periodic header fixups.

/// WAV data is capped at 4 GB; stop cleanly before the u32 sizes wrap
/// (≈ 7 h 45 of mono 24-bit at 48 kHz).
const MAX_DATA_BYTES: u64 = u32::MAX as u64 - 128;

pub(crate) struct WavLane {
    w: BufWriter<File>,
    data_bytes: u64,
}

impl WavLane {
    pub(crate) fn create(path: &Path, sample_rate: u32) -> std::io::Result<Self> {
        let f = File::create(path)?;
        let mut w = BufWriter::new(f);
        // Mono 24-bit PCM header, sizes patched by fixup()/finalize().
        w.write_all(b"RIFF")?;
        w.write_all(&36u32.to_le_bytes())?;
        w.write_all(b"WAVEfmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        w.write_all(&1u16.to_le_bytes())?; // PCM
        w.write_all(&1u16.to_le_bytes())?; // mono
        w.write_all(&sample_rate.to_le_bytes())?;
        w.write_all(&(sample_rate * 3).to_le_bytes())?; // byte rate
        w.write_all(&3u16.to_le_bytes())?; // block align
        w.write_all(&24u16.to_le_bytes())?; // bits
        w.write_all(b"data")?;
        w.write_all(&0u32.to_le_bytes())?;
        Ok(Self { w, data_bytes: 0 })
    }

    pub(crate) fn write_sample(&mut self, v: f32) -> std::io::Result<()> {
        let s = (v.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
        self.w.write_all(&s.to_le_bytes()[..3])?;
        self.data_bytes += 3;
        if self.data_bytes >= MAX_DATA_BYTES {
            return Err(std::io::Error::other(
                "WAV size limit reached (4 GB) — take stopped",
            ));
        }
        Ok(())
    }

    /// Flush samples and rewrite the RIFF/data sizes so the file is a
    /// valid WAV as of NOW, then return to the append position.
    pub(crate) fn fixup(&mut self) -> std::io::Result<()> {
        self.w.flush()?;
        let f = self.w.get_mut();
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&((36 + self.data_bytes) as u32).to_le_bytes())?;
        f.seek(SeekFrom::Start(40))?;
        f.write_all(&(self.data_bytes as u32).to_le_bytes())?;
        f.seek(SeekFrom::End(0))?;
        Ok(())
    }

    pub(crate) fn finalize(mut self) -> std::io::Result<()> {
        // RIFF chunks are even-padded; the pad byte is not counted.
        if self.data_bytes % 2 == 1 {
            self.w.write_all(&[0])?;
        }
        self.fixup()?;
        self.w.get_mut().sync_all()?;
        Ok(())
    }
}

/// While true, the device watcher stays away from the audio HAL.
static WATCH_PAUSED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------

struct RecShared {
    stop: AtomicBool,
    frames_written: AtomicU64,
    dropped: AtomicU64,
    /// Per-lane max-hold peak (f32 bits), reset by each status poll.
    levels: Vec<AtomicU32>,
    error: Mutex<Option<String>>,
}

pub struct RecorderHandle {
    shared: Arc<RecShared>,
    thread: Option<std::thread::JoinHandle<()>>,
    sample_rate: u32,
    folder: PathBuf,
    files: Vec<PathBuf>,
}

/// Next free "Take N" folder inside `base`.
fn take_folder(base: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(base)?;
    for n in 1..10_000 {
        let candidate = base.join(format!("Take {n}"));
        if !candidate.exists() {
            std::fs::create_dir(&candidate)?;
            return Ok(candidate);
        }
    }
    Err(std::io::Error::other("no free take folder name"))
}

impl RecorderHandle {
    /// Open the device and start recording. Returns once the stream is
    /// live (or with the device's error).
    pub fn start(cfg: &RecordConfig) -> Result<Self> {
        use cpal::traits::{DeviceTrait, HostTrait};
        if cfg.lanes.is_empty() {
            return Err(StillError::InvalidProject(
                "at least one input must be recorded".into(),
            ));
        }
        if cfg.lanes.iter().any(|l| l.input == 0) {
            return Err(StillError::InvalidProject(
                "inputs are numbered from 1".into(),
            ));
        }
        if cfg.dest_dir.trim().is_empty() {
            return Err(StillError::InvalidProject(
                "no recording folder selected".into(),
            ));
        }
        let host = host_by_name(&cfg.host).ok_or_else(|| {
            StillError::Audio(format!("audio host \"{}\" is not available", cfg.host))
        })?;
        let device = if cfg.device.is_empty() {
            host.default_input_device()
        } else {
            host.input_devices()
                .ok()
                .and_then(|mut it| it.find(|d| d.name().map(|n| n == cfg.device).unwrap_or(false)))
        }
        .ok_or_else(|| {
            StillError::Audio(format!("input device \"{}\" not found", cfg.device))
        })?;
        // Freeze the device watcher for the whole take.
        WATCH_PAUSED.store(true, Ordering::SeqCst);
        let unpause = scopeguard();
        struct Unpause(bool);
        fn scopeguard() -> Unpause {
            Unpause(true)
        }
        impl Unpause {
            fn disarm(mut self) {
                self.0 = false;
            }
        }
        impl Drop for Unpause {
            fn drop(&mut self) {
                if self.0 {
                    WATCH_PAUSED.store(false, Ordering::SeqCst);
                }
            }
        }
        let sup = device
            .default_input_config()
            .map_err(|e| StillError::Audio(format!("input device unavailable: {e}")))?;
        let dev_ch = sup.channels() as usize;
        let rate = sup.sample_rate().0;
        let lanes = cfg.lanes.len();
        if let Some(bad) = cfg.lanes.iter().find(|l| l.input as usize > dev_ch) {
            return Err(StillError::Audio(format!(
                "the device exposes {dev_ch} input(s) — input {} does not exist",
                bad.input
            )));
        }
        // 0-based device channel per lane, in file order.
        let inputs: Vec<usize> = cfg.lanes.iter().map(|l| l.input as usize - 1).collect();

        let folder = take_folder(Path::new(cfg.dest_dir.trim())).map_err(StillError::Io)?;
        let mut writers = Vec::with_capacity(lanes);
        let mut files = Vec::with_capacity(lanes);
        for (i, lane) in cfg.lanes.iter().enumerate() {
            // The file name IS the layer name once loaded as multitrack.
            let base = crate::naming::sanitize_filename(lane.name.trim());
            let base = if base.is_empty() {
                format!("Input {:02}", lane.input)
            } else {
                base
            };
            let path = folder.join(format!("{:02} - {base}.wav", i + 1));
            writers.push(WavLane::create(&path, rate).map_err(StillError::Io)?);
            files.push(path);
        }

        let shared = Arc::new(RecShared {
            stop: AtomicBool::new(false),
            frames_written: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            levels: (0..lanes).map(|_| AtomicU32::new(0)).collect(),
            error: Mutex::new(None),
        });

        // ~4 seconds of headroom between the callback and the disk.
        let capacity = ((rate as usize * 4).next_power_of_two()) * lanes;
        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(capacity);

        let shared_cb = shared.clone();
        let cb_inputs = inputs.clone();
        let mut on_input = move |frames: &[f32]| {
            // `frames` is interleaved with dev_ch channels; keep lanes.
            for frame in frames.chunks_exact(dev_ch) {
                if producer.slots() < lanes {
                    shared_cb.dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                for (ch, level) in cb_inputs.iter().zip(shared_cb.levels.iter()) {
                    let v = frame[*ch];
                    let _ = producer.push(v);
                    let a = v.abs();
                    let _ = level.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bits| {
                        (a > f32::from_bits(bits)).then(|| a.to_bits())
                    });
                }
            }
        };

        // The cpal stream is !Send: build and own it on the writer
        // thread; report the open result through a channel.
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<std::result::Result<(), String>>(1);
        let shared_thread = shared.clone();
        let config = cpal::StreamConfig {
            channels: dev_ch as u16,
            sample_rate: cpal::SampleRate(rate),
            buffer_size: cpal::BufferSize::Default,
        };
        let sample_format = sup.sample_format();
        let thread = std::thread::Builder::new()
            .name("still-record".into())
            .spawn(move || {
                use cpal::traits::{DeviceTrait, StreamTrait};
                let err_shared = shared_thread.clone();
                let on_err = move |e: cpal::StreamError| {
                    *err_shared.error.lock().unwrap() = Some(format!("input stream error: {e}"));
                };
                let stream = match sample_format {
                    cpal::SampleFormat::F32 => device.build_input_stream(
                        &config,
                        move |data: &[f32], _: &_| on_input(data),
                        on_err,
                        None,
                    ),
                    cpal::SampleFormat::I16 => {
                        let mut buf = Vec::new();
                        device.build_input_stream(
                            &config,
                            move |data: &[i16], _: &_| {
                                buf.clear();
                                buf.extend(data.iter().map(|s| *s as f32 / 32_768.0));
                                on_input(&buf);
                            },
                            on_err,
                            None,
                        )
                    }
                    cpal::SampleFormat::U16 => {
                        let mut buf = Vec::new();
                        device.build_input_stream(
                            &config,
                            move |data: &[u16], _: &_| {
                                buf.clear();
                                buf.extend(
                                    data.iter().map(|s| (*s as f32 - 32_768.0) / 32_768.0),
                                );
                                on_input(&buf);
                            },
                            on_err,
                            None,
                        )
                    }
                    other => {
                        let _ = ready_tx.send(Err(format!(
                            "unsupported input sample format {other:?}"
                        )));
                        return;
                    }
                };
                let stream = match stream
                    .map_err(|e| e.to_string())
                    .and_then(|s| s.play().map(|_| s).map_err(|e| e.to_string()))
                {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("cannot open the input: {e}")));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));

                // Drain loop: ring → lane files, header fixup every ~2 s.
                let fixup_every = (rate as u64) * 2;
                let mut since_fixup = 0u64;
                let fail = |msg: String, shared: &Arc<RecShared>| {
                    *shared.error.lock().unwrap() = Some(msg);
                    shared.stop.store(true, Ordering::SeqCst);
                };
                'run: loop {
                    let stopping = shared_thread.stop.load(Ordering::SeqCst);
                    let mut drained = 0u64;
                    while consumer.slots() >= lanes {
                        for w in writers.iter_mut() {
                            let v = consumer.pop().unwrap_or(0.0);
                            if let Err(e) = w.write_sample(v) {
                                fail(e.to_string(), &shared_thread);
                                break 'run;
                            }
                        }
                        drained += 1;
                    }
                    if drained > 0 {
                        shared_thread
                            .frames_written
                            .fetch_add(drained, Ordering::Relaxed);
                        since_fixup += drained;
                        if since_fixup >= fixup_every {
                            since_fixup = 0;
                            for w in writers.iter_mut() {
                                if let Err(e) = w.fixup() {
                                    fail(e.to_string(), &shared_thread);
                                    break 'run;
                                }
                            }
                        }
                    }
                    if stopping {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                drop(stream); // stop capturing before finalizing
                for w in writers.drain(..) {
                    if let Err(e) = w.finalize() {
                        *shared_thread.error.lock().unwrap() = Some(e.to_string());
                    }
                }
            })
            .map_err(|e| StillError::Audio(e.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {
                unpause.disarm();
                Ok(Self {
                    shared,
                    thread: Some(thread),
                    sample_rate: rate,
                    folder,
                    files,
                })
            }
            Ok(Err(msg)) => {
                let _ = thread.join();
                let _ = std::fs::remove_dir_all(&folder);
                Err(StillError::Audio(msg))
            }
            Err(_) => {
                let _ = std::fs::remove_dir_all(&folder);
                Err(StillError::Audio("the recording thread died".into()))
            }
        }
    }

    pub fn status(&self) -> RecordStatus {
        let levels = self
            .shared
            .levels
            .iter()
            .map(|l| f32::from_bits(l.swap(0, Ordering::Relaxed)))
            .collect();
        RecordStatus {
            recording: !self.shared.stop.load(Ordering::SeqCst),
            elapsed_seconds: self.shared.frames_written.load(Ordering::Relaxed) as f64
                / self.sample_rate.max(1) as f64,
            sample_rate: self.sample_rate,
            levels,
            dropped_frames: self.shared.dropped.load(Ordering::Relaxed),
            folder: self.folder.display().to_string(),
            error: self.shared.error.lock().unwrap().clone(),
        }
    }

    /// Stop, finalize every lane and return the recorded files.
    pub fn stop(mut self) -> Result<Vec<PathBuf>> {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        WATCH_PAUSED.store(false, Ordering::SeqCst);
        if let Some(e) = self.shared.error.lock().unwrap().clone() {
            return Err(StillError::Audio(format!("recording ended with: {e}")));
        }
        Ok(self.files.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lane writer produces valid mono 24-bit WAVs, stays readable
    /// after a fixup even if never finalized (crash recovery), and the
    /// sample conversion round-trips.
    #[test]
    fn wav_lane_writes_and_survives_crash() {
        let dir = tempfile::tempdir().unwrap();
        let finalized = dir.path().join("a.wav");
        let crashed = dir.path().join("b.wav");
        let values = [0.0f32, 0.5, -0.5, 1.0, -1.0, 0.25, 2.0 /* clamped */];

        let mut a = WavLane::create(&finalized, 48_000).unwrap();
        let mut b = WavLane::create(&crashed, 48_000).unwrap();
        for v in values {
            a.write_sample(v).unwrap();
            b.write_sample(v).unwrap();
        }
        a.finalize().unwrap();
        b.fixup().unwrap();
        std::mem::forget(b); // crash: no finalize, no Drop flush

        for path in [&finalized, &crashed] {
            let mut r = hound::WavReader::open(path).unwrap();
            let spec = r.spec();
            assert_eq!(spec.channels, 1);
            assert_eq!(spec.sample_rate, 48_000);
            assert_eq!(spec.bits_per_sample, 24);
            let got: Vec<i32> = r.samples::<i32>().map(|s| s.unwrap()).collect();
            assert_eq!(got.len(), values.len(), "{path:?}");
            for (g, v) in got.iter().zip(values) {
                let expect = (v.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                assert_eq!(*g, expect);
            }
        }
    }

    /// End-to-end against the REAL default input device — needs audio
    /// hardware, so it only runs when asked for explicitly:
    /// `cargo test -p still-core --lib record_from_default_device -- --ignored`
    #[test]
    #[ignore]
    fn record_from_default_device() {
        let dir = tempfile::tempdir().unwrap();
        let devices = list_input_devices();
        assert!(!devices.is_empty(), "no input device on this machine");
        // STILL_TEST_INPUT_DEVICE / STILL_TEST_INPUTS ("1,22,3") pick a
        // specific interface and a non-linear lane mapping.
        let device = std::env::var("STILL_TEST_INPUT_DEVICE").unwrap_or_default();
        let lanes: Vec<RecordLane> = std::env::var("STILL_TEST_INPUTS")
            .unwrap_or_else(|_| "1".into())
            .split(',')
            .filter_map(|v| v.trim().parse::<u16>().ok())
            .enumerate()
            .map(|(i, input)| RecordLane {
                input,
                name: format!("Lane {}", i + 1),
            })
            .collect();
        let n = lanes.len();
        let cfg = RecordConfig {
            host: String::new(),
            device,
            lanes,
            dest_dir: dir.path().display().to_string(),
        };
        let handle = RecorderHandle::start(&cfg).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let st = handle.status();
        assert!(st.recording);
        assert!(st.elapsed_seconds > 0.5, "elapsed {}", st.elapsed_seconds);
        assert_eq!(st.dropped_frames, 0);
        let files = handle.stop().unwrap();
        assert_eq!(files.len(), n);
        let mut durations = Vec::new();
        for f in &files {
            let r = hound::WavReader::open(f).unwrap();
            assert_eq!(r.spec().bits_per_sample, 24);
            assert!(r.duration() > r.spec().sample_rate, "less than 1 s in {f:?}");
            durations.push(r.duration());
        }
        assert!(
            durations.windows(2).all(|w| w[0] == w[1]),
            "lanes not sample-synced: {durations:?}"
        );
        eprintln!("recorded {files:?}");
    }

    /// Print the input devices this machine exposes (manual check).
    #[test]
    #[ignore]
    fn print_input_devices() {
        for d in list_input_devices() {
            eprintln!(
                "{}{} — {} ch @ {} Hz",
                d.name,
                if d.is_default { " (default)" } else { "" },
                d.channels,
                d.sample_rate
            );
            for (i, n) in d.input_names.iter().enumerate() {
                eprintln!("   {:02}: {n}", i + 1);
            }
        }
    }

    /// "Take N" folders never collide.
    #[test]
    fn take_folders_increment() {
        let dir = tempfile::tempdir().unwrap();
        let a = take_folder(dir.path()).unwrap();
        let b = take_folder(dir.path()).unwrap();
        assert_eq!(a.file_name().unwrap(), "Take 1");
        assert_eq!(b.file_name().unwrap(), "Take 2");
    }
}
