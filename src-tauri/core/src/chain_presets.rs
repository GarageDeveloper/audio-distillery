//! Named mastering-chain presets.
//!
//! A preset is the declarative recipe of a chain — component ids, display
//! names, bypass flags and plugin states — with no project-specific plugin
//! ids. Presets live outside any project, one JSON file per preset in a
//! directory owned by the app (the tauri layer passes it in; this module
//! stays FS-path agnostic and Tauri-free).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{Result, StillError};
use crate::naming::sanitize_filename;
use crate::project::MasteringPluginCfg;

/// One plugin of a saved chain (a `MasteringPluginCfg` without the
/// project-local id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainPresetPlugin {
    /// AU component id "aufx:xxxx:yyyy".
    pub component: String,
    pub name: String,
    pub bypass: bool,
    /// Base64 binary plist of the plugin state (ClassInfo).
    #[serde(default)]
    pub state_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainPreset {
    pub name: String,
    pub plugins: Vec<ChainPresetPlugin>,
}

/// Listing entry sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct ChainPresetInfo {
    pub name: String,
    /// Plugin display names, in chain order.
    pub plugins: Vec<String>,
}

impl ChainPreset {
    pub fn from_chain(name: &str, chain: &[MasteringPluginCfg]) -> Self {
        Self {
            name: name.to_string(),
            plugins: chain
                .iter()
                .map(|c| ChainPresetPlugin {
                    component: c.component.clone(),
                    name: c.name.clone(),
                    bypass: c.bypass,
                    state_b64: c.state_b64.clone(),
                })
                .collect(),
        }
    }
}

fn preset_path(dir: &Path, name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        return Err(StillError::InvalidProject(
            "Preset name cannot be empty".into(),
        ));
    }
    let stem = sanitize_filename(name.trim());
    Ok(dir.join(format!("{stem}.json")))
}

/// Save (or overwrite, same name) a preset. Returns the refreshed listing.
pub fn save_preset(dir: &Path, preset: &ChainPreset) -> Result<Vec<ChainPresetInfo>> {
    if preset.plugins.is_empty() {
        return Err(StillError::InvalidProject(
            "The mastering chain is empty — add a plugin before saving a preset".into(),
        ));
    }
    fs::create_dir_all(dir)?;
    let path = preset_path(dir, &preset.name)?;
    let json = serde_json::to_string_pretty(preset)
        .map_err(|e| StillError::InvalidProject(e.to_string()))?;
    fs::write(&path, json)?;
    list_presets(dir)
}

/// All presets in `dir`, sorted by name (missing dir = empty list).
pub fn list_presets(dir: &Path) -> Result<Vec<ChainPresetInfo>> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(preset) = load_preset_file(&path) {
            out.push(ChainPresetInfo {
                name: preset.name,
                plugins: preset.plugins.into_iter().map(|p| p.name).collect(),
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

pub fn load_preset(dir: &Path, name: &str) -> Result<ChainPreset> {
    load_preset_file(&preset_path(dir, name)?)
}

fn load_preset_file(path: &Path) -> Result<ChainPreset> {
    let json = fs::read_to_string(path)
        .map_err(|_| StillError::FileNotFound(path.display().to_string()))?;
    serde_json::from_str(&json).map_err(|e| {
        StillError::InvalidProject(format!("{}: {e}", path.display()))
    })
}

/// Delete a preset. Returns the refreshed listing.
pub fn delete_preset(dir: &Path, name: &str) -> Result<Vec<ChainPresetInfo>> {
    let path = preset_path(dir, name)?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    list_presets(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str) -> MasteringPluginCfg {
        MasteringPluginCfg {
            id: 7,
            component: "aufx:test:demo".into(),
            name: name.into(),
            bypass: false,
            state_b64: Some("AAEC".into()),
        }
    }

    #[test]
    fn preset_roundtrip_list_and_delete() {
        let dir = std::env::temp_dir().join(format!(
            "still-presets-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        assert!(list_presets(&dir).unwrap().is_empty());

        let preset = ChainPreset::from_chain("Warm Master", &[cfg("EQ"), cfg("Comp")]);
        let listed = save_preset(&dir, &preset).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Warm Master");
        assert_eq!(listed[0].plugins, vec!["EQ", "Comp"]);

        let loaded = load_preset(&dir, "Warm Master").unwrap();
        assert_eq!(loaded.plugins.len(), 2);
        assert_eq!(loaded.plugins[0].state_b64.as_deref(), Some("AAEC"));

        // Same name overwrites instead of duplicating.
        let smaller = ChainPreset::from_chain("Warm Master", &[cfg("EQ")]);
        let listed = save_preset(&dir, &smaller).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].plugins, vec!["EQ"]);

        assert!(delete_preset(&dir, "Warm Master").unwrap().is_empty());

        // Empty chain and empty name are rejected.
        assert!(save_preset(&dir, &ChainPreset::from_chain("X", &[])).is_err());
        assert!(save_preset(&dir, &ChainPreset::from_chain("  ", &[cfg("EQ")])).is_err());

        let _ = fs::remove_dir_all(&dir);
    }
}
