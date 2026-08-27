//! Thin Tauri command layer. No business logic here: every command validates
//! nothing itself — it delegates to `still-core` and returns fresh display
//! snapshots (`ProjectView`). See COMMANDS.md for the full contract.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};

use still_core::engine::render::BlockProcessor;
use still_core::project::{ExportConfig, Project};
use still_core::{
    AlbumMeta, ExportReport, PeakSlice, PlaybackState, PluginInfo, ProjectState,
    ProjectView, RegionEdge, RegionSpan, SilenceParams,
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
    groups: &[Vec<still_core::SourceRef>],
) -> CmdResult<(still_core::AudioInfo, Vec<still_core::PeakPyramid>)> {
    let path_groups: Vec<Vec<(PathBuf, Option<u64>)>> = groups
        .iter()
        .map(|g| g.iter().map(|r| (PathBuf::from(&r.path), r.start)).collect())
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

/// Per-layer playlists for the playback thread, with silent gaps between
/// take-aligned clips so every layer keeps the same clock.
fn playlists_of(info: &still_core::AudioInfo) -> Vec<still_core::LayerPlay> {
    let sr = info.sample_rate.max(1) as f64;
    info.layers
        .iter()
        .map(|scanned| {
            let mut playlist = Vec::new();
            let mut cursor = 0u64;
            for c in &scanned.clips {
                if c.start_sample > cursor {
                    playlist.push((None, (c.start_sample - cursor) as f64 / sr));
                }
                playlist.push((
                    Some(PathBuf::from(&c.path)),
                    c.duration_samples as f64 / sr,
                ));
                cursor = c.start_sample + c.duration_samples;
            }
            still_core::LayerPlay { playlist }
        })
        .collect()
}

/// The timeline volume automation matching the current project state:
/// session defaults + per-track override spans, all resolved by the core.
fn automation_of(s: &ProjectState) -> still_core::VolumeAutomation {
    let sr = s.info.sample_rate.max(1) as f64;
    still_core::VolumeAutomation {
        default: s.effective_volumes(None),
        spans: s
            .volume_spans()
            .into_iter()
            .map(|(start, end, vols)| (start as f64 / sr, end as f64 / sr, vols))
            .collect(),
    }
}

/// Push the current mix (faders, mutes, solos, per-track overrides) to the
/// playback thread — it applies immediately and follows the playhead.
fn sync_playback(state: &State<'_, AppState>, s: &ProjectState) {
    let _ = state.player.set_automation(automation_of(s));
}

/// Scan layer groups and install them as the current session.
async fn load_session(
    app: AppHandle,
    state: State<'_, AppState>,
    groups: Vec<Vec<String>>,
    project: Option<(Project, PathBuf)>,
) -> CmdResult<ProjectView> {
    let refs: Vec<Vec<still_core::SourceRef>> = groups
        .iter()
        .map(|g| g.iter().cloned().map(still_core::SourceRef::sequential).collect())
        .collect();
    let (info, peaks) = scan_sources(&app, &state, &refs).await?;

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
        .load_session(
            playlists_of(&ps.info),
            ps.info.duration_seconds,
            automation_of(&ps),
            ps.info.sample_rate,
            ps.info.channels.max(1) as usize,
        )
        .map_err(err)?;
    // Clamp regions against the real scanned duration (source may have
    // changed since the project was saved) and drop degenerate ones.
    still_core::sanitize_regions(&mut ps.project, ps.info.duration_samples, ps.info.sample_rate);
    let view = ps.view();
    *state.session.lock().unwrap() = Some(ps);
    // Re-instantiate the project's mastering chain at the session format
    // (errors surface but don't block the load).
    let _ = with_session(&state, |s| {
        let _ = rebuild_chain(&app, &state, s, true);
        Ok(())
    });
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
    groups: Vec<Vec<still_core::SourceRef>>,
    update: impl FnOnce(&mut ProjectState),
) -> CmdResult<ProjectView> {
    let (info, peaks) = scan_sources(&app, &state, &groups).await?;
    with_session(&state, |s| {
        update(s);
        s.set_audio(info, peaks);
        still_core::sanitize_regions(&mut s.project, s.info.duration_samples, s.info.sample_rate);
        state
            .player
            .load_session(
                playlists_of(&s.info),
                s.info.duration_seconds,
                automation_of(s),
                s.info.sample_rate,
                s.info.channels.max(1) as usize,
            )
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
        groups[0].extend(
            paths
                .iter()
                .cloned()
                .map(still_core::SourceRef::sequential),
        );
        Ok(groups)
    })?;
    rescan_with_groups(app, state, groups.clone(), move |s| {
        s.project.layers[0].sources = groups[0].clone();
    })
    .await
}

/// Append a whole synchronized TAKE: one file per existing layer (matched in
/// order), all starting together right after the current timeline end, with
/// silent gaps filling any length difference between layers.
#[tauri::command]
pub async fn add_take(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> CmdResult<ProjectView> {
    if paths.is_empty() {
        return Err("No audio file given.".to_string());
    }
    check_extensions(&paths)?;
    // Sort by name so recorder-style suffixes (Tr1, Tr2, …) line up with the
    // layer order across takes.
    let mut paths = paths;
    paths.sort();
    let (groups, take_start) = with_session(&state, |s| {
        if paths.len() != s.project.layers.len() {
            return Err(format!(
                "This session has {} layers but {} file(s) were given. A take needs exactly one file per layer (in order).",
                s.project.layers.len(),
                paths.len()
            ));
        }
        let take_start = s.info.duration_samples;
        let mut groups = s.project.source_groups();
        for (i, p) in paths.iter().enumerate() {
            groups[i].push(still_core::SourceRef {
                path: p.clone(),
                start: Some(take_start),
            });
        }
        Ok((groups, take_start))
    })?;
    let _ = take_start;
    rescan_with_groups(app, state, groups.clone(), move |s| {
        for (i, g) in groups.iter().enumerate() {
            s.project.layers[i].sources = g.clone();
        }
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
            groups.push(vec![still_core::SourceRef::sequential(p.clone())]);
            let id = s.project.next_layer_id;
            s.project.next_layer_id += 1;
            new_layers.push(still_core::Layer {
                id,
                sources: vec![still_core::SourceRef::sequential(p.clone())],
                gain_db: 0.0,
                muted: false,
                solo: false,
                collapsed: false,
                inserts: Vec::new(),
                custom_name: None,
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
        sync_playback(&state, s);
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
        sync_playback(&state, s);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn set_layer_solo(
    state: State<'_, AppState>,
    id: u32,
    solo: bool,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_layer_solo(id, solo).map_err(err)?;
        sync_playback(&state, s);
        Ok(s.view())
    })
}

/// Set or clear (null) a per-track mute override for one layer.
#[tauri::command]
pub fn set_track_layer_mute(
    state: State<'_, AppState>,
    track_id: u32,
    layer_id: u32,
    muted: Option<bool>,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_track_layer_flag(track_id, layer_id, false, muted)
            .map_err(err)?;
        sync_playback(&state, s);
        Ok(s.view())
    })
}

/// Set or clear (null) a per-track solo override for one layer.
#[tauri::command]
pub fn set_track_layer_solo(
    state: State<'_, AppState>,
    track_id: u32,
    layer_id: u32,
    solo: Option<bool>,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_track_layer_flag(track_id, layer_id, true, solo)
            .map_err(err)?;
        sync_playback(&state, s);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn set_layer_collapsed(
    state: State<'_, AppState>,
    id: u32,
    collapsed: bool,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_layer_collapsed(id, collapsed).map_err(err)?;
        Ok(s.view())
    })
}

/// Set or clear (null) a per-track gain override for one layer. Overrides
/// apply at export time on top of the session-wide layer gains.
#[tauri::command]
pub fn set_track_layer_gain(
    state: State<'_, AppState>,
    track_id: u32,
    layer_id: u32,
    gain_db: Option<f32>,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_track_layer_gain(track_id, layer_id, gain_db)
            .map_err(err)?;
        sync_playback(&state, s);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn remove_layer(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u32,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let before: Vec<u32> = s.all_chain_cfgs().map(|c| c.id).collect();
        s.remove_layer(id).map_err(err)?;
        let _ = state.player.load_session(
            playlists_of(&s.info),
            s.info.duration_seconds,
            automation_of(s),
            s.info.sample_rate,
            s.info.channels.max(1) as usize,
        );
        resync_chains_after_edit(&app, &state, s, &before);
        Ok(s.view())
    })
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

/// Per-layer display peaks (same window/grid), for the "layers" waveform
/// view. Each slice is already scaled by that layer's effective gain.
#[tauri::command]
pub fn get_peaks_split(
    state: State<'_, AppState>,
    start_sample: u64,
    end_sample: u64,
    max_buckets: u32,
) -> CmdResult<Vec<PeakSlice>> {
    with_session(&state, |s| {
        Ok(s.layer_slices(start_sample, end_sample, max_buckets))
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
/// ARCHITECTURE.md §3: the frontend never adjusts positions itself).
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
        sync_playback(&state, s);
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
        sync_playback(&state, s);
        Ok(s.view())
    })
}

/// Take ONE undo snapshot at the start of an interactive edge drag; the
/// following `move_region_edge_preview` calls then update live without
/// polluting the undo history.
#[tauri::command]
pub fn begin_region_edit(state: State<'_, AppState>) -> CmdResult<()> {
    with_session(&state, |s| {
        s.begin_edit();
        Ok(())
    })
}

/// Live, undo-free edge move used while dragging (durations in the track
/// panel follow in real time). The backend still clamps and snaps.
#[tauri::command]
pub fn move_region_edge_preview(
    state: State<'_, AppState>,
    id: u32,
    edge: RegionEdge,
    position: u64,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let pos = snapped(s, position);
        s.move_edge_preview(id, edge, pos).map_err(err)?;
        sync_playback(&state, s);
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
        sync_playback(&state, s);
        let _ = sync_engine_inserts(&state, s);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn remove_region(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u32,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let before: Vec<u32> = s.all_chain_cfgs().map(|c| c.id).collect();
        s.remove_region(id).map_err(err)?;
        sync_playback(&state, s);
        resync_chains_after_edit(&app, &state, s, &before);
        Ok(s.view())
    })
}

/// Set (empty = clear back to the file-name fallback) a layer's name.
#[tauri::command]
pub fn rename_layer(state: State<'_, AppState>, id: u32, name: String) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.rename_layer(id, &name).map_err(err)?;
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

// ---------------------------------------------------------------------------
// Multitrack recording (#8) — tape machine: enumerate inputs, arm, track,
// stop. All logic lives in still-core; these are thin pass-throughs.

#[tauri::command]
pub fn list_input_devices() -> CmdResult<Vec<still_core::InputDeviceInfo>> {
    Ok(still_core::list_input_devices())
}

#[tauri::command]
pub fn get_default_recording_dir() -> CmdResult<String> {
    Ok(dirs_next_audio_dir().join("Recordings").display().to_string())
}

#[tauri::command]
pub fn record_start(
    state: State<'_, AppState>,
    config: still_core::RecordConfig,
) -> CmdResult<still_core::RecordStatus> {
    let mut rec = state.recorder.lock().unwrap();
    if rec.is_some() {
        return Err("a recording is already running".into());
    }
    // Tracking and playback share no state, but a quiet transport avoids
    // confusion (and feedback loops through open monitors).
    let _ = state.player.pause();
    let handle = still_core::RecorderHandle::start(&config).map_err(err)?;
    let status = handle.status();
    *rec = Some(handle);
    Ok(status)
}

#[tauri::command]
pub fn record_status(state: State<'_, AppState>) -> CmdResult<Option<still_core::RecordStatus>> {
    Ok(state.recorder.lock().unwrap().as_ref().map(|r| r.status()))
}

#[tauri::command]
pub fn record_stop(state: State<'_, AppState>) -> CmdResult<Vec<String>> {
    let handle = state
        .recorder
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| "no recording is running".to_string())?;
    let files = handle.stop().map_err(err)?;
    Ok(files.iter().map(|p| p.display().to_string()).collect())
}

#[tauri::command]
pub fn set_track_isrc(state: State<'_, AppState>, id: u32, isrc: String) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.set_track_isrc(id, &isrc).map_err(err)?;
        Ok(s.view())
    })
}

