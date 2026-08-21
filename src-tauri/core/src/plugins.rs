//! Format-agnostic plugin surface: one listing and one factory for every
//! hosted plugin format (Audio Units, VST3). Everything above this module —
//! chain host, engine, export, frontend — deals in component-id strings and
//! `Box<dyn BlockProcessor>`; the format only matters here, discriminated by
//! the id prefix ("aufx:..." = AU, "vst3:..." = VST3).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::engine::render::BlockProcessor;
#[cfg(not(target_os = "macos"))]
use crate::error::StillError;
use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub enum PluginFormat {
    #[serde(rename = "au")]
    Au,
    #[serde(rename = "vst3")]
    Vst3,
}

/// One installed plugin, as listed to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct PluginInfo {
    /// Component id: "aufx:xxxx:yyyy" (AU) or "vst3:<32 hex>" (VST3).
    pub id: String,
    pub name: String,
    pub manufacturer: String,
    pub format: PluginFormat,
}

/// The format a component id belongs to, by prefix.
pub fn format_of(component: &str) -> PluginFormat {
    if component.starts_with(crate::vst3::ID_PREFIX) {
        PluginFormat::Vst3
    } else {
        PluginFormat::Au
    }
}

/// Every installed effect plugin, all formats, AU first then VST3.
pub fn list_plugins() -> Vec<PluginInfo> {
    let mut out = crate::aunit::list_effects();
    out.extend(crate::vst3::list_effects());
    out
}

/// Instantiate a plugin of any supported format from its component id.
/// Runs on the CALLER's thread — lifecycle threading rules are the caller's
/// responsibility (main thread for the live chain, worker thread for export).
pub fn create_plugin(
    component: &str,
    sample_rate: u32,
    channels: usize,
    playing: Arc<AtomicBool>,
) -> Result<Box<dyn BlockProcessor>> {
    match format_of(component) {
        #[cfg(target_os = "macos")]
        PluginFormat::Vst3 => Ok(Box::new(crate::vst3::Vst3Plugin::new(
            component,
            sample_rate,
            channels,
            playing,
        )?)),
        #[cfg(target_os = "macos")]
        PluginFormat::Au => Ok(Box::new(crate::aunit::AuPlugin::new(
            component,
            sample_rate,
            channels,
            playing,
        )?)),
        #[cfg(not(target_os = "macos"))]
        _ => Err(StillError::Playback(
            "plugin hosting is only available on macOS".into(),
        )),
    }
}
