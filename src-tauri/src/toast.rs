//! OS toast notifications for limit, context, pace, weekly-recap, finished-turn and connection
//! alerts, and the one-time hint after the widget was first hidden from its own × or menu.

use chrono::{Local, NaiveDateTime, TimeZone};
use mikyas_core::alerts::AlertEvent;
use mikyas_core::ctx_alerts::CtxAlertEvent;
use mikyas_core::engine::types::{Entrypoint, WindowKind};
use mikyas_core::pace_alerts::PaceAlertEvent;
use mikyas_core::recap::WeeklyRecap;
use mikyas_core::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms, ROUGH_RESET_PM_MS, now_ms};
use mikyas_core::turns::FinishedTurn;
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

/// "1h 12m", "2d 3h", "8m", "<1m"; a zero minor unit is left out ("12h", "2d"), as in the UI.
pub fn duration_text(ms: Ms) -> String {
    let ms = ms.max(0);
    let (major, minor) = if ms >= DAY_MS {
        (format!("{}d", ms / DAY_MS), (ms % DAY_MS / HOUR_MS, "h"))
    } else if ms >= HOUR_MS {
        (format!("{}h", ms / HOUR_MS), (ms % HOUR_MS / MINUTE_MS, "m"))
    } else if ms >= MINUTE_MS {
        return format!("{}m", ms / MINUTE_MS);
    } else {
        return "<1m".into();
    };
    match minor {
        (0, _) => major,
        (n, unit) => format!("{major} {n}{unit}"),
    }
}

/// Local dates and times for toast text. The app uses [`UserClock`]; tests pass a fixed zone and
/// fixed formats so they don't depend on the machine's time zone or locale.
pub trait Clock {
    /// The local wall-clock time of a Unix-ms instant.
    fn local(&self, ms: Ms) -> Option<NaiveDateTime>;
    /// Time of day without seconds: "15:40", "3:40 PM".
    fn time(&self, t: &NaiveDateTime) -> String;
    /// Short weekday: "Thu".
    fn weekday(&self, t: &NaiveDateTime) -> String;
    /// Day and short month: "12 Oct", "Oct 12".
    fn day_month(&self, t: &NaiveDateTime) -> String;
}

/// The local time zone, formatted the way the Windows user locale writes dates and times
/// (12/24-hour clock, localized weekday and month names), with fixed formats as the fallback.
pub struct UserClock;

impl Clock for UserClock {
    fn local(&self, ms: Ms) -> Option<NaiveDateTime> {
        Local.timestamp_millis_opt(ms).single().map(|t| t.naive_local())
    }
    fn time(&self, t: &NaiveDateTime) -> String {
        user_locale::time(t).unwrap_or_else(|| fixed_time(t))
    }
    fn weekday(&self, t: &NaiveDateTime) -> String {
        user_locale::weekday(t).unwrap_or_else(|| fixed_weekday(t))
    }
    fn day_month(&self, t: &NaiveDateTime) -> String {
        user_locale::day_month(t).unwrap_or_else(|| fixed_day_month(t))
    }
}

fn fixed_time(t: &NaiveDateTime) -> String {
    t.format("%H:%M").to_string()
}
fn fixed_weekday(t: &NaiveDateTime) -> String {
    t.format("%a").to_string()
}
fn fixed_day_month(t: &NaiveDateTime) -> String {
    t.format("%-d %b").to_string()
}

/// Win32 date/time formatting in the user's locale (`LOCALE_NAME_USER_DEFAULT`). `None` when a
/// call fails, so the caller falls back to the fixed formats.
#[cfg(windows)]
mod user_locale {
    use chrono::{Datelike, NaiveDateTime, Timelike};
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::Globalization::{
        GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, LOCALE_SMONTHDAY, TIME_NOSECONDS,
    };
    use windows_sys::core::PCWSTR;

