//! OS toast notifications for limit, context, pace, weekly-recap, finished-turn and connection
//! alerts, and the one-time hint after the widget was first hidden from its own × or menu.

use chrono::{Local, TimeZone};
use cuw_core::alerts::AlertEvent;
use cuw_core::ctx_alerts::CtxAlertEvent;
use cuw_core::engine::types::{Entrypoint, WindowKind};
use cuw_core::pace_alerts::PaceAlertEvent;
use cuw_core::recap::WeeklyRecap;
use cuw_core::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms, now_ms};
use cuw_core::turns::FinishedTurn;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Anything the pipeline turns into a toast.
#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    /// A usage limit crossed a threshold or reset.
    Limit(AlertEvent),
    /// A session's context window crossed a threshold.
    Context(CtxAlertEvent),
    /// The current pace reaches a limit before it resets, or a capped limit reopens soon.
    Pace(PaceAlertEvent),
    /// Summary of the weekly window that just ended.
    Recap(WeeklyRecap),
    /// A long Claude turn ended.
    Finished(FinishedTurn),
    /// Claude Code's status line no longer runs the widget's capture.
    ConnectionLost,
}

/// "5-hour", "weekly", "weekly Opus", …
pub fn window_name(kind: &WindowKind) -> String {
    match kind {
        WindowKind::FiveHour => "5-hour".into(),
        WindowKind::SevenDay => "weekly".into(),
        WindowKind::Other(k) => match k.strip_prefix("seven_day_") {
            Some(model) => {
                let mut c = model.chars();
                let cap: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
                format!("weekly {cap}")
            }
            None => k.replace('_', " "),
        },
    }
}

/// "1h 12m", "2d 3h", "8m", "<1m".
pub fn duration_text(ms: Ms) -> String {
    let ms = ms.max(0);
    if ms >= DAY_MS {
        format!("{}d {}h", ms / DAY_MS, ms % DAY_MS / HOUR_MS)
    } else if ms >= HOUR_MS {
        format!("{}h {}m", ms / HOUR_MS, ms % HOUR_MS / MINUTE_MS)
    } else if ms >= MINUTE_MS {
        format!("{}m", ms / MINUTE_MS)
    } else {
        "<1m".into()
    }
}

/// Local clock time of a reset: "15:40" today-ish, "Mon 15:40" further out.
fn clock_text(at_ms: Ms, now: Ms) -> String {
    let Some(t) = Local.timestamp_millis_opt(at_ms).single() else {
        return String::new();
    };
    if at_ms - now < 20 * HOUR_MS {
        t.format("%H:%M").to_string()
    } else {
        t.format("%a %H:%M").to_string()
    }
}

/// Title and body of a notification for an alert event.
pub fn alert_text(event: &AlertEvent, now: Ms) -> (String, String) {
    match event {
        AlertEvent::Threshold {
            kind,
            threshold,
            reset_at_ms,
            ..
        } => {
            let title = format!("Claude {} limit at {threshold}%", window_name(kind));
            let body = match reset_at_ms.filter(|r| *r > now) {
                Some(r) => format!("Resets {} (in {})", clock_text(r, now), duration_text(r - now)),
                None => "Reset time unknown".into(),
            };
            (title, body)
        }
        AlertEvent::Reset { kind } => (
            format!("Claude {} limit reset", window_name(kind)),
            "Usage is back to 0%.".into(),
        ),
    }
}

/// "Opus 5.5" from a display name, else a tidied model id ("sonnet-5"), else a generic name.
fn model_title(model: Option<&str>) -> String {
    match model.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.strip_prefix("claude-").unwrap_or(m).to_owned(),
        None => "Claude session".into(),
    }
}

/// Title and body of a context-alert notification: "Opus 5.5 at 90% context" /
/// "Consider /compact or a new session." (+ " · <project>" when the project is shown).
pub fn ctx_alert_text(event: &CtxAlertEvent) -> (String, String) {
    let title = format!("{} at {}% context", model_title(event.model.as_deref()), event.threshold);
    let mut body = String::from("Consider /compact or a new session.");
    if let Some(project) = event.project.as_deref().filter(|p| !p.is_empty()) {
        body.push_str(" · ");
        body.push_str(project);
    }
    (title, body)
}

/// Whole minutes left, rounded up and at least 1 (9 min 30 s left is "10 min").
fn minutes_up(ms: Ms) -> Ms {
    ((ms.max(0) + MINUTE_MS - 1) / MINUTE_MS).max(1)
}

