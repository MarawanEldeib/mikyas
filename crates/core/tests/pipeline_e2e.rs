//! End-to-end chains over synthetic data directories: Desktop `plan-usage-history.json`,
//! Claude Code transcripts and statusline captures → sources → `build_snapshot` → history →
//! limit / context / pace alerts, weekly recap and finished turns, through the same steps the
//! app's pipeline runs (see `common`). Assertions are on what the user sees: the %, the kind of
//! reset time, and which toasts fire (each exactly once).

mod common;

use common::{Env, SID, SID2, Statusline, Widget, assistant_line, identity_line, prompt_line};
use mikyas_core::alerts::AlertEvent;
use mikyas_core::engine::types::{Confidence, CtxBasis, DesktopHealth, Phase, ResetInfo, Source, WindowKind};
use mikyas_core::history::ViewRange;
use mikyas_core::time::{DAY_MS, HOUR_MS, MINUTE_MS, Ms};

/// A multiple of five minutes (alert instance keys round to that).
const T0: Ms = 1_790_000_100_000;

fn five_hour(pct: f64, reset_ms: Ms) -> Statusline {
    Statusline { five_hour: Some((pct, reset_ms)), ..Statusline::new(SID) }
}

fn thresholds(events: &[AlertEvent]) -> Vec<(WindowKind, u8)> {
    events
        .iter()
        .filter_map(|e| match e {
            AlertEvent::Threshold { kind, threshold, .. } => Some((kind.clone(), *threshold)),
            AlertEvent::Reset { .. } => None,
        })
        .collect()
}

fn resets(events: &[AlertEvent]) -> Vec<WindowKind> {
    events
        .iter()
        .filter_map(|e| match e {
            AlertEvent::Reset { kind } => Some(kind.clone()),
            AlertEvent::Threshold { .. } => None,
        })
        .collect()
}

#[test]
fn nothing_installed_shows_nothing_and_never_alerts() {
    let env = Env::new();
    let mut widget = Widget::start(&env);
    for i in 0..3 {
        let tick = widget.tick(T0 + i * MINUTE_MS);
        assert!(tick.snap.windows.is_empty());
        assert_eq!(tick.snap.session, None);
        assert_eq!(tick.toasts(), 0);
        assert_eq!(widget.desktop_health, DesktopHealth::NotFound);
    }
    assert!(widget.history.rows().is_empty());
}

#[test]
fn cli_ticks_fire_each_threshold_once_and_record_only_changes() {
    let env = Env::new();
    let reset = T0 + 3 * HOUR_MS;
    let mut widget = Widget::start(&env);
    let mut fired = Vec::new();
    let mut pace = 0;
    for (i, pct) in [50.0, 50.0, 70.0, 81.0, 85.0, 85.0, 90.0, 96.0, 97.0, 97.0].into_iter().enumerate() {
        let now = T0 + i as Ms * 5 * MINUTE_MS;
        env.statusline(&Statusline { api_ms: 1 + i as u64, ..five_hour(pct, reset) }, now);
        let tick = widget.tick(now);
        let w = tick.five_hour();
        assert_eq!(w.state.pct, pct as f32);
        assert_eq!(w.state.reset, ResetInfo::Exact { at_ms: reset });
        assert_eq!(w.state.source, Source::Cli);
        assert!(resets(&tick.limit).is_empty(), "no reset happened: {:?}", tick.limit);
        fired.extend(thresholds(&tick.limit));
        pace += tick.pace.len();
    }
    assert_eq!(fired, [(WindowKind::FiveHour, 80), (WindowKind::FiveHour, 95)]);
    assert!(pace <= 1, "the pace forecast fires at most once per window");
    // Repeated values are not recorded again.
    let rows = widget.history.samples(&WindowKind::FiveHour, 0);
    let pcts: Vec<f32> = rows.iter().map(|s| s.pct).collect();
    assert_eq!(pcts, [50.0, 70.0, 81.0, 85.0, 90.0, 96.0, 97.0]);
    // An unchanged capture on a later tick changes nothing.
    let again = widget.tick(T0 + 50 * MINUTE_MS);
    assert_eq!(again.toasts(), 0);
}