    /// `LOCALE_NAME_USER_DEFAULT` is a null locale name.
    const USER_DEFAULT: PCWSTR = std::ptr::null();

    fn system_time(t: &NaiveDateTime) -> Option<SYSTEMTIME> {
        let field = |v: u32| u16::try_from(v).ok();
        Some(SYSTEMTIME {
            wYear: u16::try_from(t.year()).ok()?,
            wMonth: field(t.month())?,
            wDayOfWeek: field(t.weekday().num_days_from_sunday())?,
            wDay: field(t.day())?,
            wHour: field(t.hour())?,
            wMinute: field(t.minute())?,
            wSecond: 0,
            wMilliseconds: 0,
        })
    }

    /// Runs a Win32 call that fills a UTF-16 buffer and returns the characters written,
    /// including the terminating null (0 on failure).
    fn read(call: impl FnOnce(*mut u16, i32) -> i32) -> Option<String> {
        let mut buf = [0u16; 128];
        let n = usize::try_from(call(buf.as_mut_ptr(), buf.len() as i32)).ok()?;
        let text = String::from_utf16_lossy(buf.get(..n.checked_sub(1)?)?);
        (!text.trim().is_empty()).then_some(text)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The user's short time format without seconds.
    pub fn time(t: &NaiveDateTime) -> Option<String> {
        let st = system_time(t)?;
        // SAFETY: `st` outlives the call; `read` passes a buffer of the size it names.
        read(|buf, len| unsafe { GetTimeFormatEx(USER_DEFAULT, TIME_NOSECONDS, &st, std::ptr::null(), buf, len) })
    }

    fn date(t: &NaiveDateTime, picture: &str) -> Option<String> {
        let st = system_time(t)?;
        let picture = wide(picture);
        // SAFETY: `st` and the null-terminated `picture` outlive the call; see `read`.
        read(|buf, len| unsafe { GetDateFormatEx(USER_DEFAULT, 0, &st, picture.as_ptr(), buf, len, std::ptr::null()) })
    }

    pub fn weekday(t: &NaiveDateTime) -> Option<String> {
        date(t, "ddd")
    }

    /// The user's month-and-day pattern ("MMMM d", "d. MMMM") with the short month name.
    pub fn day_month(t: &NaiveDateTime) -> Option<String> {
        // SAFETY: see `read`.
        let picture = read(|buf, len| unsafe { GetLocaleInfoEx(USER_DEFAULT, LOCALE_SMONTHDAY, buf, len) })?;
        date(t, &short_month(&picture))
    }

    /// Turns a full month name ("MMMM") in a date picture into the short one ("MMM"), leaving
    /// quoted literal text alone.
    pub(super) fn short_month(picture: &str) -> String {
        let mut out = String::with_capacity(picture.len());
        let mut quoted = false;
        let mut chars = picture.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\'' {
                quoted = !quoted;
            } else if c == 'M' && !quoted {
                let mut n = 1;
                while chars.next_if_eq(&'M').is_some() {
                    n += 1;
                }
                out.push_str(&"MMM"[..n.min(3)]);
                continue;
            }
            out.push(c);
        }
        out
    }
}

#[cfg(not(windows))]
mod user_locale {
    use chrono::NaiveDateTime;

    pub fn time(_: &NaiveDateTime) -> Option<String> {
        None
    }
    pub fn weekday(_: &NaiveDateTime) -> Option<String> {
        None
    }
    pub fn day_month(_: &NaiveDateTime) -> Option<String> {
        None
    }
}

/// Calendar days from `now` to `at_ms` in local time (0 = the same day).
fn calendar_days(at_ms: Ms, now: Ms, clock: &impl Clock) -> Option<(NaiveDateTime, i64)> {
    let (at, today) = (clock.local(at_ms)?, clock.local(now)?);
    Some((at, (at.date() - today.date()).num_days()))
}

