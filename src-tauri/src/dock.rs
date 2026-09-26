//! Edge-dock mode: the widget tucks into a thin strip flush against a screen edge and slides
//! out on hover.
//!
//! - The strip is 36×156 on the left or right edge and 232×34 on the top edge (logical px ×
//!   `ui_scale`), inside the work area of the monitor the widget is on, so never under the
//!   taskbar. Docking keeps the widget's position along the edge (centre-aligned).
//! - Slid out, the current view sits flush against the same edge, centred on the strip. A view
//!   shorter than the strip (the pill on a side edge) is centred on the cursor instead, so the
//!   pointer that slid it out is always over it and it can't bounce straight back in.
//! - Only the pill and the card slide back in; Settings, Sessions and History stay out until
//!   the user returns to the card. Click-through never slides out.
//! - Dragging: the strip itself can't be dragged (pointing at it slides the widget out). The
//!   slid-out widget drags freely, and when it slides back in it snaps flush to the edge at
//!   its new position along it (on whichever monitor it was dropped). A drag therefore moves
//!   the dock along the edge; it never undocks. Until then it stays where it was dropped:
//!   showing the window only keeps it on-screen, and a DPI change (dragged onto another
//!   monitor) only resizes it.
//! - Nothing extra is persisted: the window-state plugin saves the strip's position on quit (a
//!   slid-out widget slides in first).

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, State, WebviewWindow};

use crate::settings::{DockEdge, Settings, ViewMode};
use crate::state::{Shared, lock};
use crate::window::{Rect, area_for, physical, set_visible_rect, view_size, visible_rect, work_areas};

/// The edge a docked widget sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
}

impl Side {
    pub fn from_edge(edge: DockEdge) -> Option<Self> {
        match edge {
            DockEdge::Off => None,
            DockEdge::Left => Some(Self::Left),
            DockEdge::Right => Some(Self::Right),
            DockEdge::Top => Some(Self::Top),
        }
    }

    /// Collapsed strip size in logical px at `ui_scale` 1.
    pub fn strip_logical(self) -> (f64, f64) {
        match self {
            Self::Left | Self::Right => (36.0, 156.0),
            Self::Top => (232.0, 34.0),
        }
    }

    /// Start and length of `r` along the edge.
    fn span(self, r: Rect) -> (i32, i32) {
        match self {
            Self::Left | Self::Right => (r.1, r.3),
            Self::Top => (r.0, r.2),
        }
    }

    /// The component of a point or a size that runs along the edge.
    fn along(self, (x, y): (i32, i32)) -> i32 {
        match self {
            Self::Left | Self::Right => y,
            Self::Top => x,
        }
    }
}

/// Where the widget is, for what happens to it outside a slide (showing it, a DPI change, quit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockState {
    Undocked,
    /// The strip, flush against the edge.
    Strip,
    /// Slid out, and possibly dragged since.
    SlidOut,
}

impl DockState {
    pub fn of(edge: DockEdge, expanded: bool) -> Self {
        match (edge, expanded) {
            (DockEdge::Off, _) => Self::Undocked,
            (_, false) => Self::Strip,
            (_, true) => Self::SlidOut,
        }
    }

    pub fn now(shared: &Shared) -> Self {
        let edge = shared.settings().dock;
        let expanded = shared.ui().dock_expanded;
        Self::of(edge, expanded)
    }
}

/// Only these views slide back into the strip.
pub fn collapsible(view: ViewMode) -> bool {
    matches!(view, ViewMode::Pill | ViewMode::Card)
}

/// Collapsed strip size in logical px at a UI scale.
pub fn strip_scaled(side: Side, ui_scale: f32) -> (f64, f64) {
    let (w, h) = side.strip_logical();
    let s = f64::from(ui_scale);
    (w * s, h * s)
}

/// Collapsed strip size in physical px for a UI scale and a monitor scale factor.
pub fn strip_size(side: Side, ui_scale: f32, dpi_scale: f64) -> (i32, i32) {
    physical(strip_scaled(side, ui_scale), dpi_scale)
}

/// Keeps `start..start + len` inside `lo..lo + span` (pinned to `lo` when it can't fit).
fn fit(start: i32, len: i32, lo: i32, span: i32) -> i32 {
    start.min(lo + span - len).max(lo)
}

/// A `(w, h)` rectangle flush against `side` of `area`, centred on `center` along the edge
/// and kept inside the area.
pub fn flush_rect(side: Side, area: Rect, (w, h): (i32, i32), center: i32) -> Rect {
    match side {
        Side::Left => (area.0, fit(center - h / 2, h, area.1, area.3), w, h),
        Side::Right => (area.0 + area.2 - w, fit(center - h / 2, h, area.1, area.3), w, h),
        Side::Top => (fit(center - w / 2, w, area.0, area.2), area.1, w, h),
    }
}

