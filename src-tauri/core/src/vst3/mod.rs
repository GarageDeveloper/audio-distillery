//! VST3 effect hosting (macOS).
//!
//! Own implementation on the `vst3` crate (coupler-rs bindings, MIT/Apache).
//! Mirrors the `aunit` module shape: everything real lives behind the macOS
//! gate; other platforms get empty/erroring stubs.
//!
//! A VST3 plugin class is identified persistently by its 16-byte class id
//! (CID). Everywhere above this module the id travels as the component
//! string `"vst3:<32 lowercase hex>"`, riding the same `component` field the
//! AU ids ("aufx:xxxx:yyyy") use — the prefix is the format discriminant.

/// Component-id prefix for VST3 plugins.
pub const ID_PREFIX: &str = "vst3:";

/// Format a 16-byte class id as a component string "vst3:<32 hex>".
pub fn cid_to_id(cid: &[u8; 16]) -> String {
    let mut s = String::with_capacity(ID_PREFIX.len() + 32);
    s.push_str(ID_PREFIX);
    for b in cid {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Parse a component string back into the 16-byte class id.
pub fn parse_id(id: &str) -> Option<[u8; 16]> {
    let hex = id.strip_prefix(ID_PREFIX)?;
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut cid = [0u8; 16];
    for (i, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        cid[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(cid)
}

#[cfg(target_os = "macos")]
mod host;
#[cfg(target_os = "macos")]
mod module;
#[cfg(target_os = "macos")]
mod plugin;
#[cfg(target_os = "macos")]
mod scan;
#[cfg(target_os = "macos")]
mod stream;

#[cfg(target_os = "macos")]
pub use plugin::{Vst3Editor, Vst3Plugin};
#[cfg(target_os = "macos")]
pub use scan::{
    cache_is_stale, ensure_scanned, full_scan_blocking, list_effects, reload_from_cache,
    scan_dirs, set_cache_path,
};

#[cfg(not(target_os = "macos"))]
pub fn list_effects() -> Vec<crate::plugins::PluginInfo> {
    Vec::new()
}
#[cfg(not(target_os = "macos"))]
pub fn set_cache_path(_path: std::path::PathBuf) {}
#[cfg(not(target_os = "macos"))]
pub fn cache_is_stale() -> bool {
    false
}
#[cfg(not(target_os = "macos"))]
pub fn full_scan_blocking() {}
#[cfg(not(target_os = "macos"))]
pub fn reload_from_cache() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip() {
        let cid: [u8; 16] = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let id = cid_to_id(&cid);
        assert_eq!(id, "vst3:0123456789abcdeffedcba9876543210");
        assert_eq!(parse_id(&id), Some(cid));
    }

    #[test]
    fn parse_rejects_malformed() {
        assert_eq!(parse_id("aufx:dcmp:appl"), None);
        assert_eq!(parse_id("vst3:"), None);
        assert_eq!(parse_id("vst3:0123"), None);
        assert_eq!(parse_id("vst3:0123456789abcdeffedcba98765432zz"), None);
        assert_eq!(parse_id("vst3:0123456789abcdeffedcba98765432100"), None);
    }
}
