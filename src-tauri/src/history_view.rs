//! `get_history` command: aggregates the local history into the 14-day History view.
//! Shapes mirror `HistoryData` in `src/lib/types.ts`.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::state::Shared;

/// One local calendar day of one window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryDay {
    pub day_start_ms: i64,
    pub peak_pct: f32,
    pub consumed_pct: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryWindow {
    pub kind: cuw_core::engine::types::WindowKind,
    pub points: Vec<cuw_core::engine::types::SparkPoint>,
    pub resets_ms: Vec<i64>,
    pub days: Vec<HistoryDay>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryData {
    pub from_ms: i64,
    pub to_ms: i64,
    pub windows: Vec<HistoryWindow>,
}

/// TODO(stream A): implement (see `HistoryData` docs in src/lib/types.ts).
#[tauri::command]
pub fn get_history(shared: State<'_, Arc<Shared>>, days: u32) -> Result<HistoryData, String> {
    let _ = (shared, days);
    Err("history view not implemented yet".into())
}