/// Rebuild the engine's master-insert chain from the project recipe:
/// ensure every configured plugin has a live instance (created on the MAIN
/// thread with its saved state), push ordered proxies to the engine, then
/// dispose orphans. `force_recreate` drops existing instances first
/// (Reload semantics, and session loads at a new format).
fn rebuild_chain(
    app: &AppHandle,
    state: &State<'_, AppState>,
    s: &mut ProjectState,
    force_recreate: bool,
) -> CmdResult<()> {
    state.editors.close_all(app);
    if force_recreate {
        // The engine must release its proxies before disposal.
        state.player.set_master_inserts(Vec::new()).map_err(err)?;
        state.player.set_lane_inserts(Vec::new()).map_err(err)?;
        state.chain.clear(app);
    }
    let mut errors: Vec<String> = Vec::new();
    for cfg in s.all_chain_cfgs() {
        if !state.chain.contains(cfg.id) {
            let blob = cfg.state_b64.as_deref().and_then(still_core::b64::decode);
            if let Err(e) = state.chain.create(
                app,
                cfg.id,
                &cfg.component,
                blob,
                cfg.bypass,
                s.info.sample_rate,
                (s.info.channels.max(1) as usize).min(2),
                state.player.playing_flag(),
            ) {
                errors.push(format!("{}: {e}", cfg.name));
            }
        }
    }
    sync_engine_inserts(state, s)?;
    // Dispose orphans — the union across EVERY chain (a master edit must
    // never dispose a layer's or a track's live plugins).
    let union: Vec<u32> = s.all_chain_cfgs().map(|c| c.id).collect();
    state.chain.retain_only(app, &union);
    if let Some(e) = errors.first() {
        return Err(format!("Plugin chain: {e}"));
    }
    Ok(())
}

