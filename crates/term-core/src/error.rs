//! `term-core` domain errors. PTY operations surface `anyhow` errors from
//! `portable-pty`; we box them behind a typed variant so callers match on
//! `term_core::Error`, not a foreign error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("pty operation failed: {0}")]
    Pty(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Wrap any foreign error (e.g. `anyhow::Error` from `portable-pty`).
    pub(crate) fn pty<E>(err: E) -> Self
    where
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Error::Pty(err.into())
    }
}
