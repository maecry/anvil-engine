/// Errors raised by foundation itself (currently only logging setup).
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The `ANVIL_LOG` filter string could not be parsed.
    #[error("invalid ANVIL_LOG filter: {0}")]
    InvalidLogFilter(String),
}