/// "At this pace: 5-hour limit at 15:40" / "That's 1h 12m before it resets (16:52).", and
/// "Claude 5-hour limit reopens in 10 min" / "It resets at 16:52.".
pub fn pace_text(event: &PaceAlertEvent, now: Ms) -> (String, String) {
    match event {
        PaceAlertEvent::Forecast {
            kind,
            t100_ms,
            reset_at_ms,
            ..
        } => (
            format!("At this pace: {} limit at {}", window_name(kind), clock_text(*t100_ms, now)),
            format!(
                "That's {} before it resets ({}).",
                duration_text(reset_at_ms - t100_ms),
                clock_text(*reset_at_ms, now)
            ),
        ),
        PaceAlertEvent::HeadsUp { kind, reset_at_ms } => (
            format!(
                "Claude {} limit reopens in {} min",
                window_name(kind),
                minutes_up(reset_at_ms - now)
            ),
            format!("It resets at {}.", clock_text(*reset_at_ms, now)),
        ),
    }
}

/// "Last week: 82% of your weekly limit" / "Busiest day Tue (35%) · 14 five-hour resets ·
/// 5-hour peak 100%" (the busiest day is left out when unknown).
pub fn recap_text(recap: &WeeklyRecap) -> (String, String) {
    let title = format!("Last week: {:.0}% of your weekly limit", recap.used_pct);
    let mut parts = Vec::new();
    if let Some((day_ms, pct)) = recap.busiest_day {
        if let Some(day) = Local.timestamp_millis_opt(day_ms).single() {
            parts.push(format!("Busiest day {} ({pct:.0}%)", day.format("%a")));
        }
    }
    parts.push(match recap.five_hour_resets {
        1 => "1 five-hour reset".to_owned(),
        n => format!("{n} five-hour resets"),
    });
    parts.push(format!("5-hour peak {:.0}%", recap.peak_five_hour_pct));
    (title, parts.join(" · "))
}

/// Where a session runs, for toasts without a project name.
fn surface_name(entrypoint: Entrypoint) -> &'static str {
    match entrypoint {
        Entrypoint::Cli => "Terminal session",
        Entrypoint::Desktop => "Desktop Code tab",
        Entrypoint::Cowork => "Cowork",
        Entrypoint::Unknown => "Claude Code session",
    }
}

/// "Claude finished · 12 min" / the project, else the surface. `turn.project` comes from the
/// session view, which only carries it while `show_project` is on.
pub fn finished_text(turn: &FinishedTurn) -> (String, String) {
    let took = if turn.duration_ms < HOUR_MS {
        format!("{} min", (turn.duration_ms / MINUTE_MS).max(1))
    } else {
        duration_text(turn.duration_ms)
    };
    let body = match turn.project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(project) => project.to_owned(),
        None => surface_name(turn.entrypoint).to_owned(),
    };
    (format!("Claude finished · {took}"), body)
}

pub fn connection_lost_text() -> (String, String) {
    (
        "Claude Code status line was changed".into(),
        "The widget no longer gets exact limits. Open the widget to reconnect.".into(),
    )
}

/// Title and body of any toast.
pub fn toast_text(alert: &Alert, now: Ms) -> (String, String) {
    match alert {
        Alert::Limit(event) => alert_text(event, now),
        Alert::Context(event) => ctx_alert_text(event),
        Alert::Pace(event) => pace_text(event, now),
        Alert::Recap(recap) => recap_text(recap),
        Alert::Finished(turn) => finished_text(turn),
        Alert::ConnectionLost => connection_lost_text(),
    }
}

/// Title and body of the one-time toast after the first hide from the widget; `toggle_hotkey` is
/// the show/hide shortcut ("" when there is none or it could not be registered).
pub fn hide_hint_text(toggle_hotkey: &str) -> (String, String) {
    let body = match toggle_hotkey.trim() {
        "" => "Click the tray icon to show it again.".to_owned(),
        hotkey => format!("Click the tray icon or press {hotkey} to show it again."),
    };
    ("Claude Usage is still running".into(), body)
}

pub fn show(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        crate::diag::log(&format!("notification failed: {e}"));
    }
}

pub fn show_alert(app: &AppHandle, alert: &Alert) {
    let (title, body) = toast_text(alert, now_ms());
    show(app, &title, &body);
}

