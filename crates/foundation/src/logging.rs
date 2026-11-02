use crate::CoreError;
use tracing_subscriber::EnvFilter;

/// Debug builds log engine crates at `debug`; everything else at `info`.
const DEFAULT_FILTER: &str = if cfg!(debug_assertions) {
    "info,foundation=debug,platform=debug,rhi=debug,rhi_vulkan=debug"
} else {
    "info"
};

/// Installs the global `tracing` subscriber, filtered by `ANVIL_LOG`.
///
/// Idempotent: if a global subscriber already exists this is a no-op returning `Ok`.
///
/// # Errors
/// [`CoreError::InvalidLogFilter`] if `ANVIL_LOG` is set but unparsable.
pub fn init_logging() -> Result<(), CoreError> {
    let filter = match std::env::var("ANVIL_LOG") {
        Ok(v) => EnvFilter::try_new(v).map_err(|e| CoreError::InvalidLogFilter(e.to_string()))?,
        Err(_) => EnvFilter::new(DEFAULT_FILTER),
    };
    // Err means a subscriber is already installed, which is fine.
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twice_does_not_panic() {
        init_logging().unwrap();
        init_logging().unwrap();
    }
}
