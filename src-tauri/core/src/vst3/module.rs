//! Loaded .vst3 bundles.
//!
//! A module is loaded at most once per process and NEVER unloaded: plugin
//! dylibs register ObjC classes and spawn dispatch work whose teardown on
//! unload is undefined behaviour in practice. The global cache leaks each
//! module intentionally; the cost is a few MB per distinct plugin vendor.
//!
//! Load order (all mandatory, learned from real hosts): resolve the binary
//! inside the bundle → dlopen → `CFBundleCreate` + `bundleEntry(CFBundleRef)`
//! (many plugins — FabFilter among them — fail to expose IAudioProcessor if
//! bundleEntry never ran with a real CFBundleRef) → `GetPluginFactory` →
//! `IPluginFactory3::setHostContext` when available.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use vst3::Steinberg::{IPluginFactory, IPluginFactory3, IPluginFactory3Trait};
use vst3::ComPtr;

use crate::error::{Result, StillError};

use core_foundation_sys::base::CFRelease;
use core_foundation_sys::bundle::{CFBundleCreate, CFBundleRef};
use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithCString};
use core_foundation_sys::url::{kCFURLPOSIXPathStyle, CFURLCreateWithFileSystemPath};

type BundleEntryFn = unsafe extern "C" fn(CFBundleRef) -> bool;
type GetFactoryFn = unsafe extern "C" fn() -> *mut IPluginFactory;

pub struct Vst3Module {
    /// Keeps the dylib mapped for the process lifetime.
    _lib: libloading::Library,
    /// Retained CFBundle handed to bundleEntry; released never (module leaks).
    _bundle: CFBundleRef,
    factory: ComPtr<IPluginFactory>,
    pub path: PathBuf,
}

// The factory pointer is only ever used behind the module cache mutex or on
// the thread that resolved it; VST3 factories are required to be thread-safe.
unsafe impl Send for Vst3Module {}
unsafe impl Sync for Vst3Module {}

impl Vst3Module {
    pub fn factory(&self) -> &ComPtr<IPluginFactory> {
        &self.factory
    }
}

fn err(path: &Path, what: &str) -> StillError {
    StillError::Playback(format!("VST3 {}: {what}", path.display()))
}

/// The executable inside a .vst3 bundle: Contents/MacOS/<bundle stem>,
/// falling back to the first file in Contents/MacOS.
fn resolve_binary(bundle: &Path) -> Result<PathBuf> {
    let macos_dir = bundle.join("Contents/MacOS");
    let stem = bundle
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let candidate = macos_dir.join(&stem);
    if candidate.is_file() {
        return Ok(candidate);
    }
    let first = std::fs::read_dir(&macos_dir)
        .map_err(|_| err(bundle, "has no Contents/MacOS directory"))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_file());
    first.ok_or_else(|| err(bundle, "has no executable in Contents/MacOS"))
}

/// Create a retained CFBundleRef for the bundle directory.
fn create_cf_bundle(bundle: &Path) -> Result<CFBundleRef> {
    let cpath = std::ffi::CString::new(bundle.to_string_lossy().as_bytes())
        .map_err(|_| err(bundle, "path contains NUL"))?;
    unsafe {
        let s = CFStringCreateWithCString(std::ptr::null(), cpath.as_ptr(), kCFStringEncodingUTF8);
        if s.is_null() {
            return Err(err(bundle, "path is not valid UTF-8"));
        }
        let url = CFURLCreateWithFileSystemPath(std::ptr::null(), s, kCFURLPOSIXPathStyle, 1);
        CFRelease(s as *const c_void);
        if url.is_null() {
            return Err(err(bundle, "could not build bundle URL"));
        }
        let b = CFBundleCreate(std::ptr::null(), url);
        CFRelease(url as *const c_void);
        if b.is_null() {
            return Err(err(bundle, "CFBundleCreate failed"));
        }
        Ok(b)
    }
}

fn load(bundle: &Path) -> Result<Vst3Module> {
    let binary = resolve_binary(bundle)?;
    let lib = unsafe { libloading::Library::new(&binary) }
        .map_err(|e| err(bundle, &format!("failed to load: {e}")))?;

    let cf_bundle = create_cf_bundle(bundle)?;
    // bundleEntry is optional per spec but required by many real plugins.
    if let Ok(entry) = unsafe { lib.get::<BundleEntryFn>(b"bundleEntry") } {
        if !unsafe { entry(cf_bundle) } {
            unsafe { CFRelease(cf_bundle as *const c_void) };
            return Err(err(bundle, "bundleEntry returned false"));
        }
    }

    let get_factory = unsafe { lib.get::<GetFactoryFn>(b"GetPluginFactory") }
        .map_err(|_| err(bundle, "does not export GetPluginFactory"))?;
    let raw = unsafe { get_factory() };
    let factory = unsafe { ComPtr::from_raw(raw) }
        .ok_or_else(|| err(bundle, "GetPluginFactory returned null"))?;

    // Give the factory a host context before any createInstance (plugins may
    // reach host services during construction).
    if let Some(f3) = factory.cast::<IPluginFactory3>() {
        let host = super::host::host_application_funknown();
        unsafe {
            f3.setHostContext(host);
        }
    }

    Ok(Vst3Module {
        _lib: lib,
        _bundle: cf_bundle,
        factory,
        path: bundle.to_path_buf(),
    })
}

/// Process-global module cache. Modules are leaked deliberately (see module
/// docs); the map only ever grows.
static MODULES: Mutex<Option<HashMap<PathBuf, &'static Vst3Module>>> = Mutex::new(None);

pub fn module_for(bundle: &Path) -> Result<&'static Vst3Module> {
    let mut guard = MODULES.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(m) = map.get(bundle) {
        return Ok(m);
    }
    let module: &'static Vst3Module = Box::leak(Box::new(load(bundle)?));
    map.insert(bundle.to_path_buf(), module);
    Ok(module)
}