#[test]
fn restart_keeps_state_and_repeats_no_toast() {
    let env = Env::new();
    let reset = T0 + 3 * HOUR_MS;
    env.append_transcript(
        SID,
        &[
            identity_line(SID, "claude-opus-5-5"),
            prompt_line(SID, T0 - 10 * MINUTE_MS),
            assistant_line(SID, "claude-opus-5-5", 170_000, T0, false),
        ],
    );
    let capture = Statusline {
        model: Some(("claude-opus-5-5", "Opus 5.5")),
        context: Some((85.0, 200_000)),
        ..five_hour(82.0, reset)
    };
    env.statusline(&capture, T0);

    let mut widget = Widget::start(&env);
    let first = widget.tick(T0 + 1000);
    assert_eq!(thresholds(&first.limit), [(WindowKind::FiveHour, 80)]);
    assert_eq!(first.ctx.len(), 1, "context crossed 80%: {:?}", first.ctx);
    assert_eq!(first.ctx[0].threshold, 80);
    assert_eq!(first.ctx[0].model.as_deref(), Some("Opus 5.5"));
    let session = first.snap.session.as_ref().unwrap();
    assert_eq!(
        (session.ctx_pct, session.ctx_basis, session.ctx_is_estimate),
        (Some(85.0), CtxBasis::Statusline, false)
    );
    let rows_before = widget.history.rows().to_vec();

    let mut widget = widget.restart(&env);
    for i in 1..4 {
        let tick = widget.tick(T0 + i * MINUTE_MS);
        assert_eq!(tick.toasts(), 0, "restart repeated a toast: {tick:?}");
        assert_eq!(tick.five_hour().state.pct, 82.0);
    }
    assert_eq!(widget.history.rows(), rows_before.as_slice(), "nothing new to record");

    // The same instance going higher still alerts for the next threshold.
    env.statusline(&Statusline { api_ms: 2, context: Some((91.0, 200_000)), ..capture.clone() }, T0 + 5 * MINUTE_MS);
    env.append_transcript(SID, &[assistant_line(SID, "claude-opus-5-5", 182_000, T0 + 5 * MINUTE_MS, false)]);
    let tick = widget.tick(T0 + 5 * MINUTE_MS + 1000);
    assert!(thresholds(&tick.limit).is_empty(), "82 → 82: nothing new: {:?}", tick.limit);
    assert_eq!(tick.ctx.iter().map(|e| e.threshold).collect::<Vec<_>>(), [90]);
}

#[test]
fn clock_jumping_backwards_neither_panics_nor_duplicates() {
    let env = Env::new();
    let reset = T0 + 4 * HOUR_MS;
    let later = T0 + HOUR_MS;
    env.statusline(&five_hour(83.0, reset), later);
    let mut widget = Widget::start(&env);
    let tick = widget.tick(later);
    assert_eq!(thresholds(&tick.limit), [(WindowKind::FiveHour, 80)]);
    let rows = widget.history.rows().len();

    // The clock is set back an hour: the capture now lies in the future and is ignored.
    let back = widget.tick(T0);
    assert_eq!(back.toasts(), 0, "{back:?}");
    assert!(back.window(&WindowKind::FiveHour).is_none(), "a capture from the future is not shown");
    // While the clock is behind, the shim writes captures stamped with the earlier time.
    env.statusline(&Statusline { api_ms: 2, ..five_hour(84.0, reset) }, T0 + MINUTE_MS);
    let tick = widget.tick(T0 + MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 84.0);
    assert_eq!(tick.toasts(), 0, "same window, already alerted: {tick:?}");
    assert_eq!(widget.history.rows().len(), rows, "a row older than the newest is not recorded");

    // The clock is corrected again.
    env.statusline(&Statusline { api_ms: 3, ..five_hour(85.0, reset) }, later + MINUTE_MS);
    let tick = widget.tick(later + MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 85.0);
    assert_eq!(tick.toasts(), 0, "{tick:?}");
    let t: Vec<Ms> = widget.history.rows().iter().map(|r| r.t).collect();
    assert!(t.windows(2).all(|w| w[0] <= w[1]), "history stays sorted: {t:?}");
}