/// Debug-menu samples.
#[cfg(debug_assertions)]
pub fn simulate(app: &AppHandle, id: &str) {
    let now = now_ms();
    let event = match id {
        "sim_80" | "sim_95" => AlertEvent::Threshold {
            kind: WindowKind::FiveHour,
            threshold: if id == "sim_80" { 80 } else { 95 },
            pct: if id == "sim_80" { 80.4 } else { 95.2 },
            reset_at_ms: Some(now + HOUR_MS + 12 * MINUTE_MS),
        },
        _ => AlertEvent::Reset {
            kind: WindowKind::SevenDay,
        },
    };
    show_alert(app, &Alert::Limit(event));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts() {
        let now = 1_790_000_000_000;
        let (t, b) = alert_text(
            &AlertEvent::Threshold {
                kind: WindowKind::FiveHour,
                threshold: 80,
                pct: 80.2,
                reset_at_ms: Some(now + HOUR_MS + 12 * MINUTE_MS + 5_000),
            },
            now,
        );
        assert_eq!(t, "Claude 5-hour limit at 80%");
        assert!(b.starts_with("Resets ") && b.ends_with("(in 1h 12m)"), "{b}");
        let (t, _) = alert_text(&AlertEvent::Reset { kind: WindowKind::SevenDay }, now);
        assert_eq!(t, "Claude weekly limit reset");
        assert_eq!(window_name(&WindowKind::Other("seven_day_opus".into())), "weekly Opus");
        assert_eq!(duration_text(2 * DAY_MS + 3 * HOUR_MS), "2d 3h");
        assert_eq!(duration_text(30_000), "<1m");
    }

    #[test]
    fn hide_hint_names_the_shortcut_when_there_is_one() {
        let (t, b) = hide_hint_text("Ctrl+Alt+H");
        assert_eq!(t, "Claude Usage is still running");
        assert_eq!(b, "Click the tray icon or press Ctrl+Alt+H to show it again.");
        assert_eq!(hide_hint_text(" Ctrl+Shift+F9 ").1, "Click the tray icon or press Ctrl+Shift+F9 to show it again.");
        assert_eq!(hide_hint_text("").1, "Click the tray icon to show it again.");
        assert_eq!(hide_hint_text("  ").1, "Click the tray icon to show it again.");
    }

    fn ctx_event(model: Option<&str>, project: Option<&str>) -> CtxAlertEvent {
        CtxAlertEvent {
            key: "af63dc4c8601ec8c".into(),
            threshold: 90,
            pct: 91.4,
            model: model.map(Into::into),
            project: project.map(Into::into),
        }
    }

    #[test]
    fn context_texts() {
        let (t, b) = ctx_alert_text(&ctx_event(Some("Opus 5.5"), None));
        assert_eq!(t, "Opus 5.5 at 90% context");
        assert_eq!(b, "Consider /compact or a new session.");
        let (t, b) = ctx_alert_text(&ctx_event(Some("claude-sonnet-5"), Some("demo-app")));
        assert_eq!(t, "sonnet-5 at 90% context");
        assert_eq!(b, "Consider /compact or a new session. · demo-app");
        let (t, b) = ctx_alert_text(&ctx_event(Some("  "), Some("")));
        assert_eq!(t, "Claude session at 90% context");
        assert_eq!(b, "Consider /compact or a new session.");
        let alert = Alert::Context(ctx_event(None, None));
        assert_eq!(toast_text(&alert, 0).0, "Claude session at 90% context");
        let limit = Alert::Limit(AlertEvent::Reset { kind: WindowKind::FiveHour });
        assert_eq!(toast_text(&limit, 0).0, "Claude 5-hour limit reset");
    }

    #[test]
    fn pace_texts() {
        let now = 1_790_000_000_000;
        let forecast = PaceAlertEvent::Forecast {
            kind: WindowKind::FiveHour,
            pct: 64.0,
            t100_ms: now + 40 * MINUTE_MS,
            reset_at_ms: now + 40 * MINUTE_MS + HOUR_MS + 12 * MINUTE_MS + 5_000,
        };
        let (t, b) = pace_text(&forecast, now);
        assert!(t.starts_with("At this pace: 5-hour limit at "), "{t}");
        assert_eq!(t.len(), "At this pace: 5-hour limit at 15:40".len(), "{t}");
        assert!(b.starts_with("That's 1h 12m before it resets (") && b.ends_with(")."), "{b}");
        let weekly = PaceAlertEvent::Forecast {
            kind: WindowKind::SevenDay,
            pct: 70.0,
            t100_ms: now + 2 * DAY_MS,
            reset_at_ms: now + 3 * DAY_MS,
        };
        let (t, b) = pace_text(&weekly, now);
        assert!(t.starts_with("At this pace: weekly limit at "), "{t}");
        assert!(b.starts_with("That's 1d 0h before it resets ("), "{b}");

        let heads_up = |kind, left| PaceAlertEvent::HeadsUp {
            kind,
            reset_at_ms: now + left,
        };
        let (t, b) = pace_text(&heads_up(WindowKind::FiveHour, 10 * MINUTE_MS), now);
        assert_eq!(t, "Claude 5-hour limit reopens in 10 min");
        assert!(b.starts_with("It resets at ") && b.ends_with('.'), "{b}");
        let almost = pace_text(&heads_up(WindowKind::FiveHour, 9 * MINUTE_MS + 30_000), now);
        assert_eq!(almost.0, "Claude 5-hour limit reopens in 10 min");
        let weekly_soon = pace_text(&heads_up(WindowKind::SevenDay, HOUR_MS), now);
        assert_eq!(weekly_soon.0, "Claude weekly limit reopens in 60 min");
        let passed = pace_text(&heads_up(WindowKind::FiveHour, 0), now);
        assert_eq!(passed.0, "Claude 5-hour limit reopens in 1 min");
        assert_eq!(toast_text(&Alert::Pace(forecast.clone()), now), pace_text(&forecast, now));
    }

    #[test]
    fn recap_texts() {
        // 2026-09-22 is a Tuesday in every time zone.
        let tuesday = Local
            .with_ymd_and_hms(2026, 9, 22, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_millis();
        let recap = WeeklyRecap {
            window_end_ms: tuesday + 4 * DAY_MS,
            used_pct: 82.4,
            busiest_day: Some((tuesday, 35.2)),
            five_hour_resets: 14,
            peak_five_hour_pct: 100.0,
        };
        let (t, b) = recap_text(&recap);
        assert_eq!(t, "Last week: 82% of your weekly limit");
        assert_eq!(b, "Busiest day Tue (35%) · 14 five-hour resets · 5-hour peak 100%");
        let quiet = WeeklyRecap {
            busiest_day: None,
            five_hour_resets: 1,
            peak_five_hour_pct: 41.6,
            ..recap.clone()
        };
        assert_eq!(recap_text(&quiet).1, "1 five-hour reset · 5-hour peak 42%");
        assert_eq!(toast_text(&Alert::Recap(recap), 0).0, "Last week: 82% of your weekly limit");
    }

    fn turn(duration_ms: Ms, project: Option<&str>, entrypoint: Entrypoint) -> FinishedTurn {
        FinishedTurn {
            key: "af63dc4c8601ec8c".into(),
            duration_ms,
            model: Some("Opus 5.5".into()),
            project: project.map(Into::into),
            entrypoint,
        }
    }

    #[test]
    fn finished_texts() {
        let (t, b) = finished_text(&turn(12 * MINUTE_MS + 40_000, None, Entrypoint::Cli));
        assert_eq!(t, "Claude finished · 12 min");
        assert_eq!(b, "Terminal session");
        assert_eq!(finished_text(&turn(3 * MINUTE_MS, None, Entrypoint::Desktop)).1, "Desktop Code tab");
        assert_eq!(finished_text(&turn(3 * MINUTE_MS, Some(" "), Entrypoint::Cowork)).1, "Cowork");
        assert_eq!(finished_text(&turn(3 * MINUTE_MS, None, Entrypoint::Unknown)).1, "Claude Code session");
        let (t, b) = finished_text(&turn(HOUR_MS + 5 * MINUTE_MS, Some("demo-app"), Entrypoint::Cli));
        assert_eq!(t, "Claude finished · 1h 5m");
        assert_eq!(b, "demo-app");
        assert_eq!(finished_text(&turn(20_000, None, Entrypoint::Cli)).0, "Claude finished · 1 min");
        let alert = Alert::Finished(turn(12 * MINUTE_MS, None, Entrypoint::Cli));
        assert_eq!(toast_text(&alert, 0).0, "Claude finished · 12 min");
    }

    #[test]
    fn connection_lost_texts() {
        let (t, b) = toast_text(&Alert::ConnectionLost, 0);
        assert_eq!(t, "Claude Code status line was changed");
        assert_eq!(b, "The widget no longer gets exact limits. Open the widget to reconnect.");
    }
}