/// Push the current insert topology to the engine WITHOUT touching plugin
/// lifecycles or editor windows: per-lane chains (layer order) and the
/// master-bus sections — each track's chain gated to its region span, then
/// the always-on mastering chain. Also called after region edits so the
/// spans follow the track boundaries.
fn sync_engine_inserts(state: &State<'_, AppState>, s: &ProjectState) -> CmdResult<()> {
    let mut sections: Vec<(Option<(u64, u64)>, Vec<Box<dyn BlockProcessor>>)> = Vec::new();
    let mut regions: Vec<&still_core::project::Region> = s
        .project
        .regions
        .iter()
        .filter(|r| !r.inserts.is_empty())
        .collect();
    regions.sort_by_key(|r| r.start);
    for r in regions {
        let ids: Vec<u32> = r.inserts.iter().map(|c| c.id).collect();
        sections.push((Some((r.start, r.end)), state.chain.inserts_for(&ids)));
    }
    let master_ids: Vec<u32> = s.project.mastering_chain.iter().map(|c| c.id).collect();
    sections.push((None, state.chain.inserts_for(&master_ids)));
    state.player.set_master_inserts(sections).map_err(err)?;

    let lanes: Vec<Vec<Box<dyn BlockProcessor>>> = s
        .project
        .layers
        .iter()
        .map(|l| {
            let ids: Vec<u32> = l.inserts.iter().map(|c| c.id).collect();
            state.chain.inserts_for(&ids)
        })
        .collect();
    state.player.set_lane_inserts(lanes).map_err(err)
}


