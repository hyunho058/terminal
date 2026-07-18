//! Process-wide tracing setup. Call [`init`] once from `main`.
//!
//! Honors `RUST_LOG` (e.g. `RUST_LOG=app=debug,term_core=trace`); defaults to
//! `info` when unset.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize the global tracing subscriber. Idempotent-safe: a second call is
/// a no-op (the underlying `set_global_default` simply fails and is ignored).
pub fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .try_init();
}
