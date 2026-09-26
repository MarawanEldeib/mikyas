//! Picks which session the widget header shows.
//!
//! - Candidates: all tails. The newest `last_assistant_ms` wins.
//! - Tie-break: tails whose `last_assistant_ms` is within [`FOCUS_TIE_MS`] of the newest are
//!   tied; among them prefer the one whose `session_id` equals the `cli_session_id` of the Desktop
//!   session with the most recent `last_focused_ms`, if that focus happened within
//!   [`FOCUS_TIE_MS`] of `now_ms`. Otherwise keep the newest.
//! - `concurrent` = number of distinct sessions with a tail whose `last_assistant_ms >= now_ms -
//!   CONCURRENT_WINDOW_MS` (saturating at 255). Tails are the same session when they share a
//!   non-empty `session_id` (e.g. a Cowork copy of a transcript), else when they share a path.
//! - Empty input → `None`.

use std::collections::HashSet;
use std::path::Path;

use crate::sources::desktop_sessions::DesktopSession;
use crate::sources::transcript::TranscriptTail;
use crate::time::{MINUTE_MS, Ms};

pub const FOCUS_TIE_MS: Ms = 2 * MINUTE_MS;
pub const CONCURRENT_WINDOW_MS: Ms = 10 * MINUTE_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivePick {
    /// Index into the `tails` slice.
    pub index: usize,
    pub concurrent: u8,
}

/// Chooses the session to display; see the module docs for the rules. On exact timestamp ties
/// the earlier slice element wins, so the result is deterministic.
pub fn pick(tails: &[TranscriptTail], desktop_sessions: &[DesktopSession], now_ms: Ms) -> Option<ActivePick> {
    let newest = newest_index(tails.iter().enumerate())?;
    let newest_ms = tails[newest].last_assistant_ms;

    let index = recently_focused_cli_id(desktop_sessions, now_ms)
        .and_then(|focused_id| {
            let tie_floor = newest_ms.saturating_sub(FOCUS_TIE_MS);
            newest_index(
                tails
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.last_assistant_ms >= tie_floor && t.session_id == focused_id),
            )
        })
        .unwrap_or(newest);

    let active_floor = now_ms.saturating_sub(CONCURRENT_WINDOW_MS);
    let active: HashSet<SessionId<'_>> = tails
        .iter()
        .filter(|t| t.last_assistant_ms >= active_floor)
        .map(SessionId::of)
        .collect();
    let active = active.len();
    Some(ActivePick {
        index,
        concurrent: u8::try_from(active).unwrap_or(u8::MAX),
    })
}

/// What makes two tails the same session (see the module docs).
#[derive(PartialEq, Eq, Hash)]
enum SessionId<'a> {
    Id(&'a str),
    Path(&'a Path),
}

impl<'a> SessionId<'a> {
    fn of(tail: &'a TranscriptTail) -> Self {
        if tail.session_id.is_empty() {
            SessionId::Path(&tail.path)
        } else {
            SessionId::Id(&tail.session_id)
        }
    }
}

/// Index of the tail with the largest `last_assistant_ms`; the first one wins exact ties.
fn newest_index<'a>(tails: impl Iterator<Item = (usize, &'a TranscriptTail)>) -> Option<usize> {
    let mut best: Option<(usize, Ms)> = None;
    for (i, t) in tails {
        if best.is_none_or(|(_, ms)| t.last_assistant_ms > ms) {
            best = Some((i, t.last_assistant_ms));
        }
    }
    best.map(|(i, _)| i)
}

