//! Pure computation: merge sources, estimate resets, forecast burn, resolve context size.
//! Nothing here touches the file system; all functions take `now_ms` explicitly.

pub mod active_session;
pub mod burn;
pub mod context;
pub mod merge;
pub mod reset_estimate;
pub mod snapshot;
pub mod types;
