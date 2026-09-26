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

/// WebView2 memory target (no-op elsewhere).
pub fn set_memory_low(window: &tauri::WebviewWindow, low: bool) {
    #[cfg(windows)]
    windows::set_memory_low(window, low);
    #[cfg(not(windows))]
    let _ = (window, low);
}
