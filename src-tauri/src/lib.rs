mod commands;
mod state;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::load_audio,
            commands::load_multitrack,
            commands::add_clips,
            commands::add_layers,
            commands::set_layer_gain,
            commands::set_layer_muted,
            commands::set_track_layer_gain,
            commands::remove_layer,
            commands::cancel_load,
            commands::load_project,
            commands::save_project,
            commands::get_project_view,
            commands::get_peaks,
            commands::get_peaks_split,
            commands::add_region,
            commands::add_regions,
            commands::move_region_edge,
            commands::remove_region,
            commands::rename_track,
            commands::set_snap_to_zero,
            commands::set_export_config,
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
        .run(tauri::generate_context!())
        .expect("error while running AudioDistillery");
}