#[test]
fn clock_jumping_past_the_reset_shows_zero_and_rearms_thresholds() {
    let env = Env::new();
    let reset = T0 + HOUR_MS;
    env.statusline(&five_hour(85.0, reset), T0);
    let mut widget = Widget::start(&env);
    assert_eq!(thresholds(&widget.tick(T0).limit), [(WindowKind::FiveHour, 80)]);

    // Sleep through the reset.
    let mut reset_toasts = 0;
    for now in [T0 + 2 * HOUR_MS, T0 + 2 * HOUR_MS + MINUTE_MS] {
        let tick = widget.tick(now);
        let w = tick.five_hour();
        assert_eq!((w.state.phase, w.state.pct), (Phase::ResetAwaitingData, 0.0));
        assert!(thresholds(&tick.limit).is_empty());
        reset_toasts += resets(&tick.limit).len();
    }
    assert_eq!(reset_toasts, 1, "one reset toast");
    assert!(
        widget.history.samples(&WindowKind::FiveHour, 0).iter().all(|s| s.pct == 85.0),
        "the 0% placeholder is not history"
    );

    // Claude Code is used again: a new window.
    let next_reset = T0 + 7 * HOUR_MS;
    let now = T0 + 2 * HOUR_MS + 5 * MINUTE_MS;
    env.statusline(&Statusline { api_ms: 2, ..five_hour(12.0, next_reset) }, now);
    let tick = widget.tick(now);
    assert_eq!((tick.five_hour().state.phase, tick.five_hour().state.pct), (Phase::Active, 12.0));
    assert!(tick.limit.is_empty(), "{:?}", tick.limit);
    env.statusline(&Statusline { api_ms: 3, ..five_hour(81.0, next_reset) }, now + 5 * MINUTE_MS);
    let tick = widget.tick(now + 5 * MINUTE_MS);
    assert_eq!(thresholds(&tick.limit), [(WindowKind::FiveHour, 80)], "thresholds re-armed for the new window");
}

#[test]
fn weekly_reset_after_three_days_is_detected_from_desktop_and_recapped_once() {
    let env = Env::new();
    // Desktop samples every 30 min for three days: the weekly % climbs to 70, then the weekly
    // window resets early (Anthropic sometimes resets after 2-3 days).
    let start = T0 - 3 * DAY_MS;
    let mut samples: Vec<(Ms, Vec<(&str, f64)>)> = Vec::new();
    let steps = 3 * 48;
    for i in 0..steps {
        let sd = 10.0 + (60.0 * i as f64 / steps as f64).floor();
        samples.push((start + i * 30 * MINUTE_MS, vec![("fh", 20.0), ("sd", sd)]));
    }
    let before_drop = samples.last().unwrap().0;
    let write = |samples: &[(Ms, Vec<(&str, f64)>)]| {
        let refs: Vec<(Ms, &[(&str, f64)])> = samples.iter().map(|(t, u)| (*t, u.as_slice())).collect();
        env.write_desktop(&refs);
    };
    write(&samples);

    let mut widget = Widget::start(&env);
    let tick = widget.tick(before_drop + MINUTE_MS);
    assert_eq!(tick.seven_day().state.pct, 69.0);
    assert_eq!(tick.seven_day().state.source, Source::Desktop);
    assert!(tick.recap.is_none());

    let drop_at = before_drop + 30 * MINUTE_MS;
    samples.push((drop_at, vec![("fh", 21.0), ("sd", 3.0)]));
    write(&samples);
    let tick = widget.tick(drop_at + MINUTE_MS);
    let w = tick.seven_day();
    assert_eq!(w.state.pct, 3.0);
    assert!(
        matches!(w.state.reset, ResetInfo::Estimated { confidence: Confidence::Low, .. }),
        "weekly estimates are low confidence: {:?}",
        w.state.reset
    );
    assert_eq!(resets(&tick.limit), [WindowKind::SevenDay]);
    let recap = tick.recap.as_ref().expect("weekly recap");
    assert_eq!(recap.window_end_ms, drop_at);
    assert_eq!(recap.used_pct, 69.0);

    let view = widget.history.view(
        &WindowKind::SevenDay,
        &ViewRange { from_ms: start, to_ms: drop_at + HOUR_MS, now_ms: drop_at + MINUTE_MS, day_starts: &[] },
    );
    assert_eq!(view.resets_ms, [drop_at]);

    let again = widget.tick(drop_at + 2 * MINUTE_MS);
    assert_eq!(again.toasts(), 0, "{again:?}");
    let mut widget = widget.restart(&env);
    let after = widget.tick(drop_at + 3 * MINUTE_MS);
    assert_eq!(after.toasts(), 0, "restart repeated the recap or reset: {after:?}");
}

