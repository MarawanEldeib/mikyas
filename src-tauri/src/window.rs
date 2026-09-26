//! The single widget window: creation, view sizes (anchored to the nearest screen corner),
//! keeping it on-screen, backdrop effects, pinning, click-through and non-activation.

use tauri::webview::PageLoadEvent;
use tauri::window::{Effect, EffectsBuilder};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::settings::{EffectName, Settings, ViewMode};
use crate::state::Shared;

pub const LABEL: &str = "main";
/// Gap to the work-area edge for the default position.
const MARGIN: f64 = 16.0;

/// Logical size of each view.
pub fn logical_size(view: ViewMode) -> (f64, f64) {
    match view {
        ViewMode::Pill => (240.0, 72.0),
        ViewMode::Card => (320.0, 232.0),
        ViewMode::Settings => (320.0, 440.0),
        ViewMode::Sessions => (320.0, 300.0),
        ViewMode::History => (360.0, 380.0),
    }
}

pub fn get(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Creates the (hidden) window; it is shown once the page has loaded, so it never flashes white.
pub fn create(app: &AppHandle, settings: &Settings) -> tauri::Result<WebviewWindow> {
    let (w, h) = logical_size(settings.view);
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
        .title("Claude Usage")
        .inner_size(w, h)
        .decorations(false)
        .transparent(true)
        .always_on_top(settings.pinned)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .shadow(true)
        .focused(false)
        .focusable(settings.view == ViewMode::Settings)
        .visible(false)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Finished {
                let _ = window.show();
                if std::env::var_os("CUW_MEMORY_NORMAL").is_none() {
                    crate::platform::set_memory_low(&window, true);
                }
            }
        });
    if let Some((x, y)) = default_position(app, w, h) {
        builder = builder.position(x, y);
    }
    if let Ok(args) = std::env::var("CUW_BROWSER_ARGS") {
        builder = builder.additional_browser_args(&args);
    }
    let window = builder.build()?;

    // The window-state plugin has restored the saved position by now; a monitor may have gone.
    ensure_on_screen(&window);
    if let (Some(x), Some(y)) = (env_i32("CUW_X"), env_i32("CUW_Y")) {
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    apply_effect(&window, settings.effect, false);
    crate::platform::after_create(&window);

    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::ScaleFactorChanged { .. } = event {
            let view = handle.state::<std::sync::Arc<Shared>>().ui().view;
            resize_anchored(&handle, view, view);
        }
    });
    Ok(window)
}

fn env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok()?.trim().parse().ok()
}

/// Bottom-right of the primary monitor's work area (logical coordinates).
fn default_position(app: &AppHandle, w: f64, h: f64) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let right = (f64::from(area.position.x) + f64::from(area.size.width)) / scale;
    let bottom = (f64::from(area.position.y) + f64::from(area.size.height)) / scale;
    Some((right - w - MARGIN, bottom - h - MARGIN))
}

/// Physical rectangle (x, y, w, h).
type Rect = (i32, i32, i32, i32);

fn window_rect(window: &WebviewWindow) -> Option<Rect> {
    let pos = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some((pos.x, pos.y, size.width as i32, size.height as i32))
}

fn work_areas(window: &WebviewWindow) -> Vec<Rect> {
    window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let a = m.work_area();
            (a.position.x, a.position.y, a.size.width as i32, a.size.height as i32)
        })
        .collect()
}

/// The work area containing the rectangle's centre, else the one it overlaps most.
fn area_for(rect: Rect, areas: &[Rect]) -> Option<Rect> {
    let (cx, cy) = (rect.0 + rect.2 / 2, rect.1 + rect.3 / 2);
    areas
        .iter()
        .copied()
        .find(|a| cx >= a.0 && cx < a.0 + a.2 && cy >= a.1 && cy < a.1 + a.3)
        .or_else(|| areas.iter().copied().max_by_key(|a| overlap(rect, *a)).filter(|a| overlap(rect, *a) > 0))
}

fn overlap(a: Rect, b: Rect) -> i64 {
    let w = (a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0);
    let h = (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1);
    if w <= 0 || h <= 0 { 0 } else { i64::from(w) * i64::from(h) }
}

