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

/// Shows the window without taking the focus (follow with Tauri's `show()`; no-op elsewhere).
pub fn show_without_activating(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    windows::show_without_activating(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// The native window handle as an integer.
pub fn window_id(window: &tauri::WebviewWindow) -> Option<isize> {
    #[cfg(windows)]
    return windows::window_id(window);
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// The monitor a native window is on.
pub fn monitor_id(window: isize) -> Option<isize> {
    #[cfg(windows)]
    return windows::monitor_id(window);
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

/// The foreground window, for fullscreen auto-hide (window queries only).
pub fn foreground_window(own: Option<isize>) -> Option<crate::fullscreen::Foreground> {
    #[cfg(windows)]
    return windows::foreground_window(own);
    #[cfg(not(windows))]
    {
        let _ = own;
        None
    }
}

/// The shell's notification state (fullscreen / presentation).
pub fn notification_state() -> crate::fullscreen::Quns {
    #[cfg(windows)]
    return windows::notification_state();
    #[cfg(not(windows))]
    crate::fullscreen::Quns::Other
}
