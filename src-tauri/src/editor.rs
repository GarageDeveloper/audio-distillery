//! Native plugin editor windows (macOS): thin safe wrapper over the ObjC
//! shim in `native/au_editor.m`. All AppKit and plugin-view calls are
//! dispatched to the main thread through Tauri. The registry maps plugin
//! id → open window; `close_all` MUST run before any chain rebuild (the
//! windows reference plugin instances the rebuild destroys).
//!
//! VST3 editors need two extras the AU path doesn't: the IPlugView is
//! attached into the window's container NSView from Rust, and
//! plugin-requested resizes are DEFERRED (IPlugFrame mailbox) — a small
//! pump wakes the main thread every ~30 ms while at least one VST3 editor
//! is open, drains pending sizes, resizes the window and calls onSize.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::AppHandle;

#[cfg(target_os = "macos")]
extern "C" {
    fn still_open_au_editor(unit: usize, title: *const std::os::raw::c_char) -> usize;
    fn still_show_au_editor(win: usize);
    fn still_close_au_editor(win: usize);
    fn still_open_plugin_window(
        title: *const std::os::raw::c_char,
        width: i32,
        height: i32,
    ) -> usize;
    fn still_plugin_window_container(win: usize) -> usize;
    fn still_show_plugin_window(win: usize);
    fn still_resize_plugin_window(win: usize, width: i32, height: i32);
    fn still_close_plugin_window(win: usize);
}

enum EditorWindow {
    /// Retained StillEditorContext pointer (AU path).
    Au(usize),
    /// Retained NSWindow pointer + the live VST3 editor view.
    #[cfg(target_os = "macos")]
    Vst3 {
        win: usize,
        editor: still_core::vst3::Vst3Editor,
    },
}

#[derive(Default)]
pub struct EditorRegistry {
    windows: Arc<Mutex<HashMap<u32, EditorWindow>>>,
    pump_running: Arc<AtomicBool>,
}

impl EditorRegistry {
    /// Open (or re-show) the editor window of AU plugin `id` whose
    /// AudioUnit handle is `unit`. `title` is the plugin display name.
    #[cfg(target_os = "macos")]
    pub fn open(&self, app: &AppHandle, id: u32, unit: usize, title: &str) -> Result<(), String> {
        if self.reshow(app, id) {
            return Ok(());
        }
        let title_c = std::ffi::CString::new(title.to_string())
            .unwrap_or_else(|_| std::ffi::CString::new("Plugin").unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = app.run_on_main_thread(move || {
            let win = unsafe { still_open_au_editor(unit, title_c.as_ptr()) };
            let _ = tx.send(win);
        });
        let win = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "the editor window did not open".to_string())?;
        if win == 0 {
            return Err("this plugin did not provide an editor view".to_string());
        }
        self.windows.lock().unwrap().insert(id, EditorWindow::Au(win));
        Ok(())
    }

    /// Open (or re-show) the native VST3 editor of plugin `id`: create the
    /// view (main thread), size a window from it, attach into the container
    /// NSView, then start the resize pump.
    #[cfg(target_os = "macos")]
    pub fn open_vst3(
        &self,
        app: &AppHandle,
        id: u32,
        chain: &crate::chain_host::ChainHost,
        title: &str,
    ) -> Result<(), String> {
        if self.reshow(app, id) {
            return Ok(());
        }
        let title_c = std::ffi::CString::new(title.to_string())
            .unwrap_or_else(|_| std::ffi::CString::new("Plugin").unwrap());
        let created = chain.with_plugin(app, id, move |p| {
            let plugin = p
                .as_any()
                .and_then(|a| a.downcast_mut::<still_core::vst3::Vst3Plugin>())
                .ok_or_else(|| "this plugin is not a VST3 instance".to_string())?;
            let mut editor = plugin.create_editor().map_err(|e| e.to_string())?;
            let (w, h) = editor.size();
            let win = unsafe { still_open_plugin_window(title_c.as_ptr(), w, h) };
            if win == 0 {
                return Err("could not create the editor window".to_string());
            }
            let container = unsafe { still_plugin_window_container(win) };
            editor
                .attach(container as *mut std::ffi::c_void)
                .map_err(|e| {
                    unsafe { still_close_plugin_window(win) };
                    e.to_string()
                })?;
            unsafe { still_show_plugin_window(win) };
            Ok((win, editor))
        })?;
        let Some(result) = created else {
            return Err("this plugin is not running (did it fail to load?)".to_string());
        };
        let (win, editor) = result?;
        self.windows
            .lock()
            .unwrap()
            .insert(id, EditorWindow::Vst3 { win, editor });
        self.start_resize_pump(app);
        Ok(())
    }

    /// Re-show an already open window. True when handled.
    #[cfg(target_os = "macos")]
    fn reshow(&self, app: &AppHandle, id: u32) -> bool {
        let guard = self.windows.lock().unwrap();
        let handle = match guard.get(&id) {
            Some(EditorWindow::Au(win)) => Some((*win, true)),
            Some(EditorWindow::Vst3 { win, .. }) => Some((*win, false)),
            None => None,
        };
        drop(guard);
        if let Some((win, is_au)) = handle {
            let _ = app.run_on_main_thread(move || unsafe {
                if is_au {
                    still_show_au_editor(win);
                } else {
                    still_show_plugin_window(win);
                }
            });
            return true;
        }
        false
    }

    /// Wake the main thread periodically to apply plugin-requested editor
    /// resizes (deferred by the IPlugFrame mailbox). Parks itself when the
    /// last VST3 editor closes.
    #[cfg(target_os = "macos")]
    fn start_resize_pump(&self, app: &AppHandle) {
        if self.pump_running.swap(true, Ordering::AcqRel) {
            return;
        }
        let windows = self.windows.clone();
        let running = self.pump_running.clone();
        let app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let any_vst3 = windows
                .lock()
                .unwrap()
                .values()
                .any(|w| matches!(w, EditorWindow::Vst3 { .. }));
            if !any_vst3 {
                running.store(false, Ordering::Release);
                return;
            }
            let windows2 = windows.clone();
            let _ = app.run_on_main_thread(move || {
                let mut guard = windows2.lock().unwrap();
                for w in guard.values_mut() {
                    if let EditorWindow::Vst3 { win, editor } = w {
                        if let Some((width, height)) = editor.take_pending_resize() {
                            unsafe { still_resize_plugin_window(*win, width, height) };
                            editor.on_size(width, height);
                        }
                    }
                }
            });
        });
    }

    #[cfg(not(target_os = "macos"))]
    pub fn open(&self, _: &AppHandle, _: u32, _: usize, _: &str) -> Result<(), String> {
        Err("Plugin editors are only available on macOS".to_string())
    }

    /// Close and release every editor window (before a chain rebuild).
    /// VST3 editors detach (setFrame(null) → removed) BEFORE their window
    /// closes and before the chain disposes the plugin instances.
    pub fn close_all(&self, app: &AppHandle) {
        let windows: Vec<EditorWindow> = {
            let mut map = self.windows.lock().unwrap();
            map.drain().map(|(_, w)| w).collect()
        };
        if windows.is_empty() {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = app.run_on_main_thread(move || {
                for w in windows {
                    match w {
                        EditorWindow::Au(win) => unsafe { still_close_au_editor(win) },
                        EditorWindow::Vst3 { win, editor } => {
                            drop(editor);
                            unsafe { still_close_plugin_window(win) };
                        }
                    }
                }
                let _ = tx.send(());
            });
            // The rebuild that follows will dispose the instances the views
            // referenced — wait until the detach actually happened.
            let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = app;
            drop(windows);
        }
    }
}
