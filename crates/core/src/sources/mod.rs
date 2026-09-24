//! Readers for the three token-free data sources. Each parser reads through
//! [`crate::saferead::SafeReader`] and never retains more than the numeric/model fields it needs.

pub mod desktop_sessions;
pub mod desktop_usage;
pub mod statusline;
pub mod transcript;

use crate::saferead::ReadError;

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("source file not found")]
    NotFound,
    #[error("read denied: {0}")]
    Read(#[from] ReadError),
    #[error("parse error: {0}")]
    Parse(String),
    /// A known file whose `version` we do not support (e.g. Desktop usage v3).
    #[error("unsupported schema version {0}")]
    SchemaChanged(u32),
}
