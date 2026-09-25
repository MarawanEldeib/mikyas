//! OS-specific window touches.

#[cfg(windows)]
mod windows;

/// Called once after the window is created.
pub fn after_create(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    windows::after_create(window);
    #[cfg(not(windows))]
    let _ = window;
}
