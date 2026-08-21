mod chain_host;
mod commands;
mod editor;
mod state;

use state::AppState;

/// Hidden mode: `AudioDistillery --vst3-scan <cache.json>` runs the VST3
/// scan (which loads every plugin dylib) and exits WITHOUT running static
/// destructors — plugin dylibs are known to SIGSEGV at normal exit. The app
/// spawns this on itself so its own process never loads unused plugins.
fn maybe_run_vst3_scan() {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("--vst3-scan") {
        return;
    }
    if let Some(cache) = args.next() {
        still_core::vst3::set_cache_path(cache.into());
        #[cfg(target_os = "macos")]
        still_core::vst3::full_scan_blocking();
    }
    unsafe { libc::_exit(0) }
}

/// Refresh the VST3 scan cache in a throwaway subprocess when bundles
/// changed on disk. Runs in the background at startup; the picker simply
/// shows the current cache until the refresh lands.
fn spawn_vst3_rescan_if_stale(cache: std::path::PathBuf) {
    still_core::vst3::set_cache_path(cache.clone());
    #[cfg(target_os = "macos")]
    std::thread::spawn(move || {
        if !still_core::vst3::cache_is_stale() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let _ = std::process::Command::new(exe)
            .arg("--vst3-scan")
            .arg(&cache)
            .status();
        still_core::vst3::reload_from_cache();
    });
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
            commands::set_snap_to_zero,
            commands::set_export_config,
            commands::set_album_meta,
            commands::get_artwork_preview,
            commands::list_plugins,
            commands::add_mastering_plugin,
            commands::remove_mastering_plugin,
            commands::move_mastering_plugin,
            commands::set_mastering_bypass,
            commands::open_plugin_editor,
            commands::reload_mastering_chain,
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
                spawn_vst3_rescan_if_stale(dir.join("vst3_scan_cache.json"));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running AudioDistillery");
}
