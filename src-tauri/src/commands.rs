//! Thin Tauri command layer. No business logic here: every command validates
//! nothing itself — it delegates to `still-core` and returns fresh display
//! snapshots (`ProjectView`). See COMMANDS.md for the full contract.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};

use still_core::project::{ExportConfig, Project};
use still_core::{
    ExportReport, PeakSlice, PlaybackState, ProjectState, ProjectView, RegionEdge, RegionSpan,
    SilenceParams,
};

use crate::state::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: still_core::StillError) -> String {
    e.to_string()
}

fn check_extensions(paths: &[String]) -> CmdResult<()> {
    for path in paths {
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !still_core::SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(format!(
                "Unsupported file type \".{ext}\". Supported formats: WAV, FLAC, MP3, AIFF."
            ));
        }
    }
    Ok(())
}

/// Scan the session's layer groups (read-only): each group is one
/// time-synchronized layer, its files laid back-to-back. Emits
/// `load:progress` events (f32, 0..1) over the whole batch. Cancellable at
/// any time via `cancel_load`.
async fn scan_sources(
    app: &AppHandle,
    state: &State<'_, AppState>,
    groups: &[Vec<String>],
) -> CmdResult<(still_core::AudioInfo, Vec<still_core::PeakPyramid>)> {
    let path_groups: Vec<Vec<PathBuf>> = groups
        .iter()
        .map(|g| g.iter().map(PathBuf::from).collect())
        .collect();
    let app2 = app.clone();
    state.scan_cancel.store(false, Ordering::SeqCst);
    let cancel = state.scan_cancel.clone();
    let scanned = tauri::async_runtime::spawn_blocking(move || {
        let mut last = Instant::now() - Duration::from_secs(1);
        still_core::scan_layers(&path_groups, &cancel, |p| {
            if last.elapsed() >= Duration::from_millis(80) {
                last = Instant::now();
                let _ = app2.emit("load:progress", p);
            }
        })
    })
    .await
    .map_err(|e| format!("Analysis task failed: {e}"))?
    .map_err(err)?;
    let _ = app.emit("load:progress", 1.0f32);
    Ok(scanned)
}