fn center_along(side: Side, r: Rect) -> i32 {
    let (start, len) = side.span(r);
    start + len / 2
}

/// The strip for a window whose visible rectangle is `from` (same centre along the edge).
pub fn strip_rect(side: Side, area: Rect, size: (i32, i32), from: Rect) -> Rect {
    flush_rect(side, area, size, center_along(side, from))
}

fn contains((x, y, w, h): Rect, (px, py): (i32, i32)) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// The slid-out view of `size` for `strip`, the window currently being `current`: centred on
/// the strip; a view shorter than the strip along the edge is centred on the cursor instead
/// while the cursor is over the widget (hovering the strip, or switching the card to the pill
/// from its right-click menu), so it opens under the pointer and isn't left straight away.
pub fn expanded_rect(side: Side, area: Rect, size: (i32, i32), strip: Rect, current: Rect, cursor: Option<(i32, i32)>) -> Rect {
    let (start, len) = side.span(strip);
    let view_len = side.along(size);
    let center = cursor
        .filter(|&p| view_len < len && (contains(current, p) || contains(strip, p)))
        .map_or(start + len / 2, |p| side.along(p));
    flush_rect(side, area, size, center)
}

/// Where the dock last put the window, to tell a user's drag from its own placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    side: Side,
    strip: Rect,
    placed: Rect,
}

static LAST: Mutex<Option<Placement>> = Mutex::new(None);

/// The strip to collapse to or expand from: the remembered one while the window is still
/// where the dock put it (so expand → collapse round-trips even where clamping moved the
/// view), else one derived from where the user moved it.
pub fn current_strip(last: Option<Placement>, side: Side, area: Rect, size: (i32, i32), visible: Rect) -> Rect {
    match last {
        Some(p) if p.side == side && p.placed == visible => strip_rect(side, area, size, p.strip),
        _ => strip_rect(side, area, size, visible),
    }
}

/// Forgets the remembered strip (the dock edge changed or docking was turned off).
pub fn forget() {
    *lock(&LAST) = None;
}

/// Sizes and places the window for the dock state: the strip, or `view` slid out.
pub fn place(window: &WebviewWindow, side: Side, settings: &Settings, view: ViewMode, expanded: bool) {
    let Some((visible, inset)) = visible_rect(window) else { return };
    let areas = work_areas(window);
    let Some(area) = area_for(visible, &areas).or_else(|| areas.first().copied()) else { return };
    let dpi = window.scale_factor().unwrap_or(1.0);
    let mut last = lock(&LAST);
    let strip = current_strip(*last, side, area, strip_size(side, settings.ui_scale, dpi), visible);
    let target = if expanded {
        let cursor = window
            .cursor_position()
            .ok()
            .map(|p| (p.x.round() as i32, p.y.round() as i32));
        expanded_rect(side, area, physical(view_size(view, settings), dpi), strip, visible, cursor)
    } else {
        strip
    };
    *last = Some(Placement {
        side,
        strip,
        placed: target,
    });
    drop(last);
    set_visible_rect(window, target, inset);
}

/// Whether a slide request changes anything: click-through never slides out, and only the
/// pill and the card slide back in.
pub fn should_change(current: bool, expanded: bool, click_through: bool, view: ViewMode) -> bool {
    expanded != current && if expanded { !click_through } else { collapsible(view) }
}

fn cursor_inside(window: &WebviewWindow) -> bool {
    let (Some((rect, _)), Ok(p)) = (visible_rect(window), window.cursor_position()) else {
        return false;
    };
    contains(rect, (p.x.floor() as i32, p.y.floor() as i32))
}

/// Slides the docked widget out or back in; updates `UiState.dock_expanded` and emits it.
/// `force` is the user's own request (the card's "–"), made with the cursor over the widget.
pub fn set_expanded(app: &AppHandle, shared: &Shared, expanded: bool, force: bool) {
    let Some(side) = Side::from_edge(shared.settings().dock) else { return };
    let ui = shared.ui().clone();
    if !should_change(ui.dock_expanded, expanded, ui.click_through, ui.view) {
        return;
    }
    let Some(window) = crate::window::get(app) else { return };
    // The webview can report a leave while the cursor is still over the window (e.g. around a
    // resize); that is not a leave.
    if !expanded && !force && cursor_inside(&window) {
        return;
    }
    shared.ui().dock_expanded = expanded;
    let settings = shared.settings().clone();
    place(&window, side, &settings, ui.view, expanded);
    crate::window::emit_ui(app, shared);
}

