//! Picks which session the widget header shows.
//!
//! - Candidates: all tails. The newest `last_assistant_ms` wins.
//! - Tie-break: tails whose `last_assistant_ms` is within [`FOCUS_TIE_MS`] of the newest are
//!   tied; among them prefer the one whose `session_id` equals the `cli_session_id` of the Desktop
//!   session with the most recent `last_focused_ms`, if that focus happened within
//!   [`FOCUS_TIE_MS`] of `now_ms`. Otherwise keep the newest.
//! - `concurrent` = number of tails with `last_assistant_ms >= now_ms - CONCURRENT_WINDOW_MS`
//!   (saturating at 255).
//! - Empty input → `None`.

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

pub fn pick(tails: &[TranscriptTail], desktop_sessions: &[DesktopSession], now_ms: Ms) -> Option<ActivePick> {
    let _ = (tails, desktop_sessions, now_ms);
    todo!("active_session::pick")
}
