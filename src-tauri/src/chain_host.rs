//! Main-thread plugin host for the mastering chain.
//!
//! LESSON LEARNED (the Neutron 5 deadlock + silent-bypass bug): AU plugin
//! LIFECYCLE — instantiation, state get/set, disposal — belongs on the MAIN
//! thread. Plugins schedule async work on the runloop of the thread that
//! created them and dispatch_sync to the main queue from lifecycle calls;
//! doing lifecycle on the engine thread (unpumped runloop) starves those
//! callbacks (dead DSP after an in-plugin preset load) and deadlocks
//! against a main thread waiting on the engine.
//!
//! Here: instances live in `Arc<Mutex<Box<dyn BlockProcessor>>>` owned by
//! this registry; every lifecycle operation hops to the main thread via
//! `run_on_main_thread` (inline when already there); the engine only ever
//! receives `SharedInsert` proxies that `try_lock` for processing.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use still_core::engine::render::{BlockProcessor, SharedInsert};

use tauri::AppHandle;

type Slot = Arc<Mutex<Box<dyn BlockProcessor>>>;

#[derive(Default)]
pub struct ChainHost {
    slots: Mutex<HashMap<u32, Slot>>,
}

/// Run `f` on the main thread and wait for its result.
fn on_main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_secs(15))
        .map_err(|_| "main-thread operation timed out".to_string())
}

impl ChainHost {
    /// Instantiate a plugin ON THE MAIN THREAD and register it under `id`,
    /// restoring `state` when given.
    #[cfg(target_os = "macos")]
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        app: &AppHandle,
        id: u32,
        component: &str,
        state: Option<Vec<u8>>,
        bypass: bool,
        sample_rate: u32,
        channels: usize,
        playing: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        let component = component.to_string();
        let slot: Slot = on_main(app, move || -> Result<Slot, String> {
            let mut p = still_core::create_plugin(&component, sample_rate, channels, playing)
                .map_err(|e| e.to_string())?;
            if let Some(s) = &state {
                let _ = p.restore_state(s);
            }
            p.set_bypassed(bypass);
            Ok(Arc::new(Mutex::new(p)))
        })??;
        self.slots.lock().unwrap().insert(id, slot);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        _app: &AppHandle,
        _id: u32,
        _component: &str,
        _state: Option<Vec<u8>>,
        _bypass: bool,
        _sample_rate: u32,
        _channels: usize,
        _playing: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        Err("Audio Unit hosting is only available on macOS".into())
    }

    pub fn contains(&self, id: u32) -> bool {
        self.slots.lock().unwrap().contains_key(&id)
    }

    /// Proxies for the engine, in the given order (missing ids skipped).
    pub fn inserts_for(&self, ids: &[u32]) -> Vec<Box<dyn BlockProcessor>> {
        let slots = self.slots.lock().unwrap();
        ids.iter()
            .filter_map(|id| slots.get(id))
            .map(|slot| {
                Box::new(SharedInsert {
                    inner: slot.clone(),
                }) as Box<dyn BlockProcessor>
            })
            .collect()
    }

    /// Capture a plugin's live state (main thread).
    pub fn save_state(&self, app: &AppHandle, id: u32) -> Option<Vec<u8>> {
        let slot = self.slots.lock().unwrap().get(&id)?.clone();
        on_main(app, move || slot.lock().unwrap().save_state())
            .ok()
            .flatten()
    }

    /// Live bypass (short main-thread lock; render passes dry meanwhile).
    pub fn set_bypass(&self, app: &AppHandle, id: u32, bypass: bool) -> Result<(), String> {
        let Some(slot) = self.slots.lock().unwrap().get(&id).cloned() else {
            return Ok(());
        };
        on_main(app, move || slot.lock().unwrap().set_bypassed(bypass))
    }

    /// Run `f` on the MAIN thread with exclusive access to the plugin
    /// instance behind `id`. Ok(None) when the id has no live instance.
    pub fn with_plugin<T: Send + 'static>(
        &self,
        app: &AppHandle,
        id: u32,
        f: impl FnOnce(&mut Box<dyn BlockProcessor>) -> T + Send + 'static,
    ) -> Result<Option<T>, String> {
        let Some(slot) = self.slots.lock().unwrap().get(&id).cloned() else {
            return Ok(None);
        };
        on_main(app, move || {
            let mut p = slot.lock().unwrap();
            Some(f(&mut p))
        })
    }

    /// Native handle for the editor window.
    pub fn raw_handle(&self, id: u32) -> usize {
        self.slots
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.lock().map(|p| p.raw_handle()).unwrap_or(0))
            .unwrap_or(0)
    }

    /// Dispose plugins NOT in `keep` — on the main thread. Call only after
    /// the engine acknowledged a chain swap (it holds no Arc anymore).
    pub fn retain_only(&self, app: &AppHandle, keep: &[u32]) {
        let removed: Vec<Slot> = {
            let mut slots = self.slots.lock().unwrap();
            let ids: Vec<u32> = slots.keys().copied().filter(|k| !keep.contains(k)).collect();
            ids.iter().filter_map(|id| slots.remove(id)).collect()
        };
        if !removed.is_empty() {
            let _ = on_main(app, move || drop(removed));
        }
    }

    /// Dispose everything (session change).
    pub fn clear(&self, app: &AppHandle) {
        self.retain_only(app, &[]);
    }
}