/// Abort the analysis currently running in `load_audio` / `add_clips` /
/// `load_project`. The pending command then fails with "Import cancelled"
/// and the previous session (if any) stays untouched.
#[tauri::command]
pub fn cancel_load(state: State<'_, AppState>) -> CmdResult<()> {
    state.scan_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Per-layer playlists + current linear volumes for the playback thread.
fn playlists_of(info: &still_core::AudioInfo, project: &Project) -> Vec<still_core::LayerPlay> {
    let sr = info.sample_rate.max(1) as f64;
    info.layers
        .iter()
        .enumerate()
        .map(|(i, scanned)| {
            let volume = project
                .layers
                .get(i)
                .map(|l| {
                    if l.muted {
                        0.0
                    } else {
                        still_core::db_to_linear(l.gain_db)
                    }
                })
                .unwrap_or(1.0);
            still_core::LayerPlay {
                playlist: scanned
                    .clips
                    .iter()
                    .map(|c| (PathBuf::from(&c.path), c.duration_samples as f64 / sr))
                    .collect(),
                volume,
            }
        })
        .collect()
}

/// Scan layer groups and install them as the current session.
async fn load_session(
    app: AppHandle,
    state: State<'_, AppState>,
    groups: Vec<Vec<String>>,
    project: Option<(Project, PathBuf)>,
) -> CmdResult<ProjectView> {
    let (info, peaks) = scan_sources(&app, &state, &groups).await?;

    let mut ps = match project {
        Some((project, project_path)) => {
            let mut ps = ProjectState::new(project, info, peaks);
            ps.project_path = Some(project_path);
            ps
        }
        None => ProjectState::new(Project::new_layers(groups), info, peaks),
    };
    state
        .player
        .load(playlists_of(&ps.info, &ps.project), ps.info.duration_seconds)
        .map_err(err)?;
    // Clamp regions against the real scanned duration (source may have
    // changed since the project was saved) and drop degenerate ones.
    still_core::sanitize_regions(&mut ps.project, ps.info.duration_samples, ps.info.sample_rate);
    let view = ps.view();
    *state.session.lock().unwrap() = Some(ps);
    Ok(view)
}

/// Open one or more audio files as a NEW session (sequential clips of the
/// base layer, in the given order).
#[tauri::command]
pub async fn load_audio(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<ProjectView> {
    if paths.is_empty() {
        return Err("No audio file given.".to_string());
    }
    check_extensions(&paths)?;
    load_session(app, state, vec![paths], None).await
}

/// Open several files as a NEW multitrack session: each file becomes one
/// time-synchronized layer (all starting at t = 0), mixed together.
#[tauri::command]
pub async fn load_multitrack(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<ProjectView> {
    if paths.is_empty() {
        return Err("No audio file given.".to_string());
    }
    check_extensions(&paths)?;
    let groups = paths.into_iter().map(|p| vec![p]).collect();
    load_session(app, state, groups, None).await
}

/// Rescan with new groups while keeping the current project recipe.
async fn rescan_with_groups(
    app: AppHandle,
    state: State<'_, AppState>,
    groups: Vec<Vec<String>>,
    update: impl FnOnce(&mut ProjectState),
) -> CmdResult<ProjectView> {
    let (info, peaks) = scan_sources(&app, &state, &groups).await?;
    with_session(&state, |s| {
        update(s);
        s.set_audio(info, peaks);
        still_core::sanitize_regions(&mut s.project, s.info.duration_samples, s.info.sample_rate);
        state
            .player
            .load(playlists_of(&s.info, &s.project), s.info.duration_seconds)
            .map_err(err)?;
        Ok(s.view())
    })
}

/// Append clips to the END of the base layer's timeline. Existing regions,
/// titles and undo history are preserved.
#[tauri::command]
pub async fn add_clips(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<ProjectView> {
    if paths.is_empty() {
        return Err("No audio file given.".to_string());
    }
    check_extensions(&paths)?;
    let groups = with_session(&state, |s| {
        let mut groups = s.project.source_groups();
        groups[0].extend(paths.iter().cloned());
        Ok(groups)
    })?;
    rescan_with_groups(app, state, groups.clone(), move |s| {
        s.project.layers[0].sources = groups[0].clone();
    })
    .await
}

/// Add each given file as a new synced LAYER of the existing session.
#[tauri::command]
pub async fn add_layers(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<ProjectView> {
    if paths.is_empty() {
        return Err("No audio file given.".to_string());
    }
    check_extensions(&paths)?;
    let (groups, new_layers) = with_session(&state, |s| {
        let mut groups = s.project.source_groups();
        let mut new_layers = Vec::new();
        for p in &paths {
            groups.push(vec![p.clone()]);
            let id = s.project.next_layer_id;
            s.project.next_layer_id += 1;
            new_layers.push(still_core::Layer {
                id,
                sources: vec![p.clone()],
                gain_db: 0.0,
                muted: false,
            });
        }
        Ok((groups, new_layers))
    })?;
    rescan_with_groups(app, state, groups, move |s| {
        s.project.layers.extend(new_layers);
    })
    .await
}

#[tauri::command]
pub fn set_layer_gain(
    state: State<'_, AppState>,
    id: u32,
    gain_db: f32,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_layer_gain(id, gain_db).map_err(err)?;
        sync_player_volumes(&state, s);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn set_layer_muted(
    state: State<'_, AppState>,
    id: u32,
    muted: bool,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_layer_muted(id, muted).map_err(err)?;
        sync_player_volumes(&state, s);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn remove_layer(state: State<'_, AppState>, id: u32) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.remove_layer(id).map_err(err)?;
        let _ = state
            .player
            .load(playlists_of(&s.info, &s.project), s.info.duration_seconds);
        Ok(s.view())
    })
}

/// Push the current gains/mutes to the running playback sinks (live).
fn sync_player_volumes(state: &State<'_, AppState>, s: &ProjectState) {
    for (i, l) in s.project.layers.iter().enumerate() {
        let vol = if l.muted {
            0.0
        } else {
            still_core::db_to_linear(l.gain_db)
        };
        let _ = state.player.set_volume(i, vol);
    }
}

#[tauri::command]
pub fn get_project_view(state: State<'_, AppState>) -> CmdResult<ProjectView> {
    with_session(&state, |s| Ok(s.view()))
}

#[tauri::command]
pub fn get_peaks(
    state: State<'_, AppState>,
    start_sample: u64,
    end_sample: u64,
    max_buckets: u32,
) -> CmdResult<PeakSlice> {
    with_session(&state, |s| {
        Ok(s.peaks_slice(start_sample, end_sample, max_buckets))
    })
}

fn with_session<T>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&mut ProjectState) -> CmdResult<T>,
) -> CmdResult<T> {
    let mut guard = state.session.lock().unwrap();
    let s = guard
        .as_mut()
        .ok_or_else(|| err(still_core::StillError::NoAudioLoaded))?;
    f(s)
}

/// Apply optional zero-crossing snap to an edge position (backend decides,
/// SPEC §3: the frontend never adjusts positions itself).
fn snapped(s: &ProjectState, position: u64) -> u64 {
    if s.project.snap_to_zero {
        still_core::snap_to_zero_crossing(&s.info.clips, position, 50)
    } else {
        position
    }
}

/// Create a track region from a start/end pair (any order accepted), with an
/// optional title given at creation time.
#[tauri::command]
pub fn add_region(
    state: State<'_, AppState>,
    start: u64,
    end: u64,
    title: Option<String>,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let a = snapped(s, start);
        let b = snapped(s, end);
        s.add_region(a, b, title).map_err(err)?;
        Ok(s.view())
    })
}

#[tauri::command]
pub fn add_regions(
    state: State<'_, AppState>,
    regions: Vec<RegionSpan>,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.add_regions(&regions);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn move_region_edge(
    state: State<'_, AppState>,
    id: u32,
    edge: RegionEdge,
    position: u64,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let pos = snapped(s, position);
        s.move_edge(id, edge, pos).map_err(err)?;
        Ok(s.view())
    })
}

