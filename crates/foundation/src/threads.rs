//! Worker thread pool (ADR-0013 §3, FR-PLAT-013). Lives in `foundation` rather than `platform`
//! because the renderer, which does not depend on `platform`, is its main user.

use std::sync::OnceLock;

/// Configures the global `rayon` pool once and returns its thread count. `ANVIL_THREADS=n`
/// (n >= 1) overrides the default of one thread per logical CPU. Later calls return the same
/// count. Call before any parallel work; otherwise `rayon` has already built its own default.
pub fn init_thread_pool() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| {
        let requested = match std::env::var("ANVIL_THREADS") {
            Ok(v) => match v.parse::<usize>() {
                Ok(n) if n >= 1 => Some(n),
                _ => {
                    tracing::warn!(value = %v, "ANVIL_THREADS must be an integer >= 1, ignored");
                    None
                }
            },
            Err(_) => None,
        };
        let mut b = rayon::ThreadPoolBuilder::new().thread_name(|i| format!("anvil-worker-{i}"));
        if let Some(n) = requested {
            b = b.num_threads(n);
        }
        if let Err(e) = b.build_global() {
            tracing::warn!(error = %e, "worker pool was already initialised");
        }
        let n = rayon::current_num_threads();
        tracing::info!(threads = n, "worker pool ready");
        n
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn idempotent_and_positive() {
        let a = super::init_thread_pool();
        assert!(a >= 1);
        assert_eq!(a, super::init_thread_pool());
    }
}