/// Local clock time of a reset, like the widget's own: "15:40" on the same calendar day,
/// "Mon 15:40" up to six calendar days away, "12 Oct 15:40" beyond that.
fn clock_text(at_ms: Ms, now: Ms, clock: &impl Clock) -> String {
    let Some((at, days)) = calendar_days(at_ms, now, clock) else {
        return String::new();
    };
    let time = clock.time(&at);
    // By calendar day, not elapsed time: seven days on is the same weekday as today.
    match days.abs() {
        0 => time,
        1..=6 => format!("{} {time}", clock.weekday(&at)),
        _ => format!("{} {time}", clock.day_month(&at)),
    }
}

/// A reset time as precisely as it is known: "15:40", "~15:40" when estimated, "~Thu" (or
/// "~today", "~12 Oct") when its margin `plus_minus` makes the time of day meaningless.
fn reset_when(at_ms: Ms, plus_minus: Ms, now: Ms, clock: &impl Clock) -> String {
    if plus_minus < ROUGH_RESET_PM_MS {
        let mark = if plus_minus > 0 { "~" } else { "" };
        return format!("{mark}{}", clock_text(at_ms, now, clock));
    }
    let Some((at, days)) = calendar_days(at_ms, now, clock) else {
        return String::new();
    };
    match days {
        0 => "~today".into(),
        1..=6 => format!("~{}", clock.weekday(&at)),
        _ => format!("~{}", clock.day_month(&at)),
    }
}

/// A span that ends at a reset, as precisely as the reset is known ("1h 12m", "~1h 12m", "~2d").
fn reset_span(ms: Ms, plus_minus: Ms) -> String {
    if plus_minus < ROUGH_RESET_PM_MS {
        let mark = if plus_minus > 0 { "~" } else { "" };
        return format!("{mark}{}", duration_text(ms));
    }
    if ms < DAY_MS { "~<1d".into() } else { format!("~{}d", (ms + DAY_MS / 2) / DAY_MS) }
}