#[tauri::command]
pub fn remove_region(state: State<'_, AppState>, id: u32) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.remove_region(id).map_err(err)?;
        Ok(s.view())
    })
}

#[tauri::command]
pub fn rename_track(state: State<'_, AppState>, id: u32, title: String) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.rename_track(id, &title).map_err(err)?;
        Ok(s.view())
    })
}

#[tauri::command]
pub fn set_snap_to_zero(state: State<'_, AppState>, enabled: bool) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.project.snap_to_zero = enabled;
        Ok(s.view())
    })
}

#[tauri::command]
pub fn set_export_config(
    state: State<'_, AppState>,
    config: ExportConfig,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.project.export_config = config;
        Ok(s.view())
    })
}

#[tauri::command]
pub fn undo(state: State<'_, AppState>) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.undo();
        Ok(s.view())
    })
}

#[tauri::command]
pub fn redo(state: State<'_, AppState>) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.redo();
        Ok(s.view())
    })
}

#[tauri::command]
pub fn detect_silences(
    state: State<'_, AppState>,
    params: SilenceParams,
) -> CmdResult<Vec<RegionSpan>> {
    with_session(&state, |s| {
        // Detection runs on the same mix the user sees/hears.
        let merged = s.merged_pyramid();
        Ok(still_core::detect_track_regions(
            &merged,
            s.info.sample_rate,
            s.info.duration_samples,
            &params,
        ))
    })
}

