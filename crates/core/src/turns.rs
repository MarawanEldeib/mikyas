//! "Claude finished" detection from transcripts (no hooks, no message content).
//!
//! A turn = from the last HUMAN user line (a `type: "user"` line whose content is not only
//! tool_result blocks — read only the block `type` fields, never text) to the final assistant
//! line of that turn with `message.stop_reason == "end_turn"`. Its duration is end − start; when
//! the start is outside the scanned tail, the earliest timestamp in the tail is a lower bound.
//!
//! [`FinishedTurns::observe`] reports a turn once when: it ended after the app started
//! (`started_ms`) and within the last [`RECENT_MS`] (no startup flood) but not more than
//! `FUTURE_SLACK_MS` in the future (clock skew, corrupt lines), it lasted at least
//! `min_duration_ms`, and it was not reported before (dedupe by session key + end timestamp,
//! kept in memory only — reporting only turns that end after app start makes persistence
//! unnecessary). Sidechain/subagent lines never end a turn.

use std::collections::HashMap;

use crate::engine::types::Entrypoint;
use crate::time::{FUTURE_SLACK_MS, MINUTE_MS, Ms};

pub const RECENT_MS: Ms = 10 * MINUTE_MS;

/// Turn information extracted by the transcript tail scan (see `sources::transcript`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnInfo {
    /// Timestamp of the final `end_turn` assistant line, if the tail ends with a finished turn.
    pub ended_ms: Option<Ms>,
    /// Start of that turn (last human prompt), or the earliest timestamp in the scanned tail.
    pub started_ms: Option<Ms>,
    /// True when `started_ms` is only a lower bound (the prompt was outside the scanned tail).
    pub start_is_lower_bound: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinishedTurn {
    /// `SessionView.key` of the session.
    pub key: String,
    pub duration_ms: Ms,
    pub model: Option<String>,
    pub project: Option<String>,
    pub entrypoint: Entrypoint,
}

#[derive(Debug, Default)]
pub struct FinishedTurns {
    reported: HashMap<String, Ms>,
}