#[test]
fn early_reset_seen_by_desktop_replaces_the_cli_window() {
    let env = Env::new();
    let cli_reset = T0 + 4 * HOUR_MS;
    env.statusline(&five_hour(60.0, cli_reset), T0);
    let mut widget = Widget::start(&env);
    let tick = widget.tick(T0 + MINUTE_MS);
    assert_eq!(tick.five_hour().state.reset, ResetInfo::Exact { at_ms: cli_reset });

    // Desktop's next sample shows 5%: the window reset early.
    let seen = T0 + 30 * MINUTE_MS;
    env.write_desktop(&[(T0 - 15 * MINUTE_MS, &[("fh", 60.0)]), (seen, &[("fh", 5.0)])]);
    let tick = widget.tick(seen + MINUTE_MS);
    let w = tick.five_hour();
    assert_eq!((w.state.pct, w.state.source, w.state.phase), (5.0, Source::Desktop, Phase::Active));
    assert!(!w.state.reset.is_exact(), "the ended window's exact reset is not shown: {:?}", w.state.reset);
    assert_eq!(resets(&tick.limit), [WindowKind::FiveHour]);
    assert_eq!(widget.tick(seen + 2 * MINUTE_MS).toasts(), 0);
}

#[test]
fn cli_decimals_and_desktop_integers_are_one_window() {
    let env = Env::new();
    let reset = T0 + 2 * HOUR_MS;
    env.statusline(&five_hour(80.2, reset), T0);
    let mut widget = Widget::start(&env);
    assert_eq!(thresholds(&widget.tick(T0).limit), [(WindowKind::FiveHour, 80)]);

    // Desktop rounds down and lags: 79 is not a reset.
    env.write_desktop(&[(T0 + 15 * MINUTE_MS, &[("fh", 79.0)])]);
    let tick = widget.tick(T0 + 16 * MINUTE_MS);
    let w = tick.five_hour();
    assert_eq!((w.state.pct, w.state.source), (79.0, Source::Desktop));
    assert_eq!(w.state.reset, ResetInfo::Exact { at_ms: reset }, "the CLI's exact reset still applies");
    assert!(tick.limit.is_empty(), "no reset, no repeated threshold: {:?}", tick.limit);

    // Back above 80 in the same window: no second toast.
    env.statusline(&Statusline { api_ms: 2, ..five_hour(80.6, reset) }, T0 + 20 * MINUTE_MS);
    let tick = widget.tick(T0 + 20 * MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 80.6);
    assert!(tick.limit.is_empty(), "{:?}", tick.limit);

    let view = widget.history.view(
        &WindowKind::FiveHour,
        &ViewRange { from_ms: T0 - HOUR_MS, to_ms: T0 + HOUR_MS, now_ms: T0 + 20 * MINUTE_MS, day_starts: &[] },
    );
    assert!(view.resets_ms.is_empty(), "{:?}", view.resets_ms);
}