/// `cli_session_id` of the most recently focused Desktop session, if that focus is recent enough.
fn recently_focused_cli_id(sessions: &[DesktopSession], now_ms: Ms) -> Option<&str> {
    let mut best: Option<(&DesktopSession, Ms)> = None;
    for s in sessions {
        let Some(focused) = s.last_focused_ms else { continue };
        if best.is_none_or(|(_, ms)| focused > ms) {
            best = Some((s, focused));
        }
    }
    let (session, focused_ms) = best?;
    // "Within" on both sides: a far-future focus (corrupt file, unit mix-up) is not recent.
    if now_ms.abs_diff(focused_ms) > FOCUS_TIE_MS.unsigned_abs() {
        return None;
    }
    session.cli_session_id.as_deref().filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::engine::types::Entrypoint;
    use crate::time::SECOND_MS;

    const NOW: Ms = 1_790_208_000_000;

    fn tail(id: &str, last_ms: Ms) -> TranscriptTail {
        TranscriptTail {
            path: PathBuf::from(format!("{id}.jsonl")),
            session_id: id.to_string(),
            entrypoint: Entrypoint::Cli,
            model_id: Some("claude-opus-5-5".into()),
            ctx_tokens: 1_000,
            max_ctx_tokens_seen: 1_000,
            identity_1m: None,
            last_assistant_ms: last_ms,
            project: Some("proj".into()),
            turn: Default::default(),
        }
    }

    fn desk(id: Option<&str>, focused: Option<Ms>) -> DesktopSession {
        DesktopSession {
            cli_session_id: id.map(str::to_string),
            model: None,
            last_focused_ms: focused,
            last_activity_ms: focused,
        }
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(pick(&[], &[desk(Some("a"), Some(NOW))], NOW), None);
    }

    #[test]
    fn newest_wins() {
        let tails = [tail("a", NOW - 50 * MINUTE_MS), tail("b", NOW - SECOND_MS), tail("c", NOW - MINUTE_MS)];
        assert_eq!(pick(&tails, &[], NOW).map(|p| p.index), Some(1));
    }

    #[test]
    fn concurrent_counts_distinct_sessions() {
        // The same session seen through two files (e.g. a Cowork copy) is one session.
        let mut copy = tail("a", NOW - MINUTE_MS);
        copy.path = PathBuf::from("cowork/a.jsonl");
        let tails = [tail("a", NOW - SECOND_MS), copy, tail("b", NOW - 2 * MINUTE_MS)];
        assert_eq!(pick(&tails, &[], NOW).map(|p| p.concurrent), Some(2));
        // Tails without a session id are told apart by their path.
        let (mut x, mut y) = (tail("", NOW - SECOND_MS), tail("", NOW - SECOND_MS));
        x.path = PathBuf::from("x.jsonl");
        y.path = PathBuf::from("y.jsonl");
        assert_eq!(pick(&[x, y], &[], NOW).map(|p| p.concurrent), Some(2));
    }

    #[test]
    fn exact_tie_keeps_first() {
        let tails = [tail("a", NOW - MINUTE_MS), tail("b", NOW - MINUTE_MS)];
        assert_eq!(pick(&tails, &[], NOW).map(|p| p.index), Some(0));
    }

    #[test]
    fn recent_focus_breaks_tie() {
        let tails = [tail("a", NOW - SECOND_MS), tail("b", NOW - 90 * SECOND_MS)];
        let desktop = [
            desk(Some("a"), Some(NOW - 10 * MINUTE_MS)),
            desk(Some("b"), Some(NOW - 30 * SECOND_MS)),
        ];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(1));
    }

    #[test]
    fn focus_only_applies_within_tie_window() {
        // "b" is focused but more than 2 minutes older than the newest tail.
        let tails = [tail("a", NOW - SECOND_MS), tail("b", NOW - SECOND_MS - FOCUS_TIE_MS - 1)];
        let desktop = [desk(Some("b"), Some(NOW - SECOND_MS))];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(0));
        // Exactly at the tie boundary it still counts.
        let tails = [tail("a", NOW - SECOND_MS), tail("b", NOW - SECOND_MS - FOCUS_TIE_MS)];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(1));
    }

    #[test]
    fn stale_focus_is_ignored() {
        let tails = [tail("a", NOW - SECOND_MS), tail("b", NOW - 30 * SECOND_MS)];
        let desktop = [desk(Some("b"), Some(NOW - FOCUS_TIE_MS - 1))];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(0));
        let desktop = [desk(Some("b"), Some(NOW - FOCUS_TIE_MS))];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(1), "boundary is inclusive");
    }

    #[test]
    fn future_focus_must_also_be_within_the_window() {
        // A focus timestamp far in the future (corrupt file, unit mix-up) is not "within
        // FOCUS_TIE_MS of now" and must not pin the preference forever.
        let tails = [tail("a", NOW - SECOND_MS), tail("b", NOW - 30 * SECOND_MS)];
        let desktop = [desk(Some("b"), Some(NOW + FOCUS_TIE_MS + 1))];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(0));
        // A small skew into the future is still within the window.
        let desktop = [desk(Some("b"), Some(NOW + FOCUS_TIE_MS))];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(1));
    }

    #[test]
    fn only_most_recent_focus_counts() {
        // The most recently focused session has no matching tail: no preference at all,
        // even though an older focus matches a tied tail.
        let tails = [tail("a", NOW - SECOND_MS), tail("b", NOW - 30 * SECOND_MS)];
        let desktop = [
            desk(Some("b"), Some(NOW - 60 * SECOND_MS)),
            desk(Some("zzz"), Some(NOW - 5 * SECOND_MS)),
            desk(Some("b"), None),
        ];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(0));
        // A focused session without a CLI id gives no preference either.
        let desktop = [desk(Some("b"), Some(NOW - 60 * SECOND_MS)), desk(None, Some(NOW - SECOND_MS))];
        assert_eq!(pick(&tails, &desktop, NOW).map(|p| p.index), Some(0));
    }

    #[test]
    fn concurrent_counts_recent_tails() {
        let tails = [
            tail("a", NOW - SECOND_MS),
            tail("b", NOW - CONCURRENT_WINDOW_MS),
            tail("c", NOW - CONCURRENT_WINDOW_MS - 1),
            tail("d", NOW - 3 * MINUTE_MS),
        ];
        assert_eq!(pick(&tails, &[], NOW).map(|p| p.concurrent), Some(3));
        let old = [tail("a", NOW - 60 * MINUTE_MS)];
        assert_eq!(pick(&old, &[], NOW), Some(ActivePick { index: 0, concurrent: 0 }));
    }

    #[test]
    fn concurrent_saturates() {
        let tails: Vec<_> = (0..300).map(|i| tail(&format!("s{i}"), NOW - i)).collect();
        assert_eq!(pick(&tails, &[], NOW).map(|p| p.concurrent), Some(255));
    }
}