#[tauri::command]
pub async fn export_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
    config: ExportConfig,
) -> CmdResult<ExportReport> {
    if state.export_running.swap(true, Ordering::SeqCst) {
        return Err(err(still_core::StillError::ExportAlreadyRunning));
    }
    state.export_cancel.store(false, Ordering::SeqCst);
    // Playback is pointless during an export — stop it.
    let _ = state.player.pause();

    let prepared = with_session(&state, |s| {
        let tracks = s.tracks();
        if tracks.is_empty() {
            return Err(
                "No tracks defined. Mark at least one region (start + end) before exporting."
                    .to_string(),
            );
        }
        // The destination must never silently be the source folder (§3 bis):
        // the UI requires an explicit choice; here we only forbid emptiness.
        s.project.export_config = config.clone();
        let source = PathBuf::from(&s.info.path);
        let jobs = still_core::plan_export(&tracks, &config, &source).map_err(err)?;
        let layers: Vec<still_core::LayerMix> = s
            .project
            .layers
            .iter()
            .zip(s.info.layers.iter())
            .map(|(l, scanned)| still_core::LayerMix {
                clips: scanned.clips.clone(),
                gain_db: l.gain_db,
                muted: l.muted,
            })
            .collect();
        Ok((layers, s.info.channels, s.info.sample_rate, jobs))
    });
    let (layers, session_channels, sample_rate, jobs) = match prepared {
        Ok(x) => x,
        Err(e) => {
            state.export_running.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };

    let cancel = state.export_cancel.clone();
    let app2 = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let ffmpeg = still_core::resolve_ffmpeg(&[])?;
        // Progress arrives from several worker threads at once; throttle the
        // stream globally but always let start/end events through.
        let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
        Ok::<ExportReport, still_core::StillError>(still_core::run_export(
            &ffmpeg,
            &layers,
            session_channels,
            sample_rate,
            &jobs,
            &config,
            &cancel,
            |p| {
                let force = p.track_progress == 0.0 || p.track_progress == 1.0;
                let mut last = last.lock().unwrap();
                if force || last.elapsed() >= Duration::from_millis(60) {
                    *last = Instant::now();
                    let _ = app2.emit("export:progress", &p);
                }
            },
        ))
    })
    .await
    .map_err(|e| format!("Export task failed: {e}"));

    state.export_running.store(false, Ordering::SeqCst);
    result?.map_err(err)
}

#[tauri::command]
pub fn cancel_export(state: State<'_, AppState>) -> CmdResult<()> {
    state.export_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn save_project(state: State<'_, AppState>, path: Option<String>) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let target = match path.map(PathBuf::from).or_else(|| s.project_path.clone()) {
            Some(p) => p,
            None => return Err("No project file path given.".to_string()),
        };
        still_core::save_project(&s.project, &target).map_err(err)?;
        s.project_path = Some(target);
        Ok(s.view())
    })
}

#[tauri::command]
pub async fn load_project(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> CmdResult<ProjectView> {
    let project_path = PathBuf::from(&path);
    let project = still_core::read_project(&project_path).map_err(err)?;
    for layer in &project.layers {
        for source in &layer.sources {
            if !Path::new(source).is_file() {
                return Err(format!(
                    "A source audio file referenced by this project was not found: {source}"
                ));
            }
        }
    }
    let groups = project.source_groups();
    load_session(app, state, groups, Some((project, project_path))).await
}

// ---- Playback ----------------------------------------------------------

#[tauri::command]
pub fn player_toggle(state: State<'_, AppState>) -> CmdResult<PlaybackState> {
    let st = state.player.state();
    if st.playing {
        state.player.pause().map_err(err)?;
    } else {
        state.player.play().map_err(err)?;
    }
    Ok(state.player.state())
}

#[tauri::command]
pub fn player_pause(state: State<'_, AppState>) -> CmdResult<PlaybackState> {
    state.player.pause().map_err(err)?;
    Ok(state.player.state())
}

#[tauri::command]
pub fn player_seek(state: State<'_, AppState>, position_samples: u64) -> CmdResult<PlaybackState> {
    let secs = with_session(&state, |s| {
        Ok(position_samples as f64 / s.info.sample_rate as f64)
    })?;
    state.player.seek(secs).map_err(err)?;
    Ok(state.player.state())
}

#[tauri::command]
pub fn player_state(state: State<'_, AppState>) -> CmdResult<PlaybackState> {
    Ok(state.player.state())
}

#[tauri::command]
pub fn get_default_export_dir() -> CmdResult<String> {
    let base = dirs_next_audio_dir();
    Ok(base.display().to_string())
}

/// ~/Music/AudioDistillery (or home fallback) — never the source folder.
fn dirs_next_audio_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Music").join("AudioDistillery")
}