/// After an edit that may have created/removed whole chains (undo, redo,
/// region/layer removal): rebuild when the plugin-id universe changed
/// (instances to create/dispose), otherwise just re-push spans/lanes.
fn resync_chains_after_edit(
    app: &AppHandle,
    state: &State<'_, AppState>,
    s: &mut ProjectState,
    before_ids: &[u32],
) {
    let after: Vec<u32> = s.all_chain_cfgs().map(|c| c.id).collect();
    if after != before_ids {
        let _ = rebuild_chain(app, state, s, false);
    } else {
        let _ = sync_engine_inserts(state, s);
    }
}

/// Persist the LIVE plugin states (knob tweaks) into the project recipe —
/// every chain, every target.
fn snapshot_chain_states(app: &AppHandle, state: &State<'_, AppState>, s: &mut ProjectState) {
    let ids: Vec<u32> = s.all_chain_cfgs().map(|c| c.id).collect();
    let blobs: Vec<Option<Vec<u8>>> = ids
        .iter()
        .map(|id| state.chain.save_state(app, *id))
        .collect();
    for (cfg, blob) in s.all_chain_cfgs_mut().zip(blobs) {
        if let Some(blob) = blob {
            cfg.state_b64 = Some(still_core::b64::encode(&blob));
        }
    }
}

/// The user's extra VST3 scan directories (persisted app-side).
#[tauri::command]
pub fn get_vst3_scan_paths(app: AppHandle) -> CmdResult<Vec<String>> {
    Ok(crate::load_scan_paths(&app))
}