#[test]
fn several_sessions_report_the_account_maximum() {
    let env = Env::new();
    let reset = T0 + 2 * HOUR_MS;
    // An idle session still holds an older, lower value of the same window.
    env.statusline(&Statusline { five_hour: Some((30.0, reset)), ..Statusline::new(SID2) }, T0 - 30 * MINUTE_MS);
    env.statusline(&five_hour(45.0, reset + 60_000), T0);
    let mut widget = Widget::start(&env);
    let tick = widget.tick(T0 + MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 45.0);
    assert_eq!(tick.five_hour().state.reset, ResetInfo::Exact { at_ms: reset + 60_000 });
}

#[test]
fn one_million_context_follows_model_switches() {
    let env = Env::new();
    env.append_transcript(
        SID,
        &[
            identity_line(SID, "claude-opus-5-5[1m]"),
            prompt_line(SID, T0),
            assistant_line(SID, "claude-opus-5-5", 300_000, T0 + MINUTE_MS, true),
        ],
    );
    let mut widget = Widget::start(&env);
    let tick = widget.tick(T0 + 2 * MINUTE_MS);
    let s = tick.snap.session.as_ref().unwrap();
    assert_eq!((s.ctx_size, s.ctx_basis, s.ctx_pct), (1_000_000, CtxBasis::Identity, Some(30.0)));
    assert_eq!(s.model_id.as_deref(), Some("claude-opus-5-5"));

    // `/model` to a 200K model: the head's 1M marker no longer applies.
    env.append_transcript(
        SID,
        &[
            prompt_line(SID, T0 + 3 * MINUTE_MS),
            assistant_line(SID, "claude-sonnet-5", 150_000, T0 + 4 * MINUTE_MS, true),
        ],
    );
    let tick = widget.tick(T0 + 5 * MINUTE_MS);
    let s = tick.snap.session.as_ref().unwrap();
    assert_eq!((s.ctx_size, s.ctx_basis, s.ctx_pct), (200_000, CtxBasis::Default, Some(75.0)));
    assert!(tick.ctx.is_empty(), "a % guessed over the 200K default never alerts: {:?}", tick.ctx);

    // And back to the 1M model.
    env.append_transcript(
        SID,
        &[
            prompt_line(SID, T0 + 6 * MINUTE_MS),
            assistant_line(SID, "claude-opus-5-5", 320_000, T0 + 7 * MINUTE_MS, true),
        ],
    );
    let tick = widget.tick(T0 + 8 * MINUTE_MS);
    let s = tick.snap.session.as_ref().unwrap();
    assert_eq!((s.ctx_size, s.ctx_basis, s.ctx_pct), (1_000_000, CtxBasis::Identity, Some(32.0)));
    assert!(tick.ctx.is_empty(), "32% of 1M is not 80%: {:?}", tick.ctx);
}

