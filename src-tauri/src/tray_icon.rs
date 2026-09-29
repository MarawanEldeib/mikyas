//! Tray icon with the live % drawn into it.
//!
//! - `settings.tray_number` picks the value: `worst` (the highest of every limit Claude reports,
//!   so a model-specific or new limit that binds shows too), any window key (`five_hour`,
//!   `seven_day_opus`, ...; one not present right now falls back to `worst`), or `off` (the
//!   coloured dot icons in `tray.rs`).
//! - Digits are drawn as strokes (seven-segment shapes, 4×4 supersampled) at 16/20/24/32 px for
//!   DPI 100–200 %, coloured by level (the shared usage bands, [`Level::for_pct`]) in shades
//!   readable on a light or dark taskbar (registry `SystemUsesLightTheme`). 100 % is a lock, not
//!   "100"; stale values are grey. No data at all → `None` (the grey dot).
//! - [`tray_values`] is the one reading of the snapshot behind the dot's level, the tooltip and
//!   the number, so all three apply the same reset, limit-reached and clamp rules.

use mikyas_core::engine::types::{Phase, Snapshot, WindowKind, main_kinds};
use mikyas_core::level::{UsageLevel, display_pct};
use tauri::image::Image;

use crate::settings::TrayNumber;

/// Tray colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Green,
    Orange,
    Red,
    Grey,
}

impl Level {
    /// The shared usage bands ([`mikyas_core::level`]), judged by the shown (rounded) number.
    pub fn for_pct(pct: f32) -> Self {
        match UsageLevel::for_pct(pct) {
            UsageLevel::Ok => Self::Green,
            UsageLevel::Warn => Self::Orange,
            UsageLevel::Crit => Self::Red,
        }
    }
}

/// One usage window as the tray shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayValue {
    pub kind: WindowKind,
    /// One of the main windows (`main_kinds` of the snapshot's windows).
    pub main: bool,
    /// Rounded %, 0–100: 0 while a reset awaits data, 100 once the limit is reached.
    pub pct: u8,
    pub stale: bool,
}

impl TrayValue {
    /// The colour for this value (stale → grey).
    pub fn level(&self) -> Level {
        if self.stale { Level::Grey } else { Level::for_pct(f32::from(self.pct)) }
    }
}

/// Every window of the snapshot with the tray's rules applied, main ones first.
pub fn tray_values(snapshot: &Snapshot) -> impl Iterator<Item = TrayValue> + '_ {
    let main = main_kinds(snapshot.windows.iter().map(|w| &w.state.kind));
    let mut values: Vec<TrayValue> = snapshot
        .windows
        .iter()
        .map(|w| {
            let awaiting = w.state.phase == Phase::ResetAwaitingData;
            let pct = if awaiting { 0.0 } else { w.state.pct };
            let reached = w.state.limit_reached && !awaiting;
            let rounded = display_pct(pct);
            TrayValue {
                kind: w.state.kind.clone(),
                main: main.contains(&w.state.kind),
                pct: if reached { 100 } else { rounded },
                stale: w.state.stale,
            }
        })
        .collect();
    values.sort_by_key(|v| !v.main); // stable: snapshot order within each group
    values.into_iter()
}

/// Taskbar colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

/// How the icon is drawn: pixel size and taskbar theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub size: u32,
    pub theme: Theme,
}

impl Style {
    /// The tray icon size Windows uses at a display scale (`SM_CXSMICON`): 16 px at 100 %,
    /// 20 at 125 %, 24 at 150 %, 32 at 200 % (and between).
    pub fn for_scale(scale: f64, theme: Theme) -> Self {
        let size = if scale <= 1.0 {
            16
        } else if scale <= 1.25 {
            20
        } else if scale <= 1.5 {
            24
        } else {
            32
        };
        Self { size, theme }
    }

    /// The primary monitor's scale (the taskbar with the tray lives there) and the taskbar theme.
    pub fn current(app: &tauri::AppHandle) -> Self {
        let scale = app.primary_monitor().ok().flatten().map_or(1.0, |m| m.scale_factor());
        Self::for_scale(scale, taskbar_theme())
    }
}

/// The value the icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// Rounded %, 0–100 (100 = limit reached).
    pub pct: u8,
    pub level: Level,
}

