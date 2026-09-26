//! Windows specifics: DWM rounded corners on the frameless window, WebView2's low memory target
//! (the widget's page is tiny and mostly idle, so trimming caches costs nothing visible), showing
//! without activation, moving and resizing in one step, and the window queries behind
//! fullscreen auto-hide.
//! (Non-activation uses `set_focusable(false)`, which Tauri implements with `WS_EX_NOACTIVATE`.)

use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL, ICoreWebView2_19,
};
use windows_core::Interface;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow};
use windows_sys::Win32::UI::Shell::{QUERY_USER_NOTIFICATION_STATE, SHQueryUserNotificationState};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowRect, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    ShowWindow,
};

use crate::fullscreen::{Foreground, Quns, Rect};

pub fn after_create(window: &tauri::WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else { return };
    let preference = DWMWCP_ROUND;
    // SAFETY: a valid top-level HWND owned by this process, and a pointer to a live u32-sized
    // value of the documented type for this attribute. Failure (Windows 10) is harmless.
    unsafe {
        DwmSetWindowAttribute(
            hwnd.0 as HWND,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&raw const preference).cast(),
            std::mem::size_of_val(&preference) as u32,
        );
    }
}

/// Asks WebView2 to keep its memory use low (`true`) or normal.
pub fn set_memory_low(window: &tauri::WebviewWindow, low: bool) {
    let level = if low {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    let _ = window.with_webview(move |webview| {
        // SAFETY: COM calls on the WebView2 objects Tauri hands us on its UI thread; an older
        // runtime without ICoreWebView2_19 simply fails the cast.
        unsafe {
            if let Ok(core) = webview.controller().CoreWebView2() {
                if let Ok(core19) = core.cast::<ICoreWebView2_19>() {
                    let _ = core19.SetMemoryUsageTargetLevel(level);
                }
            }
        }
    });
}

/// The window's handle as an integer (for comparisons and the queries below).
pub fn window_id(window: &tauri::WebviewWindow) -> Option<isize> {
    window.hwnd().ok().map(|h| h.0 as isize)
}

/// Makes the window visible without activating it (`SW_SHOWNOACTIVATE`): the app in front keeps
/// the keyboard focus. Follow with Tauri's `show()` so its own visibility state stays in sync;
/// `ShowWindow(SW_SHOW)` on an already visible window does nothing.
pub fn show_without_activating(window: &tauri::WebviewWindow) {
    let Some(hwnd) = window_id(window) else { return };
    // SAFETY: a top-level window of this process; ShowWindow has no memory arguments.
    unsafe {
        ShowWindow(hwnd as HWND, SW_SHOWNOACTIVATE);
    }
}

/// Moves and sizes the window in one `SetWindowPos` (outer rect, physical px); `false` if that
/// failed.
pub fn set_outer_rect(window: &tauri::WebviewWindow, (x, y, w, h): (i32, i32, i32, i32)) -> bool {
    let Some(hwnd) = window_id(window) else { return false };
    // SAFETY: a top-level window of this process; SetWindowPos has no memory arguments.
    unsafe { SetWindowPos(hwnd as HWND, std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) != 0 }
}

// Fullscreen detection. Window queries only (no process handles, memory reads, injection or
// hooks), which keeps the widget safe next to kernel anti-cheat such as Riot Vanguard.

fn rect(r: &RECT) -> Rect {
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

fn monitor_rect(monitor: HMONITOR) -> Option<Rect> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    // SAFETY: `info` is a live MONITORINFO with `cbSize` set, as the API requires.
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then(|| rect(&info.rcMonitor))
}

/// The monitor a window is on (`None` when it is on none).
pub fn monitor_id(hwnd: isize) -> Option<isize> {
    // SAFETY: MonitorFromWindow tolerates any handle value and returns null for an invalid one.
    let monitor = unsafe { MonitorFromWindow(hwnd as HWND, MONITOR_DEFAULTTONULL) };
    (!monitor.is_null()).then_some(monitor as isize)
}

/// The foreground window's class, rectangle and monitor; `None` when there is none (e.g. while
/// the workstation is locked) or it is on no monitor.
pub fn foreground_window(own: Option<isize>) -> Option<Foreground> {
    // SAFETY: no arguments; returns null when no window is in the foreground.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut r = RECT::default();
    // SAFETY: `r` is a live RECT the call fills in.
    if unsafe { GetWindowRect(hwnd, &mut r) } == 0 {
        return None;
    }
    let monitor_id = monitor_id(hwnd as isize)?;
    let monitor = monitor_rect(monitor_id as HMONITOR)?;
    let mut class = [0u16; 256];
    // SAFETY: the buffer and its length in UTF-16 units match; the result is the length copied.
    let len = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    let class = String::from_utf16_lossy(&class[..len.clamp(0, class.len() as i32) as usize]);
    Some(Foreground {
        class,
        rect: rect(&r),
        monitor,
        monitor_id,
        is_ours: own == Some(hwnd as isize),
    })
}

/// The shell's notification state (fullscreen app, D3D exclusive mode, presentation settings).
pub fn notification_state() -> Quns {
    let mut state: QUERY_USER_NOTIFICATION_STATE = 0;
    // SAFETY: `state` is a live out-parameter of the documented type.
    if unsafe { SHQueryUserNotificationState(&mut state) } < 0 {
        return Quns::Other;
    }
    Quns::from_raw(state)
}
