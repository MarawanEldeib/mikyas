//! Tray icon with the live % drawn into it.
//!
//! TODO(stream shell): implement.
//! - `settings.tray_number` picks the value: `worst` (highest of 5-hour and weekly), `five_hour`,
//!   `seven_day`, or `off` (the coloured dot icons in `tray.rs`).
//! - Bitmap digits at 16/20/24/32 px (DPI 100–200%), coloured by level, readable on a light or
//!   dark taskbar (registry `SystemUsesLightTheme`), a distinct glyph at 100%, grey when stale.

use cuw_core::engine::types::Snapshot;
use tauri::image::Image;

use crate::settings::TrayNumber;

/// The number icon for `snapshot`, or `None` for the dot icons. TODO(stream shell).
pub fn number_icon(snapshot: &Snapshot, mode: TrayNumber) -> Option<Image<'static>> {
    let _ = (snapshot, mode);
    None
}