/// Replace the extra VST3 scan directories, rescan (in the throwaway
/// subprocess) and return the refreshed plugin list.
#[tauri::command]
pub async fn set_vst3_scan_paths(
    app: AppHandle,
    paths: Vec<String>,
) -> CmdResult<Vec<PluginInfo>> {
    use tauri::Manager;
    let file = crate::scan_paths_file(&app).ok_or("no config directory")?;
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&file, serde_json::to_string(&paths).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    still_core::vst3::set_extra_dirs(paths.iter().map(Into::into).collect());
    let cache = app
        .path()
        .app_config_dir()
        .map(|d| d.join("vst3_scan_cache.json"))
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::run_scan_subprocess(&cache, &paths);
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(still_core::list_plugins())
}

/// Every installed effect plugin (Audio Units + VST3, macOS).
/// VST3 entries come from the scan cache, refreshed in a background
/// subprocess at startup (see lib.rs) — never scanned in this process.
#[tauri::command]
pub fn list_plugins() -> CmdResult<Vec<PluginInfo>> {
    Ok(still_core::list_plugins())
}

/// Add a plugin at the end of `target`'s chain (master, one layer, or one
/// track). Rolls the entry back if instantiation fails.
#[tauri::command]
pub fn add_chain_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    target: still_core::ChainTarget,
    component: String,
    name: String,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let id = s.project.next_plugin_id;
        s.project.next_plugin_id += 1;
        s.chain_mut(target)
            .ok_or_else(|| "this layer or track no longer exists".to_string())?
            .push(still_core::MasteringPluginCfg {
                id,
                component,
                name,
                bypass: false,
                state_b64: None,
            });
        if let Err(e) = rebuild_chain(&app, &state, s, false) {
            // Instantiation failed: withdraw the entry.
            if let Some(chain) = s.chain_mut(target) {
                chain.retain(|c| c.id != id);
            }
            let _ = rebuild_chain(&app, &state, s, false);
            return Err(e);
        }
        Ok(s.view())
    })
}

/// Remove a plugin from whichever chain owns it (ids are global).
#[tauri::command]
pub fn remove_chain_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u32,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        snapshot_chain_states(&app, &state, s);
        if let Some(chain) = s.chain_containing_mut(id) {
            chain.retain(|c| c.id != id);
        }
        rebuild_chain(&app, &state, s, false)?;
        Ok(s.view())
    })
}

/// Move a plugin by `delta` within its own chain.
#[tauri::command]
pub fn move_chain_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u32,
    delta: i32,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        if let Some(chain) = s.chain_containing_mut(id) {
            if let Some(pos) = chain.iter().position(|c| c.id == id) {
                let new_pos =
                    (pos as i64 + delta as i64).clamp(0, chain.len() as i64 - 1) as usize;
                let item = chain.remove(pos);
                chain.insert(new_pos, item);
            }
        }
        rebuild_chain(&app, &state, s, false)?;
        Ok(s.view())
    })
}

/// Full reload of EVERY chain: snapshot the LIVE states, dispose all
/// instances and re-create them (recovery for wrappers whose DSP silently
/// dies).
#[tauri::command]
pub fn reload_chains(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        snapshot_chain_states(&app, &state, s);
        rebuild_chain(&app, &state, s, true)?;
        Ok(s.view())
    })
}

/// Directory holding the user's named chain presets (app-owned, outside
/// any project).
fn presets_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .map(|d| d.join("chain-presets"))
        .map_err(|e| e.to_string())
}

/// Save `target`'s CURRENT chain (live states included) as a named preset.
/// Same name = overwrite. Returns the refreshed preset list.
#[tauri::command]
pub fn save_chain_preset(
    app: AppHandle,
    state: State<'_, AppState>,
    target: still_core::ChainTarget,
    name: String,
) -> CmdResult<Vec<still_core::ChainPresetInfo>> {
    let dir = presets_dir(&app)?;
    with_session(&state, |s| {
        snapshot_chain_states(&app, &state, s);
        let chain = s
            .chain(target)
            .ok_or_else(|| "this layer or track no longer exists".to_string())?;
        let preset = still_core::ChainPreset::from_chain(&name, chain);
        still_core::chain_presets::save_preset(&dir, &preset).map_err(err)
    })
}

#[tauri::command]
pub fn list_chain_presets(app: AppHandle) -> CmdResult<Vec<still_core::ChainPresetInfo>> {
    still_core::chain_presets::list_presets(&presets_dir(&app)?).map_err(err)
}

