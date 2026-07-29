//! User configuration: font, color theme, ligatures, and grid defaults.
//!
//! Loaded from `~/.config/mac-terminal/config.toml` (TOML + serde). Every field
//! has a default, so a missing or partial file still yields a usable `Config`.
//! This type is deliberately UI/GPU-free so both `renderer` and `app` can read it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CommonError, Result};

/// Application id used for the config directory (`~/.config/<APP_ID>/`).
pub const APP_ID: &str = "mac-terminal";
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: FontConfig,
    pub theme: ThemeConfig,
    pub grid: GridConfig,
    pub sidebar: SidebarConfig,
    pub ai_monitor: AiMonitorConfig,
}

/// ai-monitor overlay settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiMonitorConfig {
    /// Context window in tokens, overriding the overlay's per-model table.
    ///
    /// Exists because the window is **not derivable from the log**: a 1M-context
    /// session records the same bare `message.model` as a 200k one, so an
    /// occupancy percentage against the assumed 200k reads up to 5x high — the
    /// measured case showed `context 128%`. `None` keeps the per-model table.
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    /// Family name resolved by the system font stack.
    pub family: String,
    pub size_pt: f32,
    /// Enable programming ligatures (e.g. `!=`, `=>`) during shaping.
    pub ligatures: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// Default background, packed `0xRRGGBB`.
    pub background: u32,
    /// Default foreground, packed `0xRRGGBB`.
    pub foreground: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GridConfig {
    pub columns: u16,
    pub rows: u16,
}

/// Left tab-sidebar appearance. Defaults are the Claude Design mock's own prop
/// values (D1). `width_px` is a *logical* px value scaled by the display backing
/// factor at the point of use (like [`FontConfig::size_pt`]), and is clamped to the
/// mock's slider band [`SIDEBAR_WIDTH_MIN`]..[`SIDEBAR_WIDTH_MAX`] at load (D2).
///
/// `accent` is defined because the mock exposes it as a user setting (SD7), but no
/// sidebar element consumes it yet — its consumers are deferred terminal-pane
/// surfaces (D1). Setting it currently changes nothing on screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SidebarConfig {
    /// Sidebar width in *logical* px (scaled by the display backing factor at use).
    /// Clamped to `[SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX]` in [`Config::load_from`].
    pub width_px: f32,
    /// Show each tab's git branch on the card's second line.
    pub show_git_branch: bool,
    /// Show each tab's AI-agent status marker.
    pub show_agent_status: bool,
    /// Persisted collapsed state — the sidebar starts hidden when true.
    pub collapsed: bool,
    /// Accent colour, packed `0xRRGGBB`. Reserved (SD7); no sidebar element reads it yet.
    pub accent: u32,
}

/// Minimum sidebar `width_px` (the mock's slider lower bound). Below this a two-line
/// card cannot fit a title plus a branch line without permanent ellipsis (D2).
pub const SIDEBAR_WIDTH_MIN: f32 = 260.0;
/// Maximum sidebar `width_px` (the mock's slider upper bound). Above this the sidebar
/// starves the terminal grid of columns on a laptop display (D2).
pub const SIDEBAR_WIDTH_MAX: f32 = 440.0;

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Menlo".to_string(),
            size_pt: 14.0,
            // Off by default: Menlo's typographic f-ligatures (fi, fl, …) collapse
            // into one glyph the renderer draws only at the lead cell, leaving a
            // "ghost space" on the monospace grid (`first` → `fi rst`). Programming
            // ligatures stay available via an explicit `font.ligatures = true`.
            ligatures: false,
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        // Solarized-ish dark defaults.
        Self {
            background: 0x0F1419,
            foreground: 0xE6E1CF,
        }
    }
}

impl Default for GridConfig {
    fn default() -> Self {
        Self {
            columns: 100,
            rows: 30,
        }
    }
}

