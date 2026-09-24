//! Token-free Claude usage data sources and engine.
//!
//! Hard rule for everything in this crate: never read credentials, cookies or browser storage,
//! and never make network calls. All file reads go through [`saferead::SafeReader`].

pub mod alerts;
pub mod capture;
pub mod claude_settings;
pub mod cmdline;
pub mod engine;
pub mod fingerprint;
pub mod history;
pub mod model_names;
pub mod paths;
pub mod saferead;
pub mod sources;
pub mod time;