/// Replace `target`'s chain with a preset's plugins (fresh project ids,
/// instances recreated). Rolls back to the previous chain if any plugin
/// fails to instantiate.
#[tauri::command]
pub fn load_chain_preset(
    app: AppHandle,
    state: State<'_, AppState>,
    target: still_core::ChainTarget,
    name: String,
) -> CmdResult<ProjectView> {
    let dir = presets_dir(&app)?;
    with_session(&state, |s| {
        let preset = still_core::chain_presets::load_preset(&dir, &name).map_err(err)?;
        let previous_next_id = s.project.next_plugin_id;
        let mut fresh = Vec::with_capacity(preset.plugins.len());
        for p in preset.plugins {
            let id = s.project.next_plugin_id;
            s.project.next_plugin_id += 1;
            fresh.push(still_core::MasteringPluginCfg {
                id,
                component: p.component,
                name: p.name,
                bypass: p.bypass,
                state_b64: p.state_b64,
            });
        }
        let chain = s
            .chain_mut(target)
            .ok_or_else(|| "this layer or track no longer exists".to_string())?;
        let previous = std::mem::replace(chain, fresh);
        if let Err(e) = rebuild_chain(&app, &state, s, true) {
            if let Some(chain) = s.chain_mut(target) {
                *chain = previous;
            }
            s.project.next_plugin_id = previous_next_id;
            let _ = rebuild_chain(&app, &state, s, true);
            return Err(e);
        }
        Ok(s.view())
    })
}

/// Delete a preset. Returns the refreshed preset list.
#[tauri::command]
pub fn delete_chain_preset(
    app: AppHandle,
    name: String,
) -> CmdResult<Vec<still_core::ChainPresetInfo>> {
    still_core::chain_presets::delete_preset(&presets_dir(&app)?, &name).map_err(err)
}

/// Total reported latency of `target`'s LIVE chain, in samples at the
/// session rate. Display only — export compensates on its own.
#[tauri::command]
pub fn chain_latency(
    state: State<'_, AppState>,
    target: still_core::ChainTarget,
) -> CmdResult<u64> {
    with_session(&state, |s| {
        let ids: Vec<u32> = s
            .chain(target)
            .map(|c| c.iter().map(|p| p.id).collect())
            .unwrap_or_default();
        Ok(state.chain.total_latency(&ids))
    })
}

/// Live bypass — the plugin instance keeps its state. Works on any chain
/// (plugin ids are global).
#[tauri::command]
pub fn set_chain_bypass(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u32,
    bypass: bool,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        if let Some(cfg) = s
            .all_chain_cfgs_mut()
            .find(|c| c.id == id)
        {
            cfg.bypass = bypass;
        }
        state.chain.set_bypass(&app, id, bypass)?;
        Ok(s.view())
    })
}

/// Open (or re-show) the native editor window of a mastering plugin.
/// Dispatches on the component id prefix: AU editors go through the
/// AudioUnit handle, VST3 editors through IPlugView.
#[tauri::command]
pub fn open_plugin_editor(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u32,
) -> CmdResult<()> {
    let (component, name) = with_session(&state, |s| {
        s.all_chain_cfgs()
            .find(|c| c.id == id)
            .map(|c| (c.component.clone(), c.name.clone()))
            .ok_or_else(|| "unknown plugin".to_string())
    })?;
    #[cfg(target_os = "macos")]
    if still_core::plugins::format_of(&component) == still_core::PluginFormat::Vst3 {
        return state.editors.open_vst3(&app, id, &state.chain, &name);
    }
    let _ = &component;
    let unit = state.chain.raw_handle(id);
    if unit == 0 {
        return Err("This plugin is not running (did it fail to load?).".into());
    }
    state.editors.open(&app, id, unit, &name)
}

/// Live master-bus loudness snapshot (EBU R128, post-chain).
#[tauri::command]
pub fn meter_state(state: State<'_, AppState>) -> CmdResult<still_core::MeterState> {
    Ok(state.player.meter_state())
}

/// Restart integrated loudness / LRA / true-peak max-hold.
#[tauri::command]
pub fn reset_meter(state: State<'_, AppState>) -> CmdResult<()> {
    state.player.reset_meter().map_err(err)
}

