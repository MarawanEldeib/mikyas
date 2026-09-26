//! "Claude finished" detection from transcripts (no hooks, no message content).
//!
//! A turn = from the last HUMAN user line (a `type: "user"` line whose content is not only
//! tool_result blocks — read only the block `type` fields, never text) to the final assistant
//! line of that turn with `message.stop_reason == "end_turn"`. Its duration is end − start; when
//! the start is outside the scanned tail, the earliest timestamp in the tail is a lower bound.
//!
//! [`FinishedTurns::observe`] reports a turn once when: it ended after the app started
//! (`started_ms`) and within the last [`RECENT_MS`] (no startup flood), it lasted at least
//! `min_duration_ms`, and it was not reported before (dedupe by session key + end timestamp,
//! kept in memory only — reporting only turns that end after app start makes persistence
//! unnecessary). Sidechain/subagent lines never end a turn.

use std::collections::HashMap;

use crate::engine::types::Entrypoint;
use crate::time::{MINUTE_MS, Ms};

pub const RECENT_MS: Ms = 10 * MINUTE_MS;

/// Turn information extracted by the transcript tail scan (see `sources::transcript`).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// TODO(stream A2): implement per the module docs. `sessions` pairs each session's view data
    /// with its latest turn info.
    pub fn observe(
        &mut self,
        sessions: &[(FinishedTurn, TurnInfo)],
        started_ms: Ms,
        min_duration_ms: Ms,
        now_ms: Ms,
    ) -> Vec<FinishedTurn> {
        let _ = (sessions, started_ms, min_duration_ms, now_ms, &self.reported);
        Vec::new()
    }
}
