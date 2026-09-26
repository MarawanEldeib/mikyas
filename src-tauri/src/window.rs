//! The single widget window: creation, view sizes (scaled by `ui_scale` and anchored to the
//! nearest screen corner), keeping it on-screen, edge docking (geometry in `dock.rs`), backdrop
//! effects, pinning, click-through and non-activation.

use std::sync::Arc;

use tauri::webview::PageLoadEvent;
use tauri::window::{Effect, EffectsBuilder};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::dock::{self, Side};
use crate::settings::{CardRows, DockEdge, EffectName, Settings, ViewMode};
use crate::state::Shared;

pub const LABEL: &str = "main";
/// Gap to the work-area edge for the default position.
const MARGIN: f64 = 16.0;

/// Logical size of each view at `ui_scale` 1 (the card with every row shown).
pub fn logical_size(view: ViewMode) -> (f64, f64) {
    match view {
        ViewMode::Pill => (240.0, 72.0),
        ViewMode::Card => (320.0, 232.0),
        ViewMode::Settings => (320.0, 440.0),
        ViewMode::Sessions => (320.0, 300.0),
        ViewMode::History => (360.0, 380.0),
    }
}

// Optional card rows, in logical px at scale 1: what hiding each row removes from the card's
// layout in Card.svelte / WindowSection.svelte (the browser mock mirrors them in app.css).
// Sparklines sit beside the figures and the source badges share the footer with the buttons,
// so hiding either frees width, not height.
/// The burn-forecast line (14 px + 4 px gap) in each of the two window sections.
const CARD_BURN_H: f64 = 2.0 * 18.0;
/// The session header (22 px) and the gap below it.
const CARD_SESSION_H: f64 = 30.0;
/// Floor for any row combination: the empty state needs about 110 px of body.
const CARD_MIN_H: f64 = 160.0;

/// Card height for the rows the user shows (logical px at scale 1).
pub fn card_height(rows: &CardRows) -> f64 {
    let mut h = logical_size(ViewMode::Card).1;
    if !rows.burn {
        h -= CARD_BURN_H;
    }
    if !rows.session {
        h -= CARD_SESSION_H;
    }
    h.max(CARD_MIN_H)
}

/// Logical window size of a view with the user's card rows and UI scale applied.
pub fn view_size(view: ViewMode, settings: &Settings) -> (f64, f64) {
    let (w, h) = logical_size(view);
    let h = if view == ViewMode::Card { card_height(&settings.card_rows) } else { h };
    let s = f64::from(settings.ui_scale);
    (w * s, h * s)
}

/// Logical → physical px (at least 1).
pub fn physical((w, h): (f64, f64), scale: f64) -> (i32, i32) {
    (((w * scale).round() as i32).max(1), ((h * scale).round() as i32).max(1))
}