/// Base64 preview of the project's cover image (display only).
#[tauri::command]
pub fn get_artwork_preview(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    with_session(&state, |s| {
        let path = &s.project.album_meta.artwork_path;
        if path.is_empty() {
            return Ok(None);
        }
        let (data, mime) =
            still_core::metadata::load_artwork(Path::new(path)).map_err(err)?;
        use std::fmt::Write as _;
        let mut b64 = String::with_capacity(data.len() * 4 / 3 + 4);
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for chunk in data.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
            let _ = write!(
                b64,
                "{}{}{}{}",
                T[(n >> 18 & 63) as usize] as char,
                T[(n >> 12 & 63) as usize] as char,
                if chunk.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' },
                if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' },
            );
        }
        Ok(Some(format!("data:{mime};base64,{b64}")))
    })
}

/// Store the album metadata (format-agnostic; written to exported files).
#[tauri::command]
pub fn set_album_meta(state: State<'_, AppState>, meta: AlbumMeta) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        s.project.album_meta = meta;
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
pub fn undo(app: AppHandle, state: State<'_, AppState>) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let before: Vec<u32> = s.all_chain_cfgs().map(|c| c.id).collect();
        s.undo();
        sync_playback(&state, s);
        resync_chains_after_edit(&app, &state, s, &before);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn redo(app: AppHandle, state: State<'_, AppState>) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        let before: Vec<u32> = s.all_chain_cfgs().map(|c| c.id).collect();
        s.redo();
        sync_playback(&state, s);
        resync_chains_after_edit(&app, &state, s, &before);
        Ok(s.view())
    })
}

