//! Readers for the four token-free data sources:
//! - [`statusline`]: the widget's own statusline captures (`<data_root>/capture/*.json`, written
//!   by the capture helper), for exact CLI usage, reset times, model and context;
//! - [`transcript`]: Claude Code and Cowork transcripts (`*.jsonl`), for the model, context size
//!   and turns of recent sessions (never message text);
//! - [`desktop_usage`]: Claude Desktop's `plan-usage-history.json`, for Desktop usage samples;
//! - [`desktop_sessions`]: Claude Desktop's Code-tab session metadata, for the focused session.
//!
//! Every read of their contents goes through [`crate::saferead::SafeReader`], and each parser
//! keeps only the numeric/model fields it needs. The few metadata-only calls that bypass the
//! reader (they never open a file) are listed in the crate docs, together with the widget's own
//! files that are read directly.

pub mod desktop_sessions;
pub mod desktop_usage;
pub(crate) mod fsutil;
pub mod statusline;
pub mod transcript;

use crate::saferead::ReadError;

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The file does not exist (also when it vanished between listing and reading).
    #[error("source file not found")]
    NotFound,
    /// Denied by the allowlist, too large, or another I/O error.
    #[error("read denied: {0}")]
    Read(ReadError),
    #[error("parse error: {0}")]
    Parse(String),
    /// A known file whose `version` we do not support (e.g. Desktop usage v3).
    #[error("unsupported schema version {0}")]
    SchemaChanged(u32),
}

/// A missing file becomes [`SourceError::NotFound`], anything else [`SourceError::Read`], so every
/// source reports missing, denied and corrupt files the same way.
impl From<ReadError> for SourceError {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => SourceError::NotFound,
            other => SourceError::Read(other),
        }
    }
}

impl From<std::io::Error> for SourceError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_map_to_not_found() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(SourceError::from(missing), SourceError::NotFound));
        let denied = ReadError::Denied("x".into());
        assert!(matches!(SourceError::from(denied), SourceError::Read(ReadError::Denied(_))));
        let other = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(matches!(SourceError::from(other), SourceError::Read(ReadError::Io(_))));
    }
}
