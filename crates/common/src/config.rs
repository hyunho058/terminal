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
    pub fn load_from(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).map_err(|source| CommonError::ConfigRead {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| CommonError::ConfigParse {
            path: path.to_path_buf(),
            source,
        })
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