#[tauri::command]
pub fn detect_silences(
    state: State<'_, AppState>,
    params: SilenceParams,
    layer_id: Option<u32>,
) -> CmdResult<Vec<RegionSpan>> {
    with_session(&state, |s| {
        // Default: detect on the same mix the user sees/hears. With a
        // layer id, detect on THAT layer's own pyramid instead — in
        // multitrack sessions one bleeding input can fill the very
        // silences that separate the songs, while a quiet-between-songs
        // layer (a DI, a spot mic) is a far better detector.
        let pyramid = match layer_id {
            Some(id) => {
                let idx = s
                    .project
                    .layers
                    .iter()
                    .position(|l| l.id == id)
                    .ok_or_else(|| format!("unknown layer id {id}"))?;
                s.peaks
                    .get(idx)
                    .cloned()
                    .ok_or_else(|| "layer has no peak data".to_string())?
            }
            None => s.merged_pyramid(),
        };
        Ok(still_core::detect_track_regions(
            &pyramid,
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
        // Export what you HEAR: capture the live plugin states first.
        snapshot_chain_states(&app, &state, s);
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
        let jobs = if config.stems {
            // Multitrack export: one job per (track × layer). Layer names
            // follow the UI (custom name, else the source file's stem).
            let stem_layers: Vec<still_core::export::StemLayer> = s
                .project
                .layers
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let first = s.info.layers.get(i).and_then(|sc| sc.clips.first());
                    let name = l.custom_name.clone().unwrap_or_else(|| {
                        first
                            .and_then(|c| {
                                std::path::Path::new(&c.name)
                                    .file_stem()
                                    .map(|n| n.to_string_lossy().to_string())
                            })
                            .unwrap_or_else(|| format!("Layer {}", i + 1))
                    });
                    still_core::export::StemLayer {
                        name,
                        source_path: first.map(|c| c.path.clone()).unwrap_or_default(),
                    }
                })
                .collect();
            still_core::export::plan_export_stems(
                &tracks,
                &config,
                &source,
                &s.project.album_meta,
                &stem_layers,
                config.stems_apply_mix,
            )
            .map_err(err)?
        } else {
            still_core::export::plan_export_with_meta(
                &tracks,
                &config,
                &source,
                &s.project.album_meta,
            )
            .map_err(err)?
        };
        let layers: Vec<still_core::LayerMix> = s
            .info
            .layers
            .iter()
            .map(|scanned| still_core::LayerMix {
                clips: scanned.clips.clone(),
            })
            .collect();
        let specs_of = |cfgs: &[still_core::MasteringPluginCfg]| -> Vec<still_core::MasterPluginSpec> {
            cfgs.iter()
                .map(|c| still_core::MasterPluginSpec {
                    id: c.id,
                    component: c.component.clone(),
                    bypass: c.bypass,
                    state: c.state_b64.as_deref().and_then(still_core::b64::decode),
                })
                .collect()
        };
        // Stems never go through the track/master chains; raw stems skip
        // the layer inserts too (untouched cut of the source).
        let chain = if config.stems {
            Vec::new()
        } else {
            specs_of(&s.project.mastering_chain)
        };
        // Per-layer chains, index-aligned with `layers`.
        let lane_chains: Vec<Vec<still_core::MasterPluginSpec>> =
            if config.stems && !config.stems_apply_mix {
                Vec::new()
            } else {
                s.project
                    .layers
                    .iter()
                    .map(|l| specs_of(&l.inserts))
                    .collect()
            };
        // Each mixdown job renders one region: attach that track's own
        // chain (jobs and `tracks` are index-aligned by construction —
        // NOT true in stems mode, which carries no track chains at all).
        let mut jobs = jobs;
        if !config.stems {
            for (job, t) in jobs.iter_mut().zip(&tracks) {
                if let Some(r) = s.project.regions.iter().find(|r| r.id == t.id) {
                    job.track_chain = specs_of(&r.inserts);
                }
            }
        }
        Ok((
            layers,
            s.info.channels,
            s.info.sample_rate,
            jobs,
            chain,
            lane_chains,
            s.project.album_meta.clone(),
        ))
    });
    let (layers, session_channels, sample_rate, jobs, chain, lane_chains, meta) = match prepared {
        Ok(x) => x,
        Err(e) => {
            state.export_running.store(false, Ordering::SeqCst);
            return Err(e);
        }
    };

    let cancel = state.export_cancel.clone();
    let app2 = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        // The bundled sidecar (externalBin) lives right next to the app
        // binary; system installs remain the fallback for dev setups.
        let sidecar: Vec<PathBuf> = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.to_path_buf()))
            .map(|dir| {
                vec![dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" })]
            })
            .unwrap_or_default();
        let ffmpeg = still_core::resolve_ffmpeg(&sidecar)?;
        // Progress arrives from several worker threads at once; throttle the
        // stream globally but always let start/end events through.
        let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
        if (config.cd_image || config.ddp) && !config.stems {
            let emit = |p: still_core::export::ExportProgress| {
                let force = p.track_progress == 0.0 || p.track_progress == 1.0;
                let mut last = last.lock().unwrap();
                if force || last.elapsed() >= Duration::from_millis(60) {
                    *last = Instant::now();
                    let _ = app2.emit("export:progress", &p);
                }
            };
            let report = if config.ddp {
                still_core::export::run_export_ddp(
                    &ffmpeg,
                    &layers,
                    session_channels,
                    sample_rate,
                    &jobs,
                    &config,
                    &chain,
                    &lane_chains,
                    &meta,
                    &cancel,
                    emit,
                )
            } else {
                still_core::export::run_export_cd_image(
                    &ffmpeg,
                    &layers,
                    session_channels,
                    sample_rate,
                    &jobs,
                    &config,
                    &chain,
                    &lane_chains,
                    &meta,
                    &cancel,
                    emit,
                )
            };
            return Ok::<ExportReport, still_core::StillError>(report);
        }
        Ok::<ExportReport, still_core::StillError>(still_core::export::run_export_with_chain(
            &ffmpeg,
            &layers,
            session_channels,
            sample_rate,
            &jobs,
            &config,
            &chain,
            &lane_chains,
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
pub fn save_project(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> CmdResult<ProjectView> {
    with_session(&state, |s| {
        // Persist the LIVE plugin states (knob tweaks) into the recipe.
        snapshot_chain_states(&app, &state, s);
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
            if !Path::new(&source.path).is_file() {
                return Err(format!(
                    "A source audio file referenced by this project was not found: {}",
                    source.path
                ));
            }
        }
    }
    let refs = project.source_groups();
    let (info, peaks) = scan_sources(&app, &state, &refs).await?;
    let mut ps = ProjectState::new(project, info, peaks);
    ps.project_path = Some(project_path);
    state
        .player
        .load_session(
            playlists_of(&ps.info),
            ps.info.duration_seconds,
            automation_of(&ps),
            ps.info.sample_rate,
            ps.info.channels.max(1) as usize,
        )
        .map_err(err)?;
    still_core::sanitize_regions(&mut ps.project, ps.info.duration_samples, ps.info.sample_rate);
    let view = ps.view();
    *state.session.lock().unwrap() = Some(ps);
    Ok(view)
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
