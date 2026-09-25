//! Windows 11 specifics: DWM rounded corners on the frameless window. (Non-activation uses
//! `set_focusable(false)`, which Tauri implements with `WS_EX_NOACTIVATE`.)

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