/// The reading for `mode`: the chosen window, or for `worst` (and a chosen window that is not
/// present) every window; fresh ones first, the highest wins. Only stale values → the highest of
/// them, grey. Nothing → `None`.
pub fn reading(snapshot: &Snapshot, mode: &TrayNumber) -> Option<Reading> {
    if mode.is_off() {
        return None;
    }
    let all: Vec<TrayValue> = tray_values(snapshot).collect();
    let chosen = mode.window().filter(|k| all.iter().any(|v| v.kind == *k));
    let values: Vec<TrayValue> = all.into_iter().filter(|v| chosen.as_ref().is_none_or(|k| v.kind == *k)).collect();
    let fresh: Vec<&TrayValue> = values.iter().filter(|v| !v.stale).collect();
    let pool = if fresh.is_empty() { values.iter().collect() } else { fresh };
    let v = pool.into_iter().max_by_key(|v| v.pct)?;
    Some(Reading { pct: v.pct, level: v.level() })
}

/// The number icon for `snapshot`, or `None` for the dot icons.
pub fn number_icon(snapshot: &Snapshot, mode: &TrayNumber, style: Style) -> Option<Image<'static>> {
    let r = reading(snapshot, mode)?;
    Some(Image::new_owned(render(r, style), style.size, style.size))
}

/// Level colour in shades that read on the taskbar: the UI's fill colours on a dark one, its
/// darker text colours on a light one (`app.css`).
pub fn color(level: Level, theme: Theme) -> [u8; 3] {
    match (theme, level) {
        (Theme::Dark, Level::Green) => [0x6c, 0xcb, 0x5f],
        (Theme::Dark, Level::Orange) => [0xff, 0xa5, 0x3d],
        (Theme::Dark, Level::Red) => [0xff, 0x6b, 0x6b],
        (Theme::Dark, Level::Grey) => [0xa0, 0xa0, 0xa0],
        (Theme::Light, Level::Green) => [0x0b, 0x6b, 0x0b],
        (Theme::Light, Level::Orange) => [0x8a, 0x4b, 0x00],
        (Theme::Light, Level::Red) => [0xb0, 0x1e, 0x14],
        (Theme::Light, Level::Grey) => [0x6e, 0x6e, 0x6e],
    }
}

type Pt = (f64, f64);

/// Seven-segment digit strokes in a unit box (y down): polylines through the corners.
fn digit_strokes(d: u8) -> Vec<Vec<Pt>> {
    const TL: Pt = (0.0, 0.0);
    const TR: Pt = (1.0, 0.0);
    const ML: Pt = (0.0, 0.5);
    const MR: Pt = (1.0, 0.5);
    const BL: Pt = (0.0, 1.0);
    const BR: Pt = (1.0, 1.0);
    match d {
        0 => vec![vec![TL, TR, BR, BL, TL]],
        1 => vec![vec![(0.25, 0.15), (0.6, 0.0), (0.6, 1.0)]],
        2 => vec![vec![TL, TR, MR, ML, BL, BR]],
        3 => vec![vec![TL, TR, BR, BL], vec![ML, MR]],
        4 => vec![vec![TL, ML, MR], vec![TR, BR]],
        5 => vec![vec![TR, TL, ML, MR, BR, BL]],
        6 => vec![vec![TR, TL, BL, BR, MR, ML]],
        7 => vec![vec![TL, TR, BR]],
        8 => vec![vec![TL, TR, BR, BL, TL], vec![ML, MR]],
        _ => vec![vec![BL, BR, TR, TL, ML, MR]],
    }
}

/// Is `p` on the segment `a`–`b` stroked `2 × half` wide with square caps (so the corners of
/// the digits are square and the 16 px strokes fill whole pixels)?
fn on_segment(p: Pt, a: Pt, b: Pt, half: f64) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    let (px, py) = (p.0 - a.0, p.1 - a.1);
    if len == 0.0 {
        return px.abs() <= half && py.abs() <= half;
    }
    let along = (px * dx + py * dy) / len;
    let across = (px * dy - py * dx).abs() / len;
    (-half..=len + half).contains(&along) && across <= half
}