impl FinishedTurns {
    /// Reports the turns that finished since the last call (see the module docs). `sessions`
    /// pairs each session's view data (its `duration_ms` is ignored) with its latest turn info.
    pub fn observe(
        &mut self,
        sessions: &[(FinishedTurn, TurnInfo)],
        started_ms: Ms,
        min_duration_ms: Ms,
        now_ms: Ms,
    ) -> Vec<FinishedTurn> {
        let recent_floor = now_ms.saturating_sub(RECENT_MS);
        // Older turns can never qualify again, so their dedupe entries are dropped.
        self.reported.retain(|_, ended| *ended >= recent_floor);
        let mut out = Vec::new();
        for (session, turn) in sessions {
            let (Some(ended), Some(start)) = (turn.ended_ms, turn.started_ms) else {
                continue;
            };
            // A far-future end (clock skew, corrupt line) would also block the session's later
            // turns in the dedupe map.
            if ended <= started_ms || ended < recent_floor || ended > now_ms.saturating_add(FUTURE_SLACK_MS) {
                continue;
            }
            let duration_ms = ended.saturating_sub(start);
            if duration_ms < min_duration_ms {
                continue;
            }
            if self.reported.get(&session.key).is_some_and(|&last| last >= ended) {
                continue;
            }
            self.reported.insert(session.key.clone(), ended);
            out.push(FinishedTurn { duration_ms, ..session.clone() });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const START: Ms = 1_790_251_200_000; // app start
    const MIN: Ms = 3 * MINUTE_MS;

    fn session(key: &str) -> FinishedTurn {
        FinishedTurn {
            key: key.into(),
            duration_ms: 0,
            model: Some("Opus 5.5".into()),
            project: None,
            entrypoint: Entrypoint::Cli,
        }
    }

    fn turn(started_ms: Ms, ended_ms: Ms) -> TurnInfo {
        TurnInfo { ended_ms: Some(ended_ms), started_ms: Some(started_ms), start_is_lower_bound: false }
    }

    fn keys(done: &[FinishedTurn]) -> Vec<(&str, Ms)> {
        done.iter().map(|t| (t.key.as_str(), t.duration_ms)).collect()
    }

    #[test]
    fn reports_a_long_turn_once() {
        let mut f = FinishedTurns::default();
        let end = START + 20 * MINUTE_MS;
        let input = [(session("a"), turn(end - 12 * MINUTE_MS, end))];
        let done = f.observe(&input, START, MIN, end + 30_000);
        assert_eq!(keys(&done), vec![("a", 12 * MINUTE_MS)]);
        assert_eq!(done[0].model.as_deref(), Some("Opus 5.5"));
        // Every later tick sees the same turn: no repeat.
        assert!(f.observe(&input, START, MIN, end + 60_000).is_empty());
        assert!(f.observe(&input, START, MIN, end + 5 * MINUTE_MS).is_empty());
    }

    #[test]
    fn next_turn_of_the_same_session_is_reported() {
        let mut f = FinishedTurns::default();
        let end = START + 20 * MINUTE_MS;
        let first = [(session("a"), turn(end - 5 * MINUTE_MS, end))];
        assert_eq!(f.observe(&first, START, MIN, end).len(), 1);
        let end2 = end + 10 * MINUTE_MS;
        let second = [(session("a"), turn(end2 - 4 * MINUTE_MS, end2))];
        assert_eq!(keys(&f.observe(&second, START, MIN, end2)), vec![("a", 4 * MINUTE_MS)]);
    }

    #[test]
    fn short_turns_are_skipped() {
        let mut f = FinishedTurns::default();
        let end = START + 20 * MINUTE_MS;
        let exactly = [(session("a"), turn(end - MIN, end))];
        let short = [(session("b"), turn(end - MIN + 1, end))];
        assert!(f.observe(&short, START, MIN, end).is_empty());
        assert_eq!(f.observe(&exactly, START, MIN, end).len(), 1, "the minimum itself counts");
    }

    #[test]
    fn turns_ending_before_app_start_are_skipped() {
        let mut f = FinishedTurns::default();
        let at_start = [(session("a"), turn(START - 10 * MINUTE_MS, START))];
        assert!(f.observe(&at_start, START, MIN, START + 1_000).is_empty());
        let after = [(session("a"), turn(START - 10 * MINUTE_MS, START + 1))];
        assert_eq!(f.observe(&after, START, MIN, START + 1_000).len(), 1);
    }

    #[test]
    fn old_turns_are_skipped() {
        let mut f = FinishedTurns::default();
        let end = START + MINUTE_MS;
        let input = [(session("a"), turn(end - 5 * MINUTE_MS, end))];
        // A tick after a long sleep: the turn ended more than RECENT_MS ago.
        assert!(f.observe(&input, START, MIN, end + RECENT_MS + 1).is_empty());
        assert_eq!(f.observe(&input, START, MIN, end + RECENT_MS).len(), 1);
    }

    #[test]
    fn unfinished_or_unknown_turns_are_skipped() {
        let mut f = FinishedTurns::default();
        let now = START + 30 * MINUTE_MS;
        let input = [
            (session("running"), TurnInfo::default()),
            (session("no_start"), TurnInfo { ended_ms: Some(now), started_ms: None, start_is_lower_bound: false }),
            // The clock went backwards: no negative durations.
            (session("backwards"), turn(now + MINUTE_MS, now)),
        ];
        assert!(f.observe(&input, START, MIN, now).is_empty());
    }

    #[test]
    fn far_future_ends_are_ignored_and_do_not_block_later_turns() {
        let mut f = FinishedTurns::default();
        let now = START + 30 * MINUTE_MS;
        // A corrupt or skewed timestamp: not reported, and not remembered as the last end.
        let bogus = now + FUTURE_SLACK_MS + 1;
        let future = [(session("a"), turn(bogus - 5 * MINUTE_MS, bogus))];
        assert!(f.observe(&future, START, MIN, now).is_empty());
        let real = [(session("a"), turn(now - 5 * MINUTE_MS, now))];
        assert_eq!(keys(&f.observe(&real, START, MIN, now)), vec![("a", 5 * MINUTE_MS)]);
        // A small skew is still reported.
        let skewed = now + FUTURE_SLACK_MS;
        let ahead = [(session("b"), turn(skewed - 5 * MINUTE_MS, skewed))];
        assert_eq!(f.observe(&ahead, START, MIN, now).len(), 1);
    }

    #[test]
    fn lower_bound_start_counts_when_long_enough() {
        let mut f = FinishedTurns::default();
        let end = START + 30 * MINUTE_MS;
        let bound = |d: Ms| TurnInfo { start_is_lower_bound: true, ..turn(end - d, end) };
        let input = [(session("short"), bound(MINUTE_MS)), (session("long"), bound(8 * MINUTE_MS))];
        assert_eq!(keys(&f.observe(&input, START, MIN, end)), vec![("long", 8 * MINUTE_MS)]);
    }

    #[test]
    fn sessions_are_deduped_independently() {
        let mut f = FinishedTurns::default();
        let end = START + 30 * MINUTE_MS;
        let a = (session("a"), turn(end - 5 * MINUTE_MS, end));
        let b = (session("b"), turn(end - 6 * MINUTE_MS, end));
        assert_eq!(keys(&f.observe(std::slice::from_ref(&a), START, MIN, end)), vec![("a", 5 * MINUTE_MS)]);
        assert_eq!(keys(&f.observe(&[a, b], START, MIN, end)), vec![("b", 6 * MINUTE_MS)]);
    }

    #[test]
    fn view_fields_are_kept() {
        let mut f = FinishedTurns::default();
        let end = START + 30 * MINUTE_MS;
        let view = FinishedTurn { project: Some("demo-app".into()), entrypoint: Entrypoint::Cowork, ..session("a") };
        let done = f.observe(&[(view.clone(), turn(end - 4 * MINUTE_MS, end))], START, MIN, end);
        assert_eq!(done, vec![FinishedTurn { duration_ms: 4 * MINUTE_MS, ..view }]);
    }
}