#[test]
fn long_turn_is_reported_once_and_not_after_a_restart() {
    let env = Env::new();
    env.append_transcript(
        SID,
        &[prompt_line(SID, T0 - HOUR_MS), assistant_line(SID, "claude-opus-5-5", 50_000, T0 - HOUR_MS + 1000, true)],
    );
    let mut widget = Widget::start(&env);
    assert!(widget.tick(T0).finished.is_empty(), "turns that ended before the start are not reported");

    env.append_transcript(
        SID,
        &[
            prompt_line(SID, T0 + MINUTE_MS),
            assistant_line(SID, "claude-opus-5-5", 60_000, T0 + 3 * MINUTE_MS, false),
            assistant_line(SID, "claude-opus-5-5", 70_000, T0 + 6 * MINUTE_MS, true),
        ],
    );
    let tick = widget.tick(T0 + 6 * MINUTE_MS + 1000);
    assert_eq!(tick.finished.len(), 1, "{:?}", tick.finished);
    assert_eq!(tick.finished[0].duration_ms, 5 * MINUTE_MS);
    assert_eq!(tick.finished[0].project.as_deref(), Some("proj"));
    assert!(widget.tick(T0 + 7 * MINUTE_MS).finished.is_empty());
    let mut widget = widget.restart(&env);
    assert!(widget.tick(T0 + 8 * MINUTE_MS).finished.is_empty());
}