/// Moves `rect` so it lies fully inside `area` (when it fits).
fn clamp_into(rect: Rect, area: Rect) -> (i32, i32) {
    let x = rect.0.min(area.0 + area.2 - rect.2).max(area.0);
    let y = rect.1.min(area.1 + area.3 - rect.3).max(area.1);
    (x, y)
}

/// New position for a size change that keeps the corner nearest to the screen edges fixed.
/// Which corner stays fixed: (right, bottom).
type Anchor = (bool, bool);

/// The corner nearest to the screen edges.
fn nearest_anchor(rect: Rect, area: Rect) -> Anchor {
    (
        rect.0 + rect.2 / 2 > area.0 + area.2 / 2,
        rect.1 + rect.3 / 2 > area.1 + area.3 / 2,
    )
}

/// The anchor chosen when Settings was opened: the tall Settings view would otherwise pick a
/// different corner and the card would not return to where it was.
static SETTINGS_ANCHOR: std::sync::Mutex<Option<Anchor>> = std::sync::Mutex::new(None);

fn anchored_position_with(rect: Rect, new_w: i32, new_h: i32, area: Rect, (right, bottom): Anchor) -> (i32, i32) {
    let x = if right { rect.0 + rect.2 - new_w } else { rect.0 };
    let y = if bottom { rect.1 + rect.3 - new_h } else { rect.1 };
    clamp_into((x, y, new_w, new_h), area)
}

/// Brings the window back if it is (mostly) off every monitor.
pub fn ensure_on_screen(window: &WebviewWindow) {
    let Some(rect) = window_rect(window) else { return };
    let areas = work_areas(window);
    if areas.is_empty() {
        return;
    }
    let (x, y) = match area_for(rect, &areas) {
        Some(area) => clamp_into(rect, area),
        None => {
            let area = areas[0];
            let margin = (MARGIN * window.scale_factor().unwrap_or(1.0)) as i32;
            (area.0 + area.2 - rect.2 - margin, area.1 + area.3 - rect.3 - margin)
        }
    };
    if (x, y) != (rect.0, rect.1) {
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
}

/// Resizes to the view's size, anchored to the nearest corner, and keeps it on-screen.
/// `from` is the view being left (the anchor chosen on entering Settings is reused on leaving).
pub fn resize_anchored(window: &WebviewWindow, from: ViewMode, view: ViewMode) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let (lw, lh) = logical_size(view);
    let (w, h) = ((lw * scale).round() as i32, (lh * scale).round() as i32);
    let Some(rect) = window_rect(window) else {
        let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
        return;
    };
    // `set_size` sets the inner size; the outer rect also holds the invisible shadow frame.
    let (frame_w, frame_h) = window
        .inner_size()
        .map(|inner| (rect.2 - inner.width as i32, rect.3 - inner.height as i32))
        .unwrap_or((0, 0));
    let areas = work_areas(window);
    let (x, y) = match area_for(rect, &areas) {
        Some(area) => {
            let mut saved = crate::state::lock(&SETTINGS_ANCHOR);
            let anchor = match (from, view) {
                (ViewMode::Settings, ViewMode::Settings) => saved.unwrap_or_else(|| nearest_anchor(rect, area)),
                (ViewMode::Settings, _) => saved.take().unwrap_or_else(|| nearest_anchor(rect, area)),
                (_, ViewMode::Settings) => *saved.insert(nearest_anchor(rect, area)),
                _ => nearest_anchor(rect, area),
            };
            anchored_position_with(rect, w + frame_w, h + frame_h, area, anchor)
        }
        None => (rect.0, rect.1),
    };
    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
}

