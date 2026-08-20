//! Native AU editor windows (macOS): thin safe wrapper over the ObjC shim
//! in `native/au_editor.m`. All AppKit calls are dispatched to the main
//! thread through Tauri. The registry maps plugin id → retained NSWindow;
//! `close_all` MUST run before any chain rebuild (the windows reference
//! AudioUnit instances the rebuild destroys).

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::AppHandle;

#[cfg(target_os = "macos")]
extern "C" {
    fn still_open_au_editor(unit: usize, title: *const std::os::raw::c_char) -> usize;
    fn still_show_au_editor(win: usize);
    fn still_close_au_editor(win: usize);
}

#[derive(Default)]
pub struct EditorRegistry {
    windows: Mutex<HashMap<u32, usize>>,
}

impl EditorRegistry {
    /// Open (or re-show) the editor window of plugin `id` whose AudioUnit
    /// handle is `unit`. `title` is the plugin display name.
    #[cfg(target_os = "macos")]
    pub fn open(
        &self,
        app: &AppHandle,
        id: u32,
        unit: usize,
        title: &str,
    ) -> Result<(), String> {
        use tauri::Manager;
        if let Some(&win) = self.windows.lock().unwrap().get(&id) {
            let _ = app.run_on_main_thread(move || unsafe {
                still_show_au_editor(win);
            });
            return Ok(());
        }
        let title_c = std::ffi::CString::new(title.to_string())
            .unwrap_or_else(|_| std::ffi::CString::new("Plugin").unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = app.app_handle().clone();
        let _ = handle.run_on_main_thread(move || {
            let win = unsafe { still_open_au_editor(unit, title_c.as_ptr()) };
            let _ = tx.send(win);
        });
        let win = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "the editor window did not open".to_string())?;
        if win == 0 {
            return Err("this plugin did not provide an editor view".to_string());
        }
        self.windows.lock().unwrap().insert(id, win);
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn open(&self, _: &AppHandle, _: u32, _: usize, _: &str) -> Result<(), String> {
        Err("Plugin editors are only available on macOS".to_string())
    }

    /// Close and release every editor window (before a chain rebuild).
    pub fn close_all(&self, app: &AppHandle) {
        let windows: Vec<usize> = {
            let mut map = self.windows.lock().unwrap();
            map.drain().map(|(_, w)| w).collect()
        };
        if windows.is_empty() {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let _ = app.run_on_main_thread(move || {
                for w in windows {
                    unsafe { still_close_au_editor(w) };
                }
            });
        }
        #[cfg(not(target_os = "macos"))]
        let _ = app;
    }
}