#[test]
fn damaged_desktop_file_keeps_the_last_good_value() {
    let env = Env::new();
    env.write_desktop(&[(T0, &[("fh", 40.0), ("sd", 50.0)])]);
    let mut widget = Widget::start(&env);
    assert_eq!(widget.tick(T0 + MINUTE_MS).five_hour().state.pct, 40.0);

    // Desktop is mid-write: the file is cut short.
    let good = std::fs::read(env.desktop_file()).unwrap();
    env.write_desktop_raw(&good[..good.len() / 2]);
    let tick = widget.tick(T0 + 2 * MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 40.0);
    assert!(matches!(widget.desktop_health, DesktopHealth::Ok { .. }));
    assert_eq!(tick.toasts(), 0);

    // A newer Desktop release bumped the version but samples still parse: read best-effort, and
    // the health says so.
    let t = T0 + 2 * MINUTE_MS;
    env.write_desktop_raw(
        format!(r#"{{"version":3,"samples":[{{"t":{t},"u":{{"fh":42,"sd":51}},"new_field":true}}]}}"#).as_bytes(),
    );
    let tick = widget.tick(T0 + 3 * MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 42.0);
    assert_eq!(widget.desktop_health, DesktopHealth::Ok { last_sample_ms: Some(t), newer_version: Some(3) });

    // A newer Desktop release changed the format beyond recognition: shown as such, no stale numbers.
    env.write_desktop_raw(br#"{"version":3,"records":[]}"#);
    let tick = widget.tick(T0 + 4 * MINUTE_MS);
    assert_eq!(widget.desktop_health, DesktopHealth::SchemaChanged { version: 3 });
    assert!(tick.snap.windows.is_empty(), "{:?}", tick.snap.windows);
    assert_eq!(tick.toasts(), 0);
}

#[test]
fn oversized_desktop_file_is_refused_not_read() {
    let env = Env::new();
    let mut huge = br#"{"version":2,"samples":[],"pad":""#.to_vec();
    huge.resize(17 * 1024 * 1024, b'x');
    huge.extend_from_slice(b"\"}");
    env.write_desktop_raw(&huge);
    let mut widget = Widget::start(&env);
    let tick = widget.tick(T0);
    assert_eq!(widget.desktop_health, DesktopHealth::Unreadable);
    assert!(tick.snap.windows.is_empty());
}

#[test]
fn bad_capture_files_are_skipped_and_good_ones_still_count() {
    let env = Env::new();
    let reset = T0 + 2 * HOUR_MS;
    let dir = env.paths.capture_dir();
    // An otherwise valid capture of another session at 90%, padded past the 64 KiB cap: if it
    // were read, the account maximum would show 90% and fire the 80% alert.
    env.statusline(&Statusline { five_hour: Some((90.0, reset)), ..Statusline::new(SID2) }, T0);
    let mut big = std::fs::read(dir.join(format!("{SID2}.json"))).unwrap();
    big.pop();
    big.extend(std::iter::repeat_n(b' ', 70 * 1024));
    big.push(b'}');
    assert!(serde_json::from_slice::<serde_json::Value>(&big).is_ok(), "still valid JSON, only too big");
    std::fs::write(dir.join(format!("{SID2}.json")), &big).unwrap();
    env.statusline(&five_hour(33.0, reset), T0);
    let good = std::fs::read(dir.join(format!("{SID}.json"))).unwrap();
    // Truncated, garbage, a stray temp file and an old log: none may break loading.
    std::fs::write(dir.join("33333333-3333-4333-8333-333333333333.json"), &good[..good.len() / 2]).unwrap();
    std::fs::write(dir.join("44444444-4444-4444-8444-444444444444.json"), [0xff, 0xfe, 0x00, 0x7b]).unwrap();
    std::fs::write(dir.join(".55555555-5555-4555-8555-555555555555.1.2.tmp"), b"{").unwrap();
    std::fs::write(dir.join("_shim.log"), b"not json").unwrap();

    let mut widget = Widget::start(&env);
    let tick = widget.tick(T0 + MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 33.0, "the oversized 90% capture was read");
    assert!(tick.limit.is_empty(), "{:?}", tick.limit);
    assert_eq!(tick.snap.health.cli_last_capture_ms, Some(T0));
}

#[test]
fn huge_transcript_line_keeps_the_session_visible() {
    let env = Env::new();
    env.append_transcript(
        SID,
        &[
            identity_line(SID, "claude-opus-5-5"),
            prompt_line(SID, T0),
            assistant_line(SID, "claude-opus-5-5", 90_000, T0 + MINUTE_MS, false),
        ],
    );
    let mut widget = Widget::start(&env);
    let s = widget.tick(T0 + 2 * MINUTE_MS).snap.session.unwrap();
    assert_eq!(s.ctx_tokens, Some(90_000));

    // A pasted 2 MiB tool result follows: no assistant line fits in the tail scan any more.
    let huge = serde_json::json!({ "type": "user", "message": { "role": "user", "content": "x".repeat(2 << 20) } });
    env.append_transcript(SID, &[huge]);
    let tick = widget.tick(T0 + 3 * MINUTE_MS);
    let s = tick.snap.session.as_ref().expect("session still shown");
    assert_eq!((s.ctx_tokens, s.model_id.as_deref()), (Some(90_000), Some("claude-opus-5-5")));
}

#[test]
fn damaged_history_file_loads_and_keeps_recording() {
    let env = Env::new();
    let history = env.paths.history_file();
    std::fs::create_dir_all(history.parent().unwrap()).unwrap();
    let row = format!(r#"{{"t":{},"w":"5h","p":20.0,"r":null,"s":"desktop","e":false}}"#, T0 - HOUR_MS);
    std::fs::write(&history, format!("\u{FEFF}{row}\nnot json\n\x00\x01\n{{\"t\":17").as_bytes()).unwrap();

    env.statusline(&five_hour(25.0, T0 + HOUR_MS), T0);
    let mut widget = Widget::start(&env);
    assert_eq!(widget.history.rows().len(), 1);
    let tick = widget.tick(T0 + MINUTE_MS);
    assert_eq!(tick.five_hour().state.pct, 25.0);
    let widget = widget.restart(&env);
    let pcts: Vec<f32> = widget.history.samples(&WindowKind::FiveHour, 0).iter().map(|s| s.pct).collect();
    assert_eq!(pcts, [20.0, 25.0], "the torn line did not swallow the new row");
}

#[test]
fn capture_files_hold_no_transcript_path_or_version() {
    let env = Env::new();
    env.statusline(&Statusline { model: Some(("claude-opus-5-5", "Opus 5.5")), ..five_hour(10.0, T0 + HOUR_MS) }, T0);
    let text = std::fs::read_to_string(env.paths.capture_dir().join(format!("{SID}.json"))).unwrap();
    for gone in ["transcript_path", "tester", "cc_version", "2.3.4", "cwd", "total_cost_usd"] {
        assert!(!text.contains(gone), "{gone:?} in {text}");
    }
}
