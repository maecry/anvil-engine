/// Errors raised by foundation itself (currently only logging setup).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The `ANVIL_LOG` filter string could not be parsed.
    #[error("invalid ANVIL_LOG filter: {0}")]
    InvalidLogFilter(String),

    /// `ANVIL_LOG_FORMAT` is neither `pretty` nor `json`.
    #[error("invalid ANVIL_LOG_FORMAT {0:?} (expected `pretty` or `json`)")]
    InvalidLogFormat(String),
    /// `ANVIL_LOG_FILE` could not be created.
    #[error("cannot open ANVIL_LOG_FILE {path}: {reason}")]
    LogFile {
        /// The requested path.
        path: String,
        /// The underlying I/O error.
        reason: String,
    },
}