/// Everything drawn, in pixel coordinates.
struct Shapes {
    /// Polylines stroked with `half` (half the stroke width).
    lines: Vec<Vec<Pt>>,
    half: f64,
    /// Filled rectangles (x0, y0, x1, y1).
    fills: Vec<(f64, f64, f64, f64)>,
}

impl Shapes {
    fn covers(&self, p: Pt) -> bool {
        self.fills.iter().any(|&(x0, y0, x1, y1)| p.0 >= x0 && p.0 < x1 && p.1 >= y0 && p.1 < y1)
            || self.lines.iter().any(|l| l.windows(2).any(|s| on_segment(p, s[0], s[1], self.half)))
    }
}

/// Stroke width, vertical margin and digit-box width for an icon size (whole pixels, so the
/// strokes of the 16 px icon land on the pixel grid).
fn metrics(size: u32) -> (f64, f64, f64) {
    let s = f64::from(size);
    let stroke = (s / 8.0).round();
    let margin = (s / 8.0).round();
    let width = ((s - stroke) / 2.0).floor();
    (stroke, margin, width)
}

fn layout(r: Reading, size: u32) -> Shapes {
    let s = f64::from(size);
    let (stroke, margin, width) = metrics(size);
    let half = stroke / 2.0;
    if r.pct >= 100 {
        // A lock: a filled body and a stroked shackle.
        let (x0, x1) = ((s * 0.2).round(), (s * 0.8).round());
        let body_top = (s * 0.46).round();
        let shackle_x = (x0 + half + s * 0.06, x1 - half - s * 0.06);
        let top = margin + half;
        let mid = (shackle_x.0 + shackle_x.1) / 2.0;
        let shackle = vec![
            (shackle_x.0, body_top),
            (shackle_x.0, top + s * 0.1),
            (mid, top),
            (shackle_x.1, top + s * 0.1),
            (shackle_x.1, body_top),
        ];
        return Shapes { lines: vec![shackle], half, fills: vec![(x0, body_top, x1, s - margin)] };
    }
    let digits: Vec<u8> = if r.pct >= 10 { vec![r.pct / 10, r.pct % 10] } else { vec![r.pct] };
    let total = width * digits.len() as f64 + stroke * (digits.len() as f64 - 1.0);
    let left = ((s - total) / 2.0).floor();
    let mut lines = Vec::new();
    for (i, d) in digits.iter().enumerate() {
        // The stroke centre-lines run half a stroke inside the digit box.
        let x = left + i as f64 * (width + stroke) + half;
        let y = margin + half;
        let (w, h) = (width - stroke, s - 2.0 * margin - stroke);
        for l in digit_strokes(*d) {
            lines.push(l.into_iter().map(|(u, v)| (x + u * w, y + v * h)).collect());
        }
    }
    Shapes { lines, half, fills: vec![] }
}

/// RGBA pixels (straight alpha) of the icon.
pub fn render(r: Reading, style: Style) -> Vec<u8> {
    const SUB: u32 = 4;
    let size = style.size;
    let shapes = layout(r, size);
    let [cr, cg, cb] = color(r.level, style.theme);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut hits = 0;
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let p = (
                        f64::from(x) + (f64::from(sx) + 0.5) / f64::from(SUB),
                        f64::from(y) + (f64::from(sy) + 0.5) / f64::from(SUB),
                    );
                    if shapes.covers(p) {
                        hits += 1;
                    }
                }
            }
            let a = (hits * 255 / (SUB * SUB)) as u8;
            out.extend_from_slice(&[cr, cg, cb, a]);
        }
    }
    out
}

