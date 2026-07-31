//! Shared error type for cross-cutting utilities (config, logging).
//!
//! Each crate owns its domain error (e.g. `term_core::Error`); this type only
//! covers the shared utilities that live in `common`.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommonError {
    #[error("could not locate the user config directory")]
    NoConfigDir,

    #[error("failed to read config at {path}: {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config at {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to write config at {path}: {source}")]
    ConfigWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("config watcher: {0}")]
    Watch(String),
}

pub type Result<T> = std::result::Result<T, CommonError>;
