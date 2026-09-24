//! Assembles a [`Snapshot`] from already-loaded source data. Implemented during integration
//! (after the individual engine modules land).

use crate::engine::types::Snapshot;

/// Placeholder so the module exists; the real `build_snapshot(EngineInputs, now_ms)` is added in
/// the integration step.
pub fn empty(now_ms: crate::time::Ms) -> Snapshot {
    use crate::engine::types::{DesktopHealth, SourceHealth};
    Snapshot {
        generated_ms: now_ms,
        windows: vec![],
        session: None,
        health: SourceHealth {
            desktop: DesktopHealth::NotFound,
            cli_last_capture_ms: None,
            transcripts_last_activity_ms: None,
        },
        warnings: vec![],
    }
}