/// Title and body of a notification for an alert event.
pub fn alert_text(event: &AlertEvent, now: Ms, clock: &impl Clock) -> (String, String) {
    match event {
        AlertEvent::Threshold { kind, threshold, reset_at_ms, reset_plus_minus_ms, .. } => {
            let title = format!("Claude {} limit at {threshold}%", window_name(kind));
            let pm = *reset_plus_minus_ms;
            let body = match reset_at_ms.filter(|r| *r > now) {
                Some(r) => format!("Resets {} (in {})", reset_when(r, pm, now, clock), reset_span(r - now, pm)),
                None => "Reset time unknown".into(),
            };
            (title, body)
        }
        AlertEvent::Reset { kind } => {
            (format!("Claude {} limit reset", window_name(kind)), "Usage is back to 0%.".into())
        }
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
pub fn pace_text(event: &PaceAlertEvent, now: Ms, clock: &impl Clock) -> (String, String) {
    match event {
        PaceAlertEvent::Forecast { kind, t100_ms, reset_at_ms, reset_plus_minus_ms, .. } => (
            format!("At this pace: {} limit at {}", window_name(kind), clock_text(*t100_ms, now, clock)),
            format!(
                "That's {} before it resets ({}).",
                reset_span(reset_at_ms - t100_ms, *reset_plus_minus_ms),
                reset_when(*reset_at_ms, *reset_plus_minus_ms, now, clock)
            ),
        ),
        PaceAlertEvent::HeadsUp { kind, reset_at_ms } => (
            format!("Claude {} limit reopens in {} min", window_name(kind), minutes_up(reset_at_ms - now)),
            format!("It resets at {}.", clock_text(*reset_at_ms, now, clock)),
        ),
    }
}

/// "Last week: 82% of your weekly limit" / "Busiest day Tue (35%) · 14 five-hour resets ·
/// 5-hour peak 100%" (the busiest day is left out when unknown).
pub fn recap_text(recap: &WeeklyRecap, clock: &impl Clock) -> (String, String) {
    let title = format!("Last week: {:.0}% of your weekly limit", recap.used_pct);
    let mut parts = Vec::new();
    if let Some((day_ms, pct)) = recap.busiest_day {
        if let Some(day) = clock.local(day_ms) {
            parts.push(format!("Busiest day {} ({pct:.0}%)", clock.weekday(&day)));
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

/// Title and body of any toast, with dates and times in the user's locale.
pub fn toast_text(alert: &Alert, now: Ms) -> (String, String) {
    toast_text_with(alert, now, &UserClock)
}

/// [`toast_text`] with the given local time zone and formats.
pub fn toast_text_with(alert: &Alert, now: Ms, clock: &impl Clock) -> (String, String) {
    match alert {
        Alert::Limit(event) => alert_text(event, now, clock),
        Alert::Context(event) => ctx_alert_text(event),
        Alert::Pace(event) => pace_text(event, now, clock),
        Alert::Recap(recap) => recap_text(recap, clock),
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
    ("Mikyas is still running".into(), body)
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
            reset_plus_minus_ms: 0,
        },
        _ => AlertEvent::Reset { kind: WindowKind::SevenDay },
    };
    show_alert(app, &Alert::Limit(event));
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    /// UTC with the fixed formats: the same text on every machine.
    struct TestClock;

    impl Clock for TestClock {
        fn local(&self, ms: Ms) -> Option<NaiveDateTime> {
            Utc.timestamp_millis_opt(ms).single().map(|t| t.naive_utc())
        }
        fn time(&self, t: &NaiveDateTime) -> String {
            fixed_time(t)
        }
        fn weekday(&self, t: &NaiveDateTime) -> String {
            fixed_weekday(t)
        }
        fn day_month(&self, t: &NaiveDateTime) -> String {
            fixed_day_month(t)
        }
    }

    /// Monday 2026-09-21 14:13:20 UTC.
    const NOW: Ms = 1_790_000_000_000;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Ms {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().timestamp_millis()
    }

    #[test]
    fn texts() {
        let now = NOW;
        let (t, b) = alert_text(
            &AlertEvent::Threshold {
                kind: WindowKind::FiveHour,
                threshold: 80,
                pct: 80.2,
                reset_at_ms: Some(now + HOUR_MS + 12 * MINUTE_MS + 5_000),
                reset_plus_minus_ms: 0,
            },
            now,
            &TestClock,
        );
        assert_eq!(t, "Claude 5-hour limit at 80%");
        assert_eq!(b, "Resets 15:25 (in 1h 12m)");
        let (t, _) = alert_text(&AlertEvent::Reset { kind: WindowKind::SevenDay }, now, &TestClock);
        assert_eq!(t, "Claude weekly limit reset");
        assert_eq!(window_name(&WindowKind::Other("seven_day_opus".into())), "weekly Opus");
        assert_eq!(duration_text(2 * DAY_MS + 3 * HOUR_MS), "2d 3h");
        assert_eq!(duration_text(30_000), "<1m");
        assert_eq!(duration_text(-5), "<1m");
        assert_eq!(duration_text(8 * MINUTE_MS + 59_999), "8m");
        assert_eq!(duration_text(12 * HOUR_MS), "12h");
        assert_eq!(duration_text(12 * HOUR_MS + 59_999), "12h");
        assert_eq!(duration_text(HOUR_MS + 12 * MINUTE_MS), "1h 12m");
        assert_eq!(duration_text(2 * DAY_MS), "2d");
        assert_eq!(duration_text(2 * DAY_MS + 59 * MINUTE_MS), "2d");
    }

    #[test]
    fn estimated_resets_match_their_precision() {
        let now = NOW;
        let at = now + 2 * DAY_MS + 3 * HOUR_MS;
        let threshold = |pm| AlertEvent::Threshold {
            kind: WindowKind::SevenDay,
            threshold: 80,
            pct: 81.0,
            reset_at_ms: Some(at),
            reset_plus_minus_ms: pm,
        };
        assert_eq!(alert_text(&threshold(DAY_MS), now, &TestClock).1, "Resets ~Wed (in ~2d)");
        assert_eq!(alert_text(&threshold(25 * MINUTE_MS), now, &TestClock).1, "Resets ~Wed 17:13 (in ~2d 3h)");
        assert_eq!(reset_when(now, ROUGH_RESET_PM_MS, now, &TestClock), "~today");
        assert_eq!(reset_when(now + 7 * DAY_MS, ROUGH_RESET_PM_MS, now, &TestClock), "~28 Sep");
        assert_eq!(reset_span(DAY_MS - 1, ROUGH_RESET_PM_MS), "~<1d");
        assert_eq!(reset_span(2 * DAY_MS + 13 * HOUR_MS, ROUGH_RESET_PM_MS), "~3d");
        assert_eq!(reset_span(HOUR_MS, 0), "1h");

        let forecast = PaceAlertEvent::Forecast {
            kind: WindowKind::SevenDay,
            pct: 70.0,
            t100_ms: now + DAY_MS,
            reset_at_ms: at,
            reset_plus_minus_ms: DAY_MS,
        };
        assert_eq!(pace_text(&forecast, now, &TestClock).1, "That's ~1d before it resets (~Wed).");
    }

    #[test]
    fn clock_text_goes_by_calendar_day_like_the_widget() {
        let late = utc(2026, 9, 21, 23, 0);
        // Two hours on but past midnight: the day is named.
        assert_eq!(clock_text(late + 2 * HOUR_MS, late, &TestClock), "Tue 01:00");
        // Twenty hours on, still the same day: time only.
        let early = utc(2026, 9, 21, 0, 30);
        assert_eq!(clock_text(early + 20 * HOUR_MS + 30 * MINUTE_MS, early, &TestClock), "21:00");
        assert_eq!(clock_text(NOW, NOW, &TestClock), "14:13");
        assert_eq!(clock_text(NOW + 6 * DAY_MS, NOW, &TestClock), "Sun 14:13");
        // Seven days on is the same weekday as today, so the date is shown.
        assert_eq!(clock_text(NOW + 7 * DAY_MS, NOW, &TestClock), "28 Sep 14:13");
        assert_eq!(clock_text(NOW - DAY_MS, NOW, &TestClock), "Sun 14:13");
        assert_eq!(clock_text(Ms::MAX, NOW, &TestClock), "");
        assert_eq!(reset_when(Ms::MAX, ROUGH_RESET_PM_MS, NOW, &TestClock), "");
    }

    #[cfg(windows)]
    #[test]
    fn user_locale_formats_are_filled_in() {
        // Whatever the machine's locale: a time with digits, and non-empty names.
        let t = TestClock.local(NOW).unwrap();
        let time = user_locale::time(&t).expect("GetTimeFormatEx");
        assert!(time.chars().any(char::is_numeric), "{time}");
        assert!(!user_locale::weekday(&t).expect("GetDateFormatEx").is_empty());
        let day_month = user_locale::day_month(&t).expect("month-day format");
        assert!(day_month.chars().any(char::is_numeric), "{day_month}");
        assert!(!UserClock.time(&t).is_empty() && !UserClock.weekday(&t).is_empty());
        assert!(!UserClock.day_month(&t).is_empty());
        assert!(UserClock.local(NOW).is_some());
        let heads_up = Alert::Pace(PaceAlertEvent::HeadsUp { kind: WindowKind::FiveHour, reset_at_ms: NOW });
        assert!(toast_text(&heads_up, NOW).1.starts_with("It resets at "));
    }

    #[cfg(windows)]
    #[test]
    fn month_day_pictures_get_the_short_month() {
        use user_locale::short_month;
        assert_eq!(short_month("MMMM d"), "MMM d");
        assert_eq!(short_month("d. MMMM"), "d. MMM");
        assert_eq!(short_month("d' de 'MMMM"), "d' de 'MMM");
        assert_eq!(short_month("'MMMM' MMMM"), "'MMMM' MMM");
        assert_eq!(short_month("MMM d"), "MMM d");
        assert_eq!(short_month("M/d"), "M/d");
        assert_eq!(short_month("MMMMM d"), "MMM d");
    }

    #[test]
    fn hide_hint_names_the_shortcut_when_there_is_one() {
        let (t, b) = hide_hint_text("Ctrl+Alt+H");
        assert_eq!(t, "Mikyas is still running");
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
        let now = NOW;
        let forecast = PaceAlertEvent::Forecast {
            kind: WindowKind::FiveHour,
            pct: 64.0,
            t100_ms: now + 40 * MINUTE_MS,
            reset_at_ms: now + 40 * MINUTE_MS + HOUR_MS + 12 * MINUTE_MS + 5_000,
            reset_plus_minus_ms: 0,
        };
        let (t, b) = pace_text(&forecast, now, &TestClock);
        assert_eq!(t, "At this pace: 5-hour limit at 14:53");
        assert_eq!(b, "That's 1h 12m before it resets (16:05).");
        let weekly = PaceAlertEvent::Forecast {
            kind: WindowKind::SevenDay,
            pct: 70.0,
            t100_ms: now + 2 * DAY_MS,
            reset_at_ms: now + 3 * DAY_MS,
            reset_plus_minus_ms: 0,
        };
        let (t, b) = pace_text(&weekly, now, &TestClock);
        assert_eq!(t, "At this pace: weekly limit at Wed 14:13");
        assert_eq!(b, "That's 1d before it resets (Thu 14:13).");

        let heads_up = |kind, left| PaceAlertEvent::HeadsUp { kind, reset_at_ms: now + left };
        let (t, b) = pace_text(&heads_up(WindowKind::FiveHour, 10 * MINUTE_MS), now, &TestClock);
        assert_eq!(t, "Claude 5-hour limit reopens in 10 min");
        assert_eq!(b, "It resets at 14:23.");
        let almost = pace_text(&heads_up(WindowKind::FiveHour, 9 * MINUTE_MS + 30_000), now, &TestClock);
        assert_eq!(almost.0, "Claude 5-hour limit reopens in 10 min");
        let weekly_soon = pace_text(&heads_up(WindowKind::SevenDay, HOUR_MS), now, &TestClock);
        assert_eq!(weekly_soon.0, "Claude weekly limit reopens in 60 min");
        let passed = pace_text(&heads_up(WindowKind::FiveHour, 0), now, &TestClock);
        assert_eq!(passed.0, "Claude 5-hour limit reopens in 1 min");
        assert_eq!(
            toast_text_with(&Alert::Pace(forecast.clone()), now, &TestClock),
            pace_text(&forecast, now, &TestClock)
        );
    }

    #[test]
    fn recap_texts() {
        let tuesday = utc(2026, 9, 22, 0, 0);
        let recap = WeeklyRecap {
            window_end_ms: tuesday + 4 * DAY_MS,
            used_pct: 82.4,
            busiest_day: Some((tuesday, 35.2)),
            five_hour_resets: 14,
            peak_five_hour_pct: 100.0,
        };
        let (t, b) = recap_text(&recap, &TestClock);
        assert_eq!(t, "Last week: 82% of your weekly limit");
        assert_eq!(b, "Busiest day Tue (35%) · 14 five-hour resets · 5-hour peak 100%");
        let quiet = WeeklyRecap { busiest_day: None, five_hour_resets: 1, peak_five_hour_pct: 41.6, ..recap.clone() };
        assert_eq!(recap_text(&quiet, &TestClock).1, "1 five-hour reset · 5-hour peak 42%");
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