/// The taskbar theme (`SystemUsesLightTheme`, which is the taskbar's; `AppsUseLightTheme` is the
/// apps'). Unreadable → dark, Windows' default.
pub fn taskbar_theme() -> Theme {
    if crate::platform::system_uses_light_theme() == Some(true) { Theme::Light } else { Theme::Dark }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mikyas_core::engine::types::{DesktopHealth, ResetInfo, Source, SourceHealth, WindowState, WindowView};

    fn snap(values: &[(WindowKind, f32, bool)]) -> Snapshot {
        Snapshot {
            generated_ms: 0,
            windows: values
                .iter()
                .map(|(k, p, stale)| WindowView {
                    state: WindowState {
                        kind: k.clone(),
                        pct: *p,
                        reset: ResetInfo::Unknown,
                        source: Source::Desktop,
                        observed_at_ms: 0,
                        stale: *stale,
                        limit_reached: *p >= 100.0,
                        phase: Phase::Active,
                    },
                    burn: None,
                    spark: vec![],
                    worked_since: false,
                    is_main: false,
                })
                .collect(),
            session: None,
            sessions: vec![],
            health: SourceHealth {
                desktop: DesktopHealth::NotFound,
                cli_last_capture_ms: None,
                transcripts_last_activity_ms: None,
            },
            warnings: vec![],
        }
    }

    fn rd(pct: u8, level: Level) -> Option<Reading> {
        Some(Reading { pct, level })
    }

    fn worst() -> TrayNumber {
        TrayNumber::worst()
    }

    fn key(k: &str) -> TrayNumber {
        TrayNumber::new(k)
    }

    #[test]
    fn picks_the_value_for_each_mode() {
        let s = snap(&[(WindowKind::FiveHour, 22.4, false), (WindowKind::SevenDay, 61.0, false)]);
        assert_eq!(reading(&s, &worst()), rd(61, Level::Orange));
        assert_eq!(reading(&s, &key("five_hour")), rd(22, Level::Green));
        assert_eq!(reading(&s, &key("seven_day")), rd(61, Level::Orange));
        assert_eq!(reading(&s, &TrayNumber::off()), None);
        assert_eq!(reading(&snap(&[]), &worst()), None);
        // A chosen window that is not present shows the worst value instead.
        assert_eq!(reading(&s, &key("thirty_day")), rd(61, Level::Orange));
    }

    #[test]
    fn every_limit_counts_for_worst_and_can_be_chosen() {
        // A limit Anthropic adds (any key) binds: the tray shows it rather than staying green.
        let extra = WindowKind::from_key("seven_day_newmodel");
        let s = snap(&[
            (WindowKind::FiveHour, 20.0, false),
            (WindowKind::SevenDay, 50.0, false),
            (extra.clone(), 91.0, false),
        ]);
        assert_eq!(reading(&s, &worst()), rd(91, Level::Red));
        assert_eq!(reading(&s, &key("seven_day_newmodel")), rd(91, Level::Red));
        assert_eq!(reading(&s, &key("five_hour")), rd(20, Level::Green));
        // Only an extra limit: it is shown too.
        assert_eq!(reading(&snap(&[(extra, 90.0, false)]), &worst()), rd(90, Level::Red));
        // Main windows come first in the values, whatever the snapshot order.
        let order: Vec<(String, bool)> = tray_values(&snap(&[
            (WindowKind::from_key("seven_day_newmodel"), 1.0, false),
            (WindowKind::from_key("weekly"), 2.0, false),
        ]))
        .map(|v| (v.kind.key().to_owned(), v.main))
        .collect();
        assert_eq!(order, [("weekly".to_owned(), true), ("seven_day_newmodel".to_owned(), false)]);
    }

    #[test]
    fn fresh_beats_stale_and_stale_is_grey() {
        let s = snap(&[(WindowKind::FiveHour, 90.0, true), (WindowKind::SevenDay, 30.0, false)]);
        assert_eq!(reading(&s, &worst()), rd(30, Level::Green));
        assert_eq!(reading(&s, &key("five_hour")), rd(90, Level::Grey));
        let all_stale = snap(&[(WindowKind::FiveHour, 12.0, true), (WindowKind::SevenDay, 45.0, true)]);
        assert_eq!(reading(&all_stale, &worst()), rd(45, Level::Grey));
    }

    #[test]
    fn thresholds_rounding_and_full() {
        let one = |p: f32| reading(&snap(&[(WindowKind::FiveHour, p, false)]), &key("five_hour"));
        assert_eq!(one(39.4), rd(39, Level::Green));
        assert_eq!(one(39.6), rd(40, Level::Orange), "the colour follows the number shown");
        assert_eq!(one(70.0), rd(70, Level::Red));
        assert_eq!(one(99.4), rd(99, Level::Red));
        assert_eq!(one(99.6), rd(100, Level::Red));
        assert_eq!(one(100.0), rd(100, Level::Red));
        assert_eq!(one(-3.0), rd(0, Level::Green));
        let mut awaiting = snap(&[(WindowKind::FiveHour, 100.0, false)]);
        awaiting.windows[0].state.phase = Phase::ResetAwaitingData;
        assert_eq!(reading(&awaiting, &key("five_hour")), rd(0, Level::Green));
    }

    #[test]
    fn sizes_follow_the_display_scale() {
        let size = |s| Style::for_scale(s, Theme::Dark).size;
        assert_eq!([size(1.0), size(1.25), size(1.5), size(1.75), size(2.0), size(2.5)], [16, 20, 24, 32, 32, 32]);
    }

    fn alpha(px: &[u8]) -> Vec<u8> {
        px.chunks(4).map(|c| c[3]).collect()
    }

    fn style(size: u32, theme: Theme) -> Style {
        Style { size, theme }
    }

    #[test]
    fn renders_every_size_with_the_level_colour() {
        for size in [16, 20, 24, 32] {
            for theme in [Theme::Dark, Theme::Light] {
                let px = render(Reading { pct: 88, level: Level::Red }, style(size, theme));
                assert_eq!(px.len(), (size * size * 4) as usize);
                let inked = px.chunks(4).filter(|c| c[3] > 0).count();
                assert!(inked > (size * size / 8) as usize, "{size}px draws something");
                assert!(inked < (size * size) as usize, "{size}px keeps a transparent background");
                let rgb = color(Level::Red, theme);
                assert!(px.chunks(4).all(|c| c[..3] == rgb));
            }
        }
    }

    #[test]
    fn the_16px_digits_sit_on_the_pixel_grid() {
        // "88" at 16 px: two 7 px boxes 2 px apart, rows 2..14, 2 px strokes → solid pixels.
        let a = alpha(&render(Reading { pct: 88, level: Level::Green }, style(16, Theme::Dark)));
        let at = |x: usize, y: usize| a[y * 16 + x];
        assert_eq!(at(0, 2), 255, "top-left corner of the first 8");
        assert_eq!(at(3, 8), 255, "its middle bar");
        assert_eq!(at(3, 5), 0, "its upper counter is empty");
        assert_eq!(at(7, 8), 0, "the gap between the digits");
        assert_eq!(at(15, 13), 255, "bottom-right corner of the second 8");
        assert_eq!(at(3, 0), 0, "the margin above");
        assert_eq!(at(3, 15), 0, "the margin below");
    }

    #[test]
    fn numbers_and_the_full_glyph_differ() {
        let draw = |pct| alpha(&render(Reading { pct, level: Level::Red }, style(24, Theme::Dark)));
        let full = draw(100);
        assert_ne!(full, draw(99));
        assert_ne!(full, draw(10));
        assert_ne!(draw(7), draw(1));
        // Every digit has its own shape.
        let shapes: std::collections::HashSet<_> = (0..10).map(draw).collect();
        assert_eq!(shapes.len(), 10);
        // The lock is centred: its left and right halves mirror each other.
        let a = alpha(&render(Reading { pct: 100, level: Level::Red }, style(32, Theme::Dark)));
        let ink = |x0: usize, x1: usize| {
            (0..32).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| u32::from(a[y * 32 + x])).sum::<u32>()
        };
        let (l, r) = (ink(0, 16), ink(16, 32));
        assert!(l.abs_diff(r) * 10 < l, "left {l} vs right {r}");
    }

    #[test]
    fn stale_and_theme_colours_are_distinct() {
        for theme in [Theme::Dark, Theme::Light] {
            let set: std::collections::HashSet<_> =
                [Level::Green, Level::Orange, Level::Red, Level::Grey].map(|l| color(l, theme)).into();
            assert_eq!(set.len(), 4);
        }
        assert_ne!(color(Level::Green, Theme::Dark), color(Level::Green, Theme::Light));
    }

    #[test]
    fn number_icon_is_none_when_off() {
        let s = snap(&[(WindowKind::FiveHour, 50.0, false)]);
        let st = style(20, Theme::Light);
        assert!(number_icon(&s, &TrayNumber::off(), st).is_none());
        let img = number_icon(&s, &worst(), st).unwrap();
        assert_eq!((img.width(), img.height()), (20, 20));
    }
}
