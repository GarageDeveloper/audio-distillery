mod chain_host;
mod commands;
mod editor;
mod state;

use state::AppState;

/// Hidden mode: `AudioDistillery --vst3-scan <cache.json> [extra dirs…]`
/// runs the VST3 scan (which loads every plugin dylib) and exits WITHOUT
/// running static destructors — plugin dylibs are known to SIGSEGV at
/// normal exit. The app spawns this on itself so its own process never
/// loads unused plugins.
fn maybe_run_vst3_scan() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--vst3-scan") {
        return;
    }
    if let Some(cache) = args.next() {
        still_core::vst3::set_cache_path(cache.into());
        still_core::vst3::set_extra_dirs(args.map(Into::into).collect());
        #[cfg(target_os = "macos")]
        still_core::vst3::full_scan_blocking();
    }
    unsafe { libc::_exit(0) }
}

/// Persisted list of user-configured extra VST3 scan directories.
pub(crate) fn scan_paths_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("vst3_scan_paths.json"))
}

pub(crate) fn load_scan_paths(app: &tauri::AppHandle) -> Vec<String> {
    scan_paths_file(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Run the scan subprocess (which _exits before plugin static destructors
/// can crash) and reload the in-process registry from the refreshed cache.
pub(crate) fn run_scan_subprocess(cache: &std::path::Path, extra: &[String]) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::process::Command::new(exe)
        .arg("--vst3-scan")
        .arg(cache)
        .args(extra)
        .status();
    still_core::vst3::reload_from_cache();
}

/// Refresh the VST3 scan cache in a throwaway subprocess when bundles
/// changed on disk. Runs in the background at startup; the picker simply
/// shows the current cache until the refresh lands.
fn spawn_vst3_rescan_if_stale(app: &tauri::AppHandle, cache: std::path::PathBuf) {
    still_core::vst3::set_cache_path(cache.clone());
    let extra = load_scan_paths(app);
    still_core::vst3::set_extra_dirs(extra.iter().map(Into::into).collect());
    #[cfg(target_os = "macos")]
    std::thread::spawn(move || {
        if !still_core::vst3::cache_is_stale() {
            return;
        }
        run_scan_subprocess(&cache, &extra);
    });
    #[cfg(not(target_os = "macos"))]
    let _ = extra;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    maybe_run_vst3_scan();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::load_audio,
            commands::load_multitrack,
            commands::add_clips,
            commands::add_take,
            commands::add_layers,
            commands::set_layer_gain,
            commands::set_layer_muted,
            commands::set_layer_solo,
            commands::set_layer_collapsed,
            commands::set_track_layer_gain,
            commands::set_track_layer_mute,
            commands::set_track_layer_solo,
            commands::remove_layer,
            commands::cancel_load,
            commands::load_project,
            commands::save_project,
            commands::get_project_view,
            commands::get_peaks,
            commands::get_peaks_split,
            commands::add_region,
            commands::add_regions,
            commands::begin_region_edit,
            commands::move_region_edge_preview,
            commands::move_region_edge,
            commands::remove_region,
            commands::rename_track,
            commands::rename_layer,
            commands::set_snap_to_zero,
            commands::set_export_config,
            commands::set_album_meta,
            commands::get_artwork_preview,
            commands::list_plugins,
            commands::get_vst3_scan_paths,
            commands::set_vst3_scan_paths,
            commands::add_chain_plugin,
            commands::remove_chain_plugin,
            commands::move_chain_plugin,
            commands::set_chain_bypass,
            commands::open_plugin_editor,
            commands::reload_chains,
            commands::chain_latency,
            commands::meter_state,
            commands::reset_meter,
            commands::save_chain_preset,
            commands::list_chain_presets,
            commands::load_chain_preset,
            commands::delete_chain_preset,
            commands::undo,
            commands::redo,
            commands::detect_silences,
            commands::export_tracks,
            commands::cancel_export,
            commands::player_toggle,
            commands::player_pause,
            commands::player_seek,
            commands::player_state,
            commands::get_default_export_dir,
        ])
        .setup(|app| {
            use tauri::Manager;
            if let Ok(dir) = app.path().app_config_dir() {
                spawn_vst3_rescan_if_stale(app.app_handle(), dir.join("vst3_scan_cache.json"));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AudioDistillery");
}
