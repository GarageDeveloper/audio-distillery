use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use still_core::{PlayerHandle, ProjectState};

use crate::editor::EditorRegistry;

/// Global app state managed by Tauri. The canonical project state (single
/// source of truth, SPEC §3) lives here, in the backend.
pub struct AppState {
    pub session: Mutex<Option<ProjectState>>,
    pub player: PlayerHandle,
    pub export_cancel: Arc<AtomicBool>,
    pub export_running: AtomicBool,
    pub scan_cancel: Arc<AtomicBool>,
    pub editors: EditorRegistry,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            player: PlayerHandle::spawn(),
            export_cancel: Arc::new(AtomicBool::new(false)),
            export_running: AtomicBool::new(false),
            scan_cancel: Arc::new(AtomicBool::new(false)),
            editors: EditorRegistry::default(),
        }
    }
}
