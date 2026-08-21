//! VST3 plugin discovery.
//!
//! macOS VST3 bundles carry no parseable metadata (none of the installed
//! bundles ship moduleinfo.json), so scanning means loading every bundle and
//! asking its factory. Loading dozens of plugin dylibs is slow AND poisons
//! the process (duplicate ObjC classes, static destructors that SIGSEGV at
//! exit — observed with the UA suite). So the APP process never scans:
//! it only reads the JSON disk cache, and the app layer runs the actual
//! scan (`full_scan_blocking`) in a throwaway subprocess whose exit crash
//! is harmless. In-process module loading only happens at instantiation
//! time, for the one bundle a chain actually uses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use vst3::Steinberg::{
    kResultOk, IPluginFactory2, IPluginFactory2Trait, IPluginFactoryTrait, PClassInfo, PClassInfo2,
    PFactoryInfo,
};

use crate::plugins::{PluginFormat, PluginInfo};

/// One plugin class found in a bundle (cached representation).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScannedClass {
    cid_hex: String,
    name: String,
    vendor: String,
    /// Raw VST3 subcategory string, e.g. "Fx|EQ" or "Instrument|Synth".
    #[serde(default)]
    subcategories: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScannedBundle {
    path: PathBuf,
    /// mtime (secs since epoch) of the bundle executable at scan time.
    binary_mtime: u64,
    classes: Vec<ScannedClass>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ScanCache {
    bundles: Vec<ScannedBundle>,
}

#[derive(Default)]
struct ScanState {
    cache_path: Option<PathBuf>,
    scanned: bool,
    bundles: Vec<ScannedBundle>,
    /// cid → bundle path, for every class of every format (not only Fx).
    registry: HashMap<[u8; 16], PathBuf>,
}

static STATE: Mutex<Option<ScanState>> = Mutex::new(None);

fn debug_on() -> bool {
    std::env::var("STILL_VST3_DEBUG").is_ok_and(|v| v != "0")
}

/// Where the scan cache lives (set once by the app layer at startup;
/// scanning works without it, just slower).
pub fn set_cache_path(path: PathBuf) {
    let mut guard = STATE.lock().unwrap();
    guard.get_or_insert_with(Default::default).cache_path = Some(path);
}

fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Library/Audio/Plug-Ins/VST3")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
    }
    dirs
}

/// Recursively collect .vst3 bundle directories (never descending into one).
fn discover_bundles(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("vst3") {
            out.push(path);
        } else if path.is_dir() {
            discover_bundles(&path, out);
        }
    }
}

