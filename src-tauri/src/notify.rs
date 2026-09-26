//! OS toast notifications for limit and context alerts, and the one-time hint after the widget
//! was first hidden from its own × or menu.

use chrono::{Local, TimeZone};
use cuw_core::alerts::AlertEvent;
use cuw_core::ctx_alerts::CtxAlertEvent;
use cuw_core::engine::types::WindowKind;
use cuw_core::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms, now_ms};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Anything the pipeline turns into a toast.
#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    /// A usage limit crossed a threshold or reset.
    Limit(AlertEvent),
    /// A session's context window crossed a threshold.
    Context(CtxAlertEvent),
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

/// Title and body of any toast.
pub fn toast_text(alert: &Alert, now: Ms) -> (String, String) {
    match alert {
        Alert::Limit(event) => alert_text(event, now),
        Alert::Context(event) => ctx_alert_text(event),
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
        crate::pipeline::log(&format!("notification failed: {e}"));
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
}