pub fn get(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Creates the (hidden) window; it is shown once the page has loaded, so it never flashes white.
pub fn create(app: &AppHandle, settings: &Settings) -> tauri::Result<WebviewWindow> {
    // A docked widget starts as the strip, so the restored position (saved while it was the
    // strip) maps back onto the same strip.
    let (w, h) = match Side::from_edge(settings.dock) {
        Some(side) => dock::strip_scaled(side, settings.ui_scale),
        None => view_size(settings.view, settings),
    };
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
    // (This also snaps a docked widget flush to its edge.)
    ensure_on_screen(&window);
    if let (Some(x), Some(y)) = (env_i32("CUW_X"), env_i32("CUW_Y")) {
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    apply_effect(&window, settings.effect, false);
    crate::platform::after_create(&window);

    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::ScaleFactorChanged { .. } = event {
            let view = handle.state::<Arc<Shared>>().ui().view;
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
pub type Rect = (i32, i32, i32, i32);

fn window_rect(window: &WebviewWindow) -> Option<Rect> {
    let pos = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    Some((pos.x, pos.y, size.width as i32, size.height as i32))
}

/// The visible (client) rectangle, and the offset of the outer frame's origin from it.
pub fn visible_rect(window: &WebviewWindow) -> Option<(Rect, (i32, i32))> {
    let outer = window.outer_position().ok()?;
    let inner = window.inner_position().ok()?;
    let size = window.inner_size().ok()?;
    let rect = (inner.x, inner.y, size.width as i32, size.height as i32);
    Some((rect, (inner.x - outer.x, inner.y - outer.y)))
}

/// Moves and sizes the window so that its visible part is `rect` (a flush placement must not
/// leave an invisible frame as a gap).
pub fn set_visible_rect(window: &WebviewWindow, (x, y, w, h): Rect, (dx, dy): (i32, i32)) {
    let _ = window.set_position(PhysicalPosition::new(x - dx, y - dy));
    let _ = window.set_size(PhysicalSize::new(w.max(1) as u32, h.max(1) as u32));
}

pub fn work_areas(window: &WebviewWindow) -> Vec<Rect> {
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
pub fn area_for(rect: Rect, areas: &[Rect]) -> Option<Rect> {
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

fn shared_of(window: &WebviewWindow) -> Option<Arc<Shared>> {
    window.try_state::<Arc<Shared>>().map(|s| s.inner().clone())
}

/// Brings the window back if it is (mostly) off every monitor; a docked one is re-snapped.
pub fn ensure_on_screen(window: &WebviewWindow) {
    if let Some(shared) = shared_of(window) {
        if shared.settings().dock != DockEdge::Off {
            let view = shared.ui().view;
            resize_anchored(window, view, view);
            return;
        }
    }
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

/// Sizes and places the window for `view` at the user's scale and card rows. Docked: the strip,
/// or the view slid out flush to the edge. Otherwise: the view anchored to the nearest corner
/// and kept on-screen. Every resize goes through here. `from` is the view being left (the
/// anchor chosen on entering Settings is reused on leaving).
pub fn resize_anchored(window: &WebviewWindow, from: ViewMode, view: ViewMode) {
    let Some(shared) = shared_of(window) else {
        resize_free(window, from, view, logical_size(view));
        return;
    };
    let settings = shared.settings().clone();
    match Side::from_edge(settings.dock) {
        Some(side) => {
            let expanded = shared.ui().dock_expanded;
            dock::place(window, side, &settings, view, expanded);
        }
        None => resize_free(window, from, view, view_size(view, &settings)),
    }
}

fn resize_free(window: &WebviewWindow, from: ViewMode, view: ViewMode, logical: (f64, f64)) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let (w, h) = physical(logical, scale);
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

/// Switches view: size, focusability (only Settings takes keyboard focus). A docked strip
/// slides out for Settings, Sessions and History (which never slide back in).
pub fn set_view(app: &AppHandle, from: ViewMode, view: ViewMode) {
    let Some(window) = get(app) else { return };
    if let Some(shared) = shared_of(&window) {
        let docked = shared.settings().dock != DockEdge::Off;
        if docked && !dock::collapsible(view) {
            shared.ui().dock_expanded = true;
        }
    }
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
    let Some(w) = get(app) else { return };
    let _ = w.set_ignore_cursor_events(on);
    apply_effect(&w, effect, on);
    // A slid-out dock could never be left again while it ignores the pointer.
    let Some(shared) = shared_of(&w).filter(|_| on) else { return };
    let view = {
        let mut ui = shared.ui();
        if !(ui.dock_expanded && dock::collapsible(ui.view)) {
            return;
        }
        ui.dock_expanded = false;
        ui.view
    };
    resize_anchored(&w, view, view);
}

/// Applies the appearance settings that change the window (the `apply_settings_patch` hook,
/// which emits `ui-state` afterwards).
pub fn on_settings_changed(app: &AppHandle, shared: &Shared, old: &Settings, new: &Settings) {
    let Some(window) = get(app) else { return };
    let view = shared.ui().view;
    let dock_changed = new.dock != old.dock;
    if dock_changed {
        // Docking is switched on from Settings, which never slides in: it stays out, flush to
        // the edge, until the user goes back to the card.
        shared.ui().dock_expanded = new.dock != DockEdge::Off && !dock::collapsible(view);
        dock::forget();
        // Docked placement ignores the Settings anchor; one saved before docking is stale.
        *crate::state::lock(&SETTINGS_ANCHOR) = None;
    }
    let rows_changed = new.card_rows != old.card_rows && view == ViewMode::Card;
    if dock_changed || rows_changed || new.ui_scale != old.ui_scale {
        resize_anchored(&window, view, view);
    }
}

pub fn toggle_visible(app: &AppHandle) {
    let Some(w) = get(app) else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
    } else {
        show(app);
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

    fn rows(sparklines: bool, burn: bool, session: bool, sources: bool) -> CardRows {
        CardRows {
            sparklines,
            burn,
            session,
            sources,
        }
    }

    #[test]
    fn card_height_follows_the_visible_rows() {
        assert_eq!(card_height(&CardRows::default()), 232.0);
        for bits in 0..16u8 {
            let r = rows(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let expected = match (r.burn, r.session) {
                (true, true) => 232.0,
                (false, true) => 196.0,
                (true, false) => 202.0,
                (false, false) => 166.0,
            };
            assert_eq!(card_height(&r), expected, "{r:?}");
            assert!(card_height(&r) >= CARD_MIN_H);
        }
    }

    #[test]
    fn view_size_applies_scale_and_rows() {
        let close = |(w, h): (f64, f64), (ew, eh): (f64, f64)| (w - ew).abs() < 1e-3 && (h - eh).abs() < 1e-3;
        let mut s = Settings::default();
        assert!(close(view_size(ViewMode::Card, &s), (320.0, 232.0)));
        s.card_rows = rows(false, false, true, false);
        assert!(close(view_size(ViewMode::Card, &s), (320.0, 196.0)));
        assert!(close(view_size(ViewMode::Pill, &s), (240.0, 72.0)), "rows only shape the card");
        s.ui_scale = 1.15;
        assert!(close(view_size(ViewMode::Card, &s), (368.0, 225.4)));
        assert!(close(view_size(ViewMode::History, &s), (414.0, 437.0)));
        s.ui_scale = 0.85;
        assert!(close(view_size(ViewMode::Pill, &s), (204.0, 61.2)));
        // At 150 % DPI the physical size is rounded once, from the scaled logical size.
        assert_eq!(physical(view_size(ViewMode::Pill, &s), 1.5), (306, 92));
        assert_eq!(physical((0.0, -3.0), 1.0), (1, 1));
    }
}