fn binary_mtime(bundle: &Path) -> u64 {
    let macos = bundle.join("Contents/MacOS");
    let stem = bundle
        .file_stem()
        .map(|s| macos.join(s))
        .filter(|p| p.is_file());
    let bin = stem.or_else(|| {
        std::fs::read_dir(&macos)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_file())
    });
    bin.and_then(|p| std::fs::metadata(&p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn c_chars_to_string(chars: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

fn cid_hex(cid: &[std::ffi::c_char; 16]) -> String {
    let bytes: [u8; 16] = std::array::from_fn(|i| cid[i] as u8);
    super::cid_to_id(&bytes)
        .strip_prefix(super::ID_PREFIX)
        .unwrap()
        .to_string()
}

/// Load a bundle and enumerate its "Audio Module Class" entries.
fn enumerate_bundle(bundle: &Path) -> Result<Vec<ScannedClass>, String> {
    let module = super::module::module_for(bundle).map_err(|e| e.to_string())?;
    let factory = module.factory();

    let mut factory_vendor = String::new();
    unsafe {
        let mut info: PFactoryInfo = std::mem::zeroed();
        if factory.getFactoryInfo(&mut info) == kResultOk {
            factory_vendor = c_chars_to_string(&info.vendor);
        }
    }

    let factory2 = factory.cast::<IPluginFactory2>();
    let count = unsafe { factory.countClasses() };
    let mut classes = Vec::new();
    for index in 0..count {
        let mut cls: Option<ScannedClass> = None;
        if let Some(f2) = &factory2 {
            unsafe {
                let mut info: PClassInfo2 = std::mem::zeroed();
                if f2.getClassInfo2(index, &mut info) == kResultOk {
                    if c_chars_to_string(&info.category) != "Audio Module Class" {
                        continue;
                    }
                    let vendor = c_chars_to_string(&info.vendor);
                    cls = Some(ScannedClass {
                        cid_hex: cid_hex(&info.cid),
                        name: c_chars_to_string(&info.name),
                        vendor: if vendor.is_empty() {
                            factory_vendor.clone()
                        } else {
                            vendor
                        },
                        subcategories: c_chars_to_string(&info.subCategories),
                    });
                }
            }
        }
        if cls.is_none() {
            unsafe {
                let mut info: PClassInfo = std::mem::zeroed();
                if factory.getClassInfo(index, &mut info) == kResultOk {
                    if c_chars_to_string(&info.category) != "Audio Module Class" {
                        continue;
                    }
                    cls = Some(ScannedClass {
                        cid_hex: cid_hex(&info.cid),
                        name: c_chars_to_string(&info.name),
                        vendor: factory_vendor.clone(),
                        subcategories: String::new(),
                    });
                }
            }
        }
        if let Some(c) = cls {
            classes.push(c);
        }
    }
    Ok(classes)
}

fn load_cache(path: &Path) -> ScanCache {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_cache(path: &Path, bundles: &[ScannedBundle]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let cache = ScanCache {
        bundles: bundles.to_vec(),
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = std::fs::write(path, json);
    }
}

fn rescan(state: &mut ScanState, dirs: &[PathBuf]) {
    let cached: HashMap<PathBuf, ScannedBundle> = state
        .cache_path
        .as_deref()
        .map(load_cache)
        .unwrap_or_default()
        .bundles
        .into_iter()
        .map(|b| (b.path.clone(), b))
        .collect();

    let mut found = Vec::new();
    for dir in dirs {
        discover_bundles(dir, &mut found);
    }
    found.sort();
    found.dedup();

    let mut bundles = Vec::new();
    for bundle in found {
        let mtime = binary_mtime(&bundle);
        if let Some(hit) = cached.get(&bundle) {
            if hit.binary_mtime == mtime {
                bundles.push(hit.clone());
                continue;
            }
        }
        match enumerate_bundle(&bundle) {
            Ok(classes) => bundles.push(ScannedBundle {
                path: bundle,
                binary_mtime: mtime,
                classes,
            }),
            Err(e) => {
                if debug_on() {
                    eprintln!("[vst3] skipping {}: {e}", bundle.display());
                }
            }
        }
    }

    if let Some(cache_path) = state.cache_path.clone() {
        save_cache(&cache_path, &bundles);
    }

    state.bundles = bundles;
    rebuild_registry(state);
    state.scanned = true;
}

fn rebuild_registry(state: &mut ScanState) {
    state.registry.clear();
    for b in &state.bundles {
        for c in &b.classes {
            if let Some(cid) = super::parse_id(&format!("{}{}", super::ID_PREFIX, c.cid_hex)) {
                state.registry.insert(cid, b.path.clone());
            }
        }
    }
}

/// Read the disk cache into the in-process registry. Never loads a bundle.
fn load_from_cache(state: &mut ScanState) {
    state.bundles = state
        .cache_path
        .as_deref()
        .map(load_cache)
        .unwrap_or_default()
        .bundles;
    rebuild_registry(state);
    state.scanned = true;
}

/// Make sure the in-process registry reflects the cache. Cheap; never
/// loads a bundle (see module docs — scanning is the subprocess's job).
pub fn ensure_scanned() {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(Default::default);
    if !state.scanned {
        load_from_cache(state);
    }
}

/// Re-read the cache (after the scan subprocess finished).
pub fn reload_from_cache() {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(Default::default);
    load_from_cache(state);
}

/// Does the cache miss bundles that exist on disk (new or changed)?
/// Cheap: directory walk + mtime compare, no loading.
pub fn cache_is_stale() -> bool {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(Default::default);
    let cached: HashMap<PathBuf, u64> = state
        .cache_path
        .as_deref()
        .map(load_cache)
        .unwrap_or_default()
        .bundles
        .into_iter()
        .map(|b| (b.path, b.binary_mtime))
        .collect();
    let mut found = Vec::new();
    for dir in default_dirs() {
        discover_bundles(&dir, &mut found);
    }
    found
        .iter()
        .any(|b| cached.get(b) != Some(&binary_mtime(b)))
}

/// Full scan with bundle loading — call this ONLY from the throwaway scan
/// subprocess. Writes the cache; the process is expected to _exit right
/// after (plugin dylib static destructors may crash at normal exit).
pub fn full_scan_blocking() {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(Default::default);
    rescan(state, &default_dirs());
}

/// Scan ONLY the given directories, loading their bundles in-process.
/// Meant for tests (register one real bundle without touching the
/// machine-wide folders) and future user-defined scan paths.
pub fn scan_dirs(dirs: &[PathBuf]) {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(Default::default);
    rescan(state, dirs);
}

/// Bundle path for a class id, from the cache-backed registry.
pub fn bundle_for_cid(cid: &[u8; 16]) -> Option<PathBuf> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(Default::default);
    if !state.scanned {
        load_from_cache(state);
    }
    state.registry.get(cid).cloned()
}

/// Is this class an audio effect (vs an instrument)?
/// Classes without subcategory info are included (old plugins).
fn is_effect(subcategories: &str) -> bool {
    if subcategories.contains("Instrument") {
        return false;
    }
    subcategories.is_empty() || subcategories.contains("Fx")
}

/// Installed VST3 effects, for the frontend picker.
pub fn list_effects() -> Vec<PluginInfo> {
    ensure_scanned();
    let guard = STATE.lock().unwrap();
    let Some(state) = guard.as_ref() else {
        return Vec::new();
    };
    let mut out: Vec<PluginInfo> = state
        .bundles
        .iter()
        .flat_map(|b| b.classes.iter())
        .filter(|c| is_effect(&c.subcategories))
        .map(|c| PluginInfo {
            id: format!("{}{}", super::ID_PREFIX, c.cid_hex),
            name: c.name.clone(),
            manufacturer: if c.vendor.is_empty() {
                "Unknown".into()
            } else {
                c.vendor.clone()
            },
            format: PluginFormat::Vst3,
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads every installed bundle in-process — run explicitly with
    /// `cargo test -p still-core -- --ignored lists_installed`.
    /// NOTE: the test process may SIGSEGV at exit (plugin dylib static
    /// destructors) AFTER all assertions passed — that very crash is why
    /// the app delegates scanning to a throwaway subprocess.
    #[test]
    #[ignore]
    fn lists_installed_vst3_effects() {
        if !Path::new("/Library/Audio/Plug-Ins/VST3").exists() {
            eprintln!("skipped: no VST3 directory on this machine");
            return;
        }
        full_scan_blocking();
        let list = list_effects();
        eprintln!("found {} VST3 effects", list.len());
        for p in list.iter().take(10) {
            eprintln!("  {} — {} ({})", p.name, p.manufacturer, p.id);
        }
        let neutron_installed = Path::new("/Library/Audio/Plug-Ins/VST3")
            .read_dir()
            .map(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().contains("Neutron 5"))
            })
            .unwrap_or(false);
        if neutron_installed {
            assert!(
                list.iter()
                    .any(|p| p.name.contains("Neutron") || p.manufacturer.contains("iZotope")),
                "Neutron 5 bundle present but not listed"
            );
        }
    }

    #[test]
    fn effect_filter() {
        assert!(is_effect("Fx|EQ"));
        assert!(is_effect("Fx"));
        assert!(is_effect(""));
        assert!(!is_effect("Instrument|Synth"));
        assert!(!is_effect("Instrument"));
    }

    #[test]
    fn discovers_and_caches_fixture_bundles() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("VST3");
        // vendor subdir with a fake bundle (never loaded — only discovery).
        let bundle = root.join("Vendor/Fake.vst3");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/MacOS/Fake"), b"not a dylib").unwrap();

        let mut found = Vec::new();
        discover_bundles(&root, &mut found);
        assert_eq!(found, vec![bundle.clone()]);
        assert!(binary_mtime(&bundle) > 0);

        // Cache roundtrip.
        let cache_path = tmp.path().join("cache.json");
        let bundles = vec![ScannedBundle {
            path: bundle,
            binary_mtime: 42,
            classes: vec![ScannedClass {
                cid_hex: "00112233445566778899aabbccddeeff".into(),
                name: "Fake EQ".into(),
                vendor: "Vendor".into(),
                subcategories: "Fx|EQ".into(),
            }],
        }];
        save_cache(&cache_path, &bundles);
        let loaded = load_cache(&cache_path);
        assert_eq!(loaded.bundles.len(), 1);
        assert_eq!(loaded.bundles[0].classes[0].name, "Fake EQ");
        // Corrupt cache degrades to empty, never fails.
        std::fs::write(&cache_path, b"{broken").unwrap();
        assert!(load_cache(&cache_path).bundles.is_empty());
    }
}
