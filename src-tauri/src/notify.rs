//! OS toast notifications for limit alerts.

use chrono::{Local, TimeZone};
use cuw_core::alerts::AlertEvent;
use cuw_core::engine::types::WindowKind;
use cuw_core::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms, now_ms};
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

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

pub fn show(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        crate::pipeline::log(&format!("notification failed: {e}"));
    }
}

pub fn show_alert(app: &AppHandle, event: &AlertEvent) {
    let (title, body) = alert_text(event, now_ms());
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
    show_alert(app, &event);
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
}
