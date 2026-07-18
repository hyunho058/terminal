//! Live config watching (settings feature, D1/D2).
//!
//! [`ConfigWatcher`] watches the config file's **parent directory** with `notify`
//! and re-loads on any change to `config.toml`, invoking a caller-supplied
//! `on_reload` with the fresh [`Config`]. Watching the *directory* (not the file
//! inode) is load-bearing: editors save atomically by writing a temp file and
//! `rename()`-ing it over the target, which swaps the inode — a file-inode watch
//! goes silent after the first save (D1).
//!
//! The channel contract is **emit only valid config** (D2): a parse/validation
//! error logs a warning and emits nothing, so the app keeps its last-applied
//! config (a typo never blanks the terminal). Lives in `common` — UI/GPU-free
//! (SD2); the app supplies the wake via `on_reload`.

use std::path::{Path, PathBuf};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::Config;
use crate::error::{CommonError, Result};

/// A live watcher over the config file. Dropping it stops watching.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    /// Watch `config_path`'s parent directory; on each change to that file,
    /// re-load and call `on_reload` with the new [`Config`] (only when valid).
    /// `on_reload` runs on the notify thread — keep it cheap (e.g. send on a
    /// channel + wake the event loop).
    pub fn spawn(
        config_path: PathBuf,
        on_reload: impl Fn(Config) + Send + 'static,
    ) -> Result<ConfigWatcher> {
        let dir = config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // Create the dir so a not-yet-existing config can still be watched (R1.2).
        let _ = std::fs::create_dir_all(&dir);

        let watch_path = config_path.clone();
        let target_name = config_path.file_name().map(|n| n.to_os_string());
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            // Match by file NAME, not full path: macOS FSEvents reports
            // canonicalized paths (`/private/var/…` vs `/var/…`), so exact path
            // equality would never fire (this is the D1 atomic-rename case too).
            let touches_config = event
                .paths
                .iter()
                .any(|p| p.file_name().map(|n| n.to_os_string()) == target_name);
            if touches_config {
                if let Some(cfg) = try_reload(&watch_path) {
                    on_reload(cfg);
                }
            }
        })
        .map_err(watch_err)?;

        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .map_err(watch_err)?;

        Ok(ConfigWatcher { _watcher: watcher })
    }
}

/// Load the config, returning `Some` only if it is valid (D2). An invalid config
/// (bad TOML, unknown key, non-positive size) logs a warning and yields `None`,
/// so the caller keeps its last-applied config. A missing file also yields `None`.
pub fn try_reload(config_path: &Path) -> Option<Config> {
    match Config::load_from(config_path) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::warn!("config reload rejected, keeping previous: {e}");
            None
        }
    }
}

fn watch_err(e: notify::Error) -> CommonError {
    CommonError::Watch(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A unique temp dir under the system temp (no external tempfile dep).
    fn temp_dir(tag: &str) -> PathBuf {
        let mut base = std::env::temp_dir();
        base.push(format!("mac-term-watch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    const VALID: &str = "[font]\nfamily = \"Menlo\"\nsize_pt = 18.0\nligatures = false\n\n[theme]\nbackground = 1122867\nforeground = 15132390\n\n[grid]\n";

    #[test]
    fn try_reload_returns_config_on_valid() {
        let dir = temp_dir("valid");
        let path = dir.join("config.toml");
        std::fs::write(&path, VALID).unwrap();
        let cfg = try_reload(&path).expect("valid config reloads");
        assert_eq!(cfg.font.size_pt, 18.0);
        assert!(!cfg.font.ligatures);
    }

    #[test]
    fn try_reload_drops_invalid_keeping_previous() {
        let dir = temp_dir("invalid");
        let path = dir.join("config.toml");
        // Unknown key → deny_unknown_fields rejects it (D2).
        std::fs::write(&path, "[font]\nnope = 1\n").unwrap();
        assert!(
            try_reload(&path).is_none(),
            "invalid config emits nothing, previous kept"
        );
        // Missing file is also safe.
        assert!(try_reload(&dir.join("does-not-exist.toml")).is_none());
    }

    #[test]
    fn watcher_survives_atomic_rename_save() {
        let dir = temp_dir("atomic");
        let path = dir.join("config.toml");
        std::fs::write(&path, VALID).unwrap();

        let (tx, rx) = mpsc::channel();
        let _w = ConfigWatcher::spawn(path.clone(), move |cfg| {
            let _ = tx.send(cfg);
        })
        .expect("watcher spawns");

        // Save via write-temp-then-rename (the editor pattern that swaps the inode).
        let bigger = VALID.replace("size_pt = 18.0", "size_pt = 24.0");
        let tmp = dir.join("config.toml.tmp");
        std::fs::write(&tmp, &bigger).unwrap();
        std::fs::rename(&tmp, &path).unwrap();

        // FSEvents has latency; allow a generous window.
        let cfg = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("watcher emits after an atomic-rename save");
        assert_eq!(cfg.font.size_pt, 24.0, "the new value is delivered");
    }
}