/// Switches view: size, focusability (only Settings takes keyboard focus).
pub fn set_view(app: &AppHandle, from: ViewMode, view: ViewMode) {
    let Some(window) = get(app) else { return };
    resize_anchored(&window, from, view);
    let settings_view = view == ViewMode::Settings;
    let _ = window.set_focusable(settings_view);
    if settings_view {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Maps the settings effect to a native backdrop. `auto` and `none` use no native effect: the
/// spike showed Mica and Acrylic render flat while the (non-activating) widget is unfocused.
fn native_effect(effect: EffectName) -> Option<Effect> {
    match effect {
        EffectName::Mica => Some(Effect::Mica),
        EffectName::Acrylic => Some(Effect::Acrylic),
        EffectName::Blur => Some(Effect::Blur),
        EffectName::Auto | EffectName::None => None,
    }
}

/// Applies the backdrop; click-through ("ghost") always clears it.
pub fn apply_effect(window: &WebviewWindow, effect: EffectName, ghost: bool) {
    match native_effect(effect).filter(|_| !ghost) {
        Some(e) => {
            let _ = window.set_effects(EffectsBuilder::new().effect(e).build());
        }
        None => {
            let _ = window.set_effects(None);
        }
    }
}

pub fn set_pinned(app: &AppHandle, pinned: bool) {
    if let Some(w) = get(app) {
        let _ = w.set_always_on_top(pinned);
    }
}

pub fn set_click_through(app: &AppHandle, on: bool, effect: EffectName) {
    if let Some(w) = get(app) {
        let _ = w.set_ignore_cursor_events(on);
        apply_effect(&w, effect, on);
    }
}

pub fn show(app: &AppHandle) {
    if let Some(w) = get(app) {
        ensure_on_screen(&w);
        let _ = w.show();
    }
}

/// Emits the current UI state to the webview.
pub fn emit_ui(app: &AppHandle, shared: &Shared) {
    let ui = shared.ui().clone();
    let _ = app.emit("ui-state", &ui);
    crate::tray::sync_checks(app, shared);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchored_position(rect: Rect, new_w: i32, new_h: i32, area: Rect) -> (i32, i32) {
        anchored_position_with(rect, new_w, new_h, area, nearest_anchor(rect, area))
    }

    const AREA: Rect = (0, 0, 1920, 1040);

    #[test]
    fn anchors_to_nearest_corner() {
        // Bottom-right card → pill keeps its bottom-right corner.
        let card = (1584, 792, 320, 232);
        assert_eq!(anchored_position(card, 240, 72, AREA), (1664, 952));
        // Top-left keeps top-left.
        assert_eq!(anchored_position((10, 10, 320, 232), 240, 72, AREA), (10, 10));
        // Growing near the bottom edge stays inside the work area.
        assert_eq!(anchored_position((1600, 960, 240, 72), 320, 440, AREA), (1520, 592));
    }

    #[test]
    fn settings_reuses_the_anchor_it_was_opened_with() {
        // A card at the bottom right opens Settings (bottom-right anchor)…
        let card = (1350, 480, 418, 300);
        let anchor = nearest_anchor(card, AREA);
        assert_eq!(anchor, (true, true));
        let (x, y) = anchored_position_with(card, 418, 560, AREA, anchor);
        assert_eq!((x, y), (1350, 220));
        // …whose own centre is in the upper half, so the nearest corner would be the top one…
        let settings = (x, y, 418, 560);
        assert_eq!(nearest_anchor(settings, AREA), (true, false));
        // …but leaving Settings with the saved anchor returns the card to where it was.
        assert_eq!(anchored_position_with(settings, 418, 300, AREA, anchor), (1350, 480));
    }

    #[test]
    fn finds_area_and_clamps() {
        let second = (1920, 0, 1280, 1000);
        let areas = [AREA, second];
        assert_eq!(area_for((2000, 100, 320, 232), &areas), Some(second));
        assert_eq!(area_for((5000, 5000, 320, 232), &areas), None);
        assert_eq!(clamp_into((1800, 900, 320, 232), AREA), (1600, 808));
        assert_eq!(clamp_into((-50, -50, 320, 232), AREA), (0, 0));
    }

    #[test]
    fn sizes() {
        assert_eq!(logical_size(ViewMode::Pill), (240.0, 72.0));
        assert_eq!(logical_size(ViewMode::Card), (320.0, 232.0));
        assert_eq!(logical_size(ViewMode::Settings), (320.0, 440.0));
    }
}
