//! Windows specifics: DWM rounded corners on the frameless window, and WebView2's low memory
//! target (the widget's page is tiny and mostly idle, so trimming caches costs nothing visible).
//! (Non-activation uses `set_focusable(false)`, which Tauri implements with `WS_EX_NOACTIVATE`.)

use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL, ICoreWebView2_19,
};
use windows_core::Interface;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};

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