impl Default for SidebarConfig {
    fn default() -> Self {
        // The Claude Design mock's own prop defaults (D1).
        Self {
            width_px: 340.0,
            show_git_branch: true,
            show_agent_status: true,
            collapsed: false,
            accent: 0x0086_B0A1,
        }
    }
}

impl Config {
    /// Absolute path to the config file (`~/.config/mac-terminal/config.toml`).
    pub fn path() -> Result<PathBuf> {
        let dirs =
            directories::ProjectDirs::from("", "", APP_ID).ok_or(CommonError::NoConfigDir)?;
        Ok(dirs.config_dir().join(CONFIG_FILE))
    }

    /// Load config from the default path, falling back to defaults if the file
    /// does not exist. Parse/IO errors on an *existing* file are returned.
    pub fn load() -> Result<Config> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Config::default());
        }
        Self::load_from(&path)
    }

    /// Load config from an explicit path. Used by tests and the exemplar.
    ///
    /// This is the single parse entry point (`load` delegates here, as does the
    /// live-reload watcher), so the sidebar-width clamp applied here is
    /// unbypassable on every production path (D2).
    pub fn load_from(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).map_err(|source| CommonError::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        let mut cfg: Config = toml::from_str(&text).map_err(|source| CommonError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })?;
        cfg.normalize();
        Ok(cfg)
    }

    /// Clamp cosmetic fields into their supported range at load, warning once per
    /// out-of-range value rather than failing (D2) — a typo in a cosmetic field must
    /// never keep the terminal from starting. A non-finite width falls back to the
    /// default. Emitted here (load/reload only), never per frame.
    fn normalize(&mut self) {
        let requested = self.sidebar.width_px;
        let effective = if requested.is_finite() {
            requested.clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX)
        } else {
            SidebarConfig::default().width_px
        };
        if effective != requested {
            tracing::warn!(
                requested,
                effective,
                "sidebar.width_px {requested} out of range [{SIDEBAR_WIDTH_MIN}, {SIDEBAR_WIDTH_MAX}]; clamped to {effective}"
            );
            self.sidebar.width_px = effective;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert!(c.grid.columns > 0 && c.grid.rows > 0);
        assert!(c.font.size_pt > 0.0);
    }

    #[test]
    fn ai_monitor_context_window_is_optional_and_parses() {
        // Absent by default: the overlay then falls back to its per-model table.
        assert_eq!(Config::default().ai_monitor.context_window, None);

        let c: Config = toml::from_str(
            r#"
            [ai_monitor]
            context_window = 1000000
        "#,
        )
        .unwrap();
        assert_eq!(c.ai_monitor.context_window, Some(1_000_000));

        // A config with no [ai_monitor] table at all still loads.
        let c: Config = toml::from_str("[font]\nsize_pt = 13.0\n").unwrap();
        assert_eq!(c.ai_monitor.context_window, None);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let toml = r#"
            [font]
            size_pt = 18.0
        "#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.font.size_pt, 18.0);
        // Untouched fields fall back to defaults.
        assert_eq!(c.font.family, FontConfig::default().family);
        assert_eq!(c.grid.rows, GridConfig::default().rows);
    }

    /// A unique temp dir under the system temp (no external tempfile dep), mirroring
    /// the idiom in `watch.rs` / `ai-monitor`. Clamp tests must go through `load_from`
    /// (a real file) because `toml::from_str::<Config>` bypasses the clamp.
    fn write_tmp_config(tag: &str, body: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mac-term-cfg-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.toml");
        std::fs::write(&path, body).expect("write temp config");
        path
    }

    #[test]
    fn sidebar_defaults_match_mock() {
        // R1.1 — an absent [sidebar] yields the mock's prop defaults.
        let c: Config = toml::from_str("[grid]\ncolumns = 80\n").unwrap();
        assert_eq!(c.sidebar, SidebarConfig::default());
        assert_eq!(c.sidebar.width_px, 340.0);
        assert!(c.sidebar.show_git_branch);
        assert!(c.sidebar.show_agent_status);
        assert!(!c.sidebar.collapsed);
        assert_eq!(c.sidebar.accent, 0x0086_B0A1);
    }

    #[test]
    fn partial_sidebar_fills_defaults() {
        // R1.2 — one key set, the rest fall back to defaults.
        let c: Config = toml::from_str("[sidebar]\nshow_git_branch = false\n").unwrap();
        assert!(!c.sidebar.show_git_branch);
        assert_eq!(c.sidebar.width_px, 340.0);
        assert!(c.sidebar.show_agent_status);
        assert!(!c.sidebar.collapsed);
        assert_eq!(c.sidebar.accent, 0x0086_B0A1);
    }

    #[test]
    fn unknown_sidebar_key_rejected() {
        // R1.3 — a typo fails loudly under deny_unknown_fields.
        let err = toml::from_str::<Config>("[sidebar]\nwidht_px = 300.0\n");
        assert!(err.is_err(), "unknown key must be rejected");
    }

    #[test]
    fn accent_accepts_hex_literal() {
        // R1.4 — packed 0xRRGGBB, no string parsing.
        let c: Config = toml::from_str("[sidebar]\naccent = 0xE0863F\n").unwrap();
        assert_eq!(c.sidebar.accent, 0x00E0_863F);
    }

    #[test]
    fn width_clamped_up_at_load() {
        // R2.1 — a too-small width is clamped to the floor (through load_from).
        let p = write_tmp_config("clamp-up", "[sidebar]\nwidth_px = 10.0\n");
        let c = Config::load_from(&p).unwrap();
        assert_eq!(c.sidebar.width_px, SIDEBAR_WIDTH_MIN);
    }

    #[test]
    fn width_clamped_down_at_load() {
        // R2.2 — a too-large width is clamped to the ceiling; startup survives.
        let p = write_tmp_config("clamp-down", "[sidebar]\nwidth_px = 100000.0\n");
        let c = Config::load_from(&p).unwrap();
        assert_eq!(c.sidebar.width_px, SIDEBAR_WIDTH_MAX);
    }

    #[test]
    fn width_in_band_passes_through() {
        // R2.3 — an in-band width is untouched.
        let p = write_tmp_config("in-band", "[sidebar]\nwidth_px = 300.0\n");
        let c = Config::load_from(&p).unwrap();
        assert_eq!(c.sidebar.width_px, 300.0);
    }

    #[test]
    fn nonfinite_width_falls_back_to_default() {
        // R2 / R8-spirit — a NaN width normalizes to the default rather than propagating.
        let mut c = Config::default();
        c.sidebar.width_px = f32::NAN;
        c.normalize();
        assert_eq!(c.sidebar.width_px, SidebarConfig::default().width_px);
    }

    #[test]
    fn example_toml_loads_and_is_in_band() {
        // R1.5 / R1.6 — the shipped example parses under deny_unknown_fields and
        // its documented width is in-band (no clamp warning on the example).
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.toml");
        let c = Config::load_from(&path).expect("example config must load");
        assert_eq!(c.sidebar.width_px, 340.0);
        assert_eq!(c.sidebar.accent, 0x0086_B0A1);
    }

    #[test]
    fn ligatures_default_off() {
        // Regression: Menlo's typographic `fi` ligature breaks the monospace grid
        // (ghost space: `first` → `fi rst`), so ligatures default OFF.
        assert!(!FontConfig::default().ligatures);
        assert!(!Config::default().font.ligatures);
        // An omitted key falls back to the (now false) default via serde.
        let omitted: Config = toml::from_str("[font]\nsize_pt = 14.0\n").unwrap();
        assert!(!omitted.font.ligatures, "omitted key → default false");
        // But an EXPLICIT opt-in is still honored — the default flip must not override it.
        let explicit: Config = toml::from_str("[font]\nligatures = true\n").unwrap();
        assert!(
            explicit.font.ligatures,
            "explicit `ligatures = true` still enables"
        );
    }
}