/// Slides a slid-out widget (any view) back into its strip before the window-state plugin saves
/// the position: the next start recreates the strip where the window was saved.
pub fn collapse_before_save(app: &AppHandle, shared: &Shared) {
    let Some(side) = Side::from_edge(shared.settings().dock) else { return };
    if DockState::now(shared) != DockState::SlidOut {
        return;
    }
    let Some(window) = crate::window::get(app) else { return };
    let view = {
        let mut ui = shared.ui();
        ui.dock_expanded = false;
        ui.view
    };
    let settings = shared.settings().clone();
    place(&window, side, &settings, view, false);
}

/// Called by the UI on pointer enter/leave of the docked widget, and with `force` by the card's
/// "–" button.
#[tauri::command]
pub fn set_dock_expanded(app: AppHandle, shared: State<'_, Arc<Shared>>, expanded: bool, force: Option<bool>) {
    set_expanded(&app, &shared, expanded, force.unwrap_or(false));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second monitor right of a 1920×1080 primary, 1280×1000 work area above a 40 px taskbar.
    const AREA: Rect = (1920, 0, 1280, 1000);

    #[test]
    fn strip_sizes_scale_with_ui_and_dpi() {
        assert_eq!(strip_size(Side::Left, 1.0, 1.0), (36, 156));
        assert_eq!(strip_size(Side::Top, 1.0, 1.0), (232, 34));
        assert_eq!(strip_size(Side::Right, 1.3, 1.0), (47, 203));
        assert_eq!(strip_size(Side::Right, 1.0, 1.5), (54, 234));
        assert_eq!(strip_size(Side::Top, 0.85, 1.25), (247, 36));
        assert_eq!(Side::from_edge(DockEdge::Off), None);
        assert_eq!(Side::from_edge(DockEdge::Top), Some(Side::Top));
    }

    #[test]
    fn strips_sit_flush_against_each_edge_of_the_work_area() {
        // A card on the second monitor, its centre at y = 516 / x = 2560.
        let card = (2400, 400, 320, 232);
        assert_eq!(strip_rect(Side::Left, AREA, (36, 156), card), (1920, 438, 36, 156));
        assert_eq!(strip_rect(Side::Right, AREA, (36, 156), card), (3164, 438, 36, 156));
        assert_eq!(strip_rect(Side::Top, AREA, (232, 34), card), (2444, 0, 232, 34));
        // Near the taskbar the strip stays above the work area's bottom (1000), not the screen's.
        let low = (2400, 900, 320, 232);
        assert_eq!(strip_rect(Side::Left, AREA, (36, 156), low), (1920, 844, 36, 156));
        // A work area that doesn't start at 0 (taskbar on top): the top strip sits below it.
        let below_bar = (0, 48, 1920, 1032);
        assert_eq!(strip_rect(Side::Top, below_bar, (232, 34), (10, 300, 320, 232)), (54, 48, 232, 34));
        // Larger than the area: pinned to its start, never panics.
        assert_eq!(flush_rect(Side::Left, (0, 0, 100, 100), (36, 156), 50), (0, 0, 36, 156));
    }

    #[test]
    fn expanded_views_cover_the_strip() {
        let strip = (1920, 438, 36, 156);
        // The card (taller than the strip) is centred on it and flush with the edge.
        let card = expanded_rect(Side::Left, AREA, (320, 232), strip, strip, Some((1930, 450)));
        assert_eq!(card, (1920, 400, 320, 232));
        assert!(card.1 <= strip.1 && card.1 + card.3 >= strip.1 + strip.3);
        let right_strip = (3164, 438, 36, 156);
        let right = expanded_rect(Side::Right, AREA, (320, 232), right_strip, right_strip, None);
        assert_eq!(right, (2880, 400, 320, 232));
        let top_strip = (2444, 0, 232, 34);
        let top = expanded_rect(Side::Top, AREA, (240, 72), top_strip, top_strip, Some((2450, 5)));
        assert_eq!(top, (2440, 0, 240, 72));
        // Clamped at the area's end, it still covers the strip.
        let low_strip = (1920, 844, 36, 156);
        let low = expanded_rect(Side::Left, AREA, (320, 232), low_strip, low_strip, None);
        assert_eq!(low, (1920, 768, 320, 232));
    }

    #[test]
    fn a_view_shorter_than_the_strip_opens_under_the_cursor() {
        let strip = (1920, 438, 36, 156);
        let pill = |current: Rect, cursor| expanded_rect(Side::Left, AREA, (240, 72), strip, current, cursor);
        // Pointing at the strip: the pill is centred on the pointer.
        assert_eq!(pill(strip, Some((1925, 450))), (1920, 414, 240, 72));
        assert_eq!(pill(strip, Some((1925, 520))), (1920, 484, 240, 72));
        // "Compact" picked from the card's right-click menu, below the strip: a pill under the pointer.
        let card = (1920, 400, 320, 232);
        let from_card = pill(card, Some((2100, 612)));
        assert_eq!(from_card, (1920, 576, 240, 72));
        assert!(contains(from_card, (2100, 612)));
        // Pointer elsewhere (switched from the tray) or unknown: centred on the strip.
        assert_eq!(pill(card, Some((3000, 990))), (1920, 480, 240, 72));
        assert_eq!(pill(strip, None), (1920, 480, 240, 72));
        // Near the area's top the pill stays inside it.
        let high = (1920, 0, 36, 156);
        assert_eq!(expanded_rect(Side::Left, AREA, (240, 72), high, high, Some((1925, 5))), (1920, 0, 240, 72));
    }

    #[test]
    fn expand_then_collapse_round_trips_unless_moved() {
        let strip = (1920, 0, 36, 156);
        // At the top of the area the card can't be centred on the strip…
        let card = expanded_rect(Side::Left, AREA, (320, 232), strip, strip, None);
        assert_eq!(card, (1920, 0, 320, 232));
        let last = Some(Placement {
            side: Side::Left,
            strip,
            placed: card,
        });
        // …yet collapsing returns to the same strip, not one centred on the card.
        assert_eq!(current_strip(last, Side::Left, AREA, (36, 156), card), strip);
        assert_eq!(strip_rect(Side::Left, AREA, (36, 156), card), (1920, 38, 36, 156));
        // Dragged down by the user: the strip follows to the new position.
        let dragged = (1990, 600, 320, 232);
        assert_eq!(current_strip(last, Side::Left, AREA, (36, 156), dragged), (1920, 638, 36, 156));
        // A new UI scale resizes the remembered strip about its centre.
        assert_eq!(current_strip(last, Side::Left, AREA, (47, 203), card), (1920, 0, 47, 203));
        // Another edge starts from the window, not from the old edge's strip.
        assert_eq!(current_strip(last, Side::Top, AREA, (232, 34), card), (1964, 0, 232, 34));
    }

    #[test]
    fn dock_states() {
        assert_eq!(DockState::of(DockEdge::Off, false), DockState::Undocked);
        assert_eq!(DockState::of(DockEdge::Off, true), DockState::Undocked, "stale flag");
        assert_eq!(DockState::of(DockEdge::Left, false), DockState::Strip);
        assert_eq!(DockState::of(DockEdge::Right, true), DockState::SlidOut);
        assert_eq!(DockState::of(DockEdge::Top, true), DockState::SlidOut);
    }

    #[test]
    fn quitting_slid_out_saves_the_strip_the_next_start_recreates() {
        let strip = (1920, 438, 36, 156);
        let card = expanded_rect(Side::Left, AREA, (320, 232), strip, strip, None);
        let last = Some(Placement {
            side: Side::Left,
            strip,
            placed: card,
        });
        // Saved slid out, the next start creates the strip at the card's origin, and centring
        // it there moves it.
        let restored = (card.0, card.1, 36, 156);
        assert_ne!(strip_rect(Side::Left, AREA, (36, 156), restored), strip);
        // Slid in first, the saved position is the strip's, and the next start keeps it.
        let saved = current_strip(last, Side::Left, AREA, (36, 156), card);
        assert_eq!(saved, strip);
        assert_eq!(strip_rect(Side::Left, AREA, (36, 156), saved), strip);
        // Dragged before quitting: the strip saved is the one it would slide into.
        let dragged = (1990, 600, 320, 232);
        assert_eq!(current_strip(last, Side::Left, AREA, (36, 156), dragged), (1920, 638, 36, 156));
    }

    #[test]
    fn slide_rules() {
        assert!(should_change(false, true, false, ViewMode::Pill));
        assert!(!should_change(true, true, false, ViewMode::Card), "already out");
        assert!(!should_change(false, true, true, ViewMode::Card), "click-through never slides out");
        assert!(should_change(true, false, false, ViewMode::Card));
        assert!(should_change(true, false, true, ViewMode::Pill), "sliding in is always allowed");
        for view in [ViewMode::Settings, ViewMode::Sessions, ViewMode::History] {
            assert!(!should_change(true, false, false, view), "{view:?} stays out");
        }
    }
}
