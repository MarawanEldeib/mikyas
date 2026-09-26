//! OS-specific window touches.

#[cfg(windows)]
mod windows;

/// Restricts where the process loads DLLs from to the app's folder, System32 and folders added
/// with `AddDllDirectory` (no current directory, no `PATH`). Call first thing in `main`, before
/// anything loads a DLL on demand. No-op elsewhere.
#[allow(dead_code, reason = "main.rs calls it first thing, through a lib.rs re-export")]
pub fn restrict_dll_search() {
    #[cfg(windows)]
    windows::restrict_dll_search();
}

/// The Windows `SystemUsesLightTheme` value (the taskbar's colour scheme); `None` when unreadable
/// or on other systems.
pub fn system_uses_light_theme() -> Option<bool> {
    #[cfg(windows)]
    return windows::system_uses_light_theme();
    #[cfg(not(windows))]
    None
}

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

/// Moves and sizes the window in one call (outer rect, physical px); `false` where that is not
/// available, and the caller moves and resizes separately.
pub fn set_outer_rect(window: &tauri::WebviewWindow, rect: (i32, i32, i32, i32)) -> bool {
    #[cfg(windows)]
    return windows::set_outer_rect(window, rect);
    #[cfg(not(windows))]
    {
        let _ = (window, rect);
        false
    }
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

/// The current foreground window's handle (0 if none or unknown).
pub fn foreground_handle() -> isize {
    #[cfg(windows)]
    return windows::foreground_handle();
    #[cfg(not(windows))]
    0
}

/// Gives the foreground (keyboard focus) back to a window that had it (no-op elsewhere).
pub fn set_foreground(window: isize) {
    #[cfg(windows)]
    windows::set_foreground(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// The shell's notification state (fullscreen / presentation).
pub fn notification_state() -> crate::fullscreen::Quns {
    #[cfg(windows)]
    return windows::notification_state();
    #[cfg(not(windows))]
    crate::fullscreen::Quns::Other
}
