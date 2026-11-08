//! User configuration: font, color theme, ligatures, and grid defaults.
//!
//! Loaded from the platform config dir via [`directories::ProjectDirs`] — on macOS that
//! is `~/Library/Application Support/Crewterm/config.toml`, **not** `~/.config`
//! (verified empirically; several docs still say otherwise). TOML + serde. Every field
//! has a default, so a missing or partial file still yields a usable `Config`.
//! This type is deliberately UI/GPU-free so both `renderer` and `app` can read it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CommonError, Result};

/// Application id used for the config directory
/// (`~/Library/Application Support/<APP_ID>/`).
pub const APP_ID: &str = "Crewterm";
/// The name this app shipped under before it became Crewterm; `Config::path` still
/// reads a config left there when none exists under `APP_ID`.
pub const LEGACY_APP_ID: &str = "mac-terminal";
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

/// ai-monitor panel settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiMonitorConfig {
    /// Context window in tokens, overriding the overlay's per-model table.
    ///
    /// Exists because the window is **not derivable from the log**: a 1M-context
    /// session records the same bare `message.model` as a 200k one, so an
    /// occupancy percentage against the assumed 200k reads up to 5x high — the
    /// measured case showed `context 128%`. `None` keeps the per-model table.
    pub context_window: Option<u64>,
    /// The docked panel's width in *logical* px (ai-monitor-right-panel D6), clamped to
    /// `[MONITOR_WIDTH_MIN, MONITOR_WIDTH_MAX]` in [`Config::load_from`]. Logical, like
    /// [`SidebarConfig::width_px`], so the file does not become display-dependent.
    ///
    /// This is the user's **intent**, never the width actually drawn (D8): a narrow window
    /// squeezes the band below it at layout time, and persisting that squeeze would destroy
    /// a width chosen on a large display the first afternoon spent on a laptop.
    pub width_px: f32,
    /// Whether the panel was open when the user last pressed `Cmd+Shift+M` (D6/D8).
    ///
    /// Written by that chord and by nothing else — a window too narrow to fit the panel
    /// force-closes it as a *display clamp*, which must leave this flag alone so re-widening
    /// can restore the panel with no keystroke (R4.5/R6.3).
    pub open: bool,
}

/// Minimum ai-monitor panel `width_px` — the design's drag lower bound. Below this the
/// section cards and the two-column stat grid cannot hold their labels.
pub const MONITOR_WIDTH_MIN: f32 = 300.0;
/// Maximum ai-monitor panel `width_px` — the design's drag upper bound. Above this the
/// panel starves the terminal of columns before D4's 20-column floor even engages.
pub const MONITOR_WIDTH_MAX: f32 = 760.0;

impl Default for AiMonitorConfig {
    fn default() -> Self {
        Self {
            context_window: None,
            // The design's `width:430px`.
            width_px: 430.0,
            open: false,
        }
    }
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

/// Minimum terminal `font.size_pt` (terminal-font-size D7). Below `8.0 pt` (`cell_w = 5px`
/// at 1×), glyphs become illegible.
pub const FONT_SIZE_MIN: f32 = 8.0;
/// Maximum terminal `font.size_pt` (terminal-font-size D7). Above `32.0 pt` (`cell_w = 19px`
/// at 1×), a standard laptop window with sidebar open fits barely 25 columns.
pub const FONT_SIZE_MAX: f32 = 32.0;
/// Default terminal `font.size_pt` (terminal-font-size D7) — also the target of `Cmd+0` reset.
pub const FONT_SIZE_DEFAULT: f32 = 14.0;
/// Per-chord step (`±1.0 pt`) for `Cmd+=` / `Cmd++` and `Cmd+-` (terminal-font-size D7).
pub const FONT_SIZE_STEP: f32 = 1.0;

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Menlo".to_string(),
            size_pt: FONT_SIZE_DEFAULT,
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
    /// Absolute path to the config file. `ProjectDirs` follows the platform convention, so
    /// on macOS this is `~/Library/Application Support/Crewterm/config.toml` — the
    /// XDG-style `~/.config/...` that older comments cite is not what this returns.
    ///
    /// The app was renamed from `mac-terminal` to Crewterm (2026-11-02). Until a config
    /// exists under the new name, a file still at the old directory is used, so the
    /// rename does not silently reset anyone's settings. The watcher follows this same
    /// path, so live reload keeps working on the legacy file too.
    pub fn path() -> Result<PathBuf> {
        let dirs =
            directories::ProjectDirs::from("", "", APP_ID).ok_or(CommonError::NoConfigDir)?;
        let current = dirs.config_dir().join(CONFIG_FILE);
        if current.exists() {
            return Ok(current);
        }
        let legacy = directories::ProjectDirs::from("", "", LEGACY_APP_ID)
            .map(|d| d.config_dir().join(CONFIG_FILE));
        match legacy {
            Some(old) if old.exists() => Ok(old),
            _ => Ok(current),
        }
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
        self.font.size_pt = clamp_width(
            "font.size_pt",
            self.font.size_pt,
            FONT_SIZE_MIN,
            FONT_SIZE_MAX,
            FONT_SIZE_DEFAULT,
        );
        self.sidebar.width_px = clamp_width(
            "sidebar.width_px",
            self.sidebar.width_px,
            SIDEBAR_WIDTH_MIN,
            SIDEBAR_WIDTH_MAX,
            SidebarConfig::default().width_px,
        );
        // The panel's width gets the same treatment as the sidebar's, and for the same
        // reason: it is a cosmetic value the user hand-edits, so an out-of-band number
        // must clamp with a warning rather than keep the terminal from starting (D2).
        self.ai_monitor.width_px = clamp_width(
            "ai_monitor.width_px",
            self.ai_monitor.width_px,
            MONITOR_WIDTH_MIN,
            MONITOR_WIDTH_MAX,
            AiMonitorConfig::default().width_px,
        );
    }
}

/// One cosmetic width clamped into its supported band, warning once when it moved (D2).
/// A non-finite value falls back to `fallback` rather than propagating a `NaN` into the
/// renderer's inset arithmetic, where it would silently poison every comparison.
fn clamp_width(field: &str, requested: f32, min: f32, max: f32, fallback: f32) -> f32 {
    let effective = if requested.is_finite() {
        requested.clamp(min, max)
    } else {
        fallback
    };
    if effective != requested {
        tracing::warn!(
            field,
            requested,
            effective,
            "{field} {requested} out of range [{min}, {max}]; clamped to {effective}"
        );
    }
    effective
}

// ---------------------------------------------------------------------------
// Chrome write-back (tab-resize D8) — the FIRST thing in this workspace that
// writes `config.toml` rather than only reading it. Three callers now: the sidebar's
// drag/collapse, the ai-monitor panel's drag/toggle (right-panel D6), and terminal
// font size chords (terminal-font-size D9).
// ---------------------------------------------------------------------------

/// The temp file a [`save_sidebar`] writes before renaming it over the target (D8).
///
/// It must live in the config directory (`rename` is only atomic within one filesystem)
/// and it must **not** be named [`CONFIG_FILE`]: [`crate::watch::ConfigWatcher`] matches
/// change events by file *name* over that very directory, so a same-named temp would trip
/// our own watcher twice per save.
const TMP_FILE: &str = "config.toml.tmp";

/// Render an `f32` the way [`save_sidebar`] writes it — shortest representation that
/// round-trips exactly (tab-resize D15/R8.7).
///
/// `{}` is deliberate: a fixed precision like `{:.1}` loses bits, and the echo-suppression
/// predicate compares the *re-parsed* value against what we wrote. A lossy format would
/// therefore turn every one of our own writes into a spurious "human edit", clearing the
/// runtime override on every drag. Also forces a decimal point so the value re-parses as a
/// TOML float, not an integer (`400` would be an integer and fail `f32` deserialization).
fn fmt_f32(v: f32) -> String {
    let s = format!("{v}");
    if s.contains(['.', 'e', 'E', 'n', 'i']) {
        // already fractional, exponential, NaN or inf
        s
    } else {
        format!("{s}.0")
    }
}

/// Persist ONLY the sidebar's `width_px` and `collapsed` to `path`, preserving everything
/// else in the file byte-for-byte (tab-resize D2/D8/D16).
pub fn save_sidebar(path: &Path, width_px: f32, collapsed: bool) -> Result<()> {
    save_table(
        path,
        "sidebar",
        &[
            ("width_px", fmt_f32(width_px)),
            ("collapsed", collapsed.to_string()),
        ],
    )
}

/// Persist ONLY the ai-monitor panel's `width_px` and `open` to `path` (right-panel D6/D8),
/// under exactly the sidebar's rules — same surgical editing, same atomic rename.
///
/// `width_px` is the user's **intent**: a released drag's width, never the width a narrow
/// window squeezed the band down to (D8/R6.2). `open` is written only by the `Cmd+Shift+M`
/// chord, so a force-close leaves it reading `true` and re-widening restores the panel with
/// no keystroke (R6.3).
pub fn save_ai_monitor(path: &Path, width_px: f32, open: bool) -> Result<()> {
    save_table(
        path,
        "ai_monitor",
        &[("width_px", fmt_f32(width_px)), ("open", open.to_string())],
    )
}

/// Persist ONLY `[font].size_pt` to `path` (terminal-font-size D9/R1.4), preserving comments,
/// key order, `[font].family`, `[font].ligatures`, and all other tables byte-for-byte via
/// atomic rename.
pub fn save_font_size(path: &Path, size_pt: f32) -> Result<()> {
    save_table(path, "font", &[("size_pt", fmt_f32(size_pt))])
}

/// Persist updated settings from `cfg` to `path` across all standard tables
/// (`font`, `grid`, `sidebar`, `ai_monitor`), preserving comments and unrecognized
/// keys byte-for-byte.
pub fn save_config(path: &Path, cfg: &Config) -> Result<()> {
    save_table(
        path,
        "font",
        &[
            ("family", format!("\"{}\"", cfg.font.family)),
            ("size_pt", fmt_f32(cfg.font.size_pt)),
            ("ligatures", cfg.font.ligatures.to_string()),
        ],
    )?;
    save_table(
        path,
        "grid",
        &[
            ("columns", cfg.grid.columns.to_string()),
            ("rows", cfg.grid.rows.to_string()),
        ],
    )?;
    save_table(
        path,
        "theme",
        &[
            ("background", format!("0x{:06X}", cfg.theme.background)),
            ("foreground", format!("0x{:06X}", cfg.theme.foreground)),
        ],
    )?;
    save_table(
        path,
        "sidebar",
        &[
            ("width_px", fmt_f32(cfg.sidebar.width_px)),
            ("show_git_branch", cfg.sidebar.show_git_branch.to_string()),
            (
                "show_agent_status",
                cfg.sidebar.show_agent_status.to_string(),
            ),
            ("collapsed", cfg.sidebar.collapsed.to_string()),
        ],
    )?;
    save_table(
        path,
        "ai_monitor",
        &[
            ("width_px", fmt_f32(cfg.ai_monitor.width_px)),
            ("open", cfg.ai_monitor.open.to_string()),
        ],
    )?;
    Ok(())
}

/// Write `entries` into `[table]` of the TOML file at `path`, leaving every other byte
/// alone (tab-resize D2/D8/D16).
///
/// **Surgical, not a serde round-trip.** `toml::to_string(&Config)` would be three lines
/// and would silently delete every comment and reorder every key — turning a file the user
/// hand-edits into one only the machine can maintain. So this rewrites the target lines in
/// place, inserts the table when absent, and leaves comments, blank lines, key order, and
/// keys we do not model exactly where they were.
///
/// **Atomic.** Writes [`TMP_FILE`] beside the target and renames over it, so the watcher
/// (and any concurrent reader) can never observe a half-written file.
///
/// Creates the parent directory and the file itself when missing — a fresh install has no
/// `config.toml` at all, because [`Config::load`] returns defaults without creating one.
fn save_table(path: &Path, table: &str, entries: &[(&str, String)]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|source| CommonError::ConfigWrite {
            path: path.to_path_buf(),
            source,
        })?;
    }
    // A missing file is not an error — it is a fresh install. Treat it as empty and let
    // the editor below synthesize the table.
    let original = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(CommonError::ConfigWrite {
                path: path.to_path_buf(),
                source,
            })
        }
    };

    // Refuse to touch a file whose table is an inline table or a dotted key
    // (`sidebar = { .. }` / `sidebar.width_px = ..`). Both are valid TOML that
    // `load_from` accepts today, but the line-oriented editor below cannot see them as a
    // table and would append a second `[sidebar]` — defining the key twice, which makes
    // the file unparseable and silently drops the user back to full defaults at next
    // launch. Warning and keeping the session value is exactly what D14 prescribes.
    if has_non_table_key(&original, table) {
        return Err(CommonError::ConfigWrite {
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "[{table}] is written as an inline table or dotted key; refusing to rewrite it"
                ),
            ),
        });
    }
    let updated = edit_table_toml(&original, table, entries);

    let tmp = path.with_file_name(TMP_FILE);
    std::fs::write(&tmp, updated).map_err(|source| CommonError::ConfigWrite {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| CommonError::ConfigWrite {
        path: path.to_path_buf(),
        source,
    })
}

/// The pure text transform behind [`save_table`] — separated so every fidelity rule is
/// unit-testable without touching a filesystem.
///
/// Rules, in the order they matter:
/// 1. Inside the `[table]` table, replace the *value* of an existing line for each entry
///    key, preserving its indentation and re-attaching any trailing comment.
/// 2. Keys absent from an existing `[table]` table are appended to the end of that
///    table (before the next `[header]`), never to the file's end — which would place them
///    under a different table and change their meaning.
/// 3. No `[table]` table at all → append a fresh one.
///
/// Parameterised over the table and its keys rather than duplicated per caller: every rule
/// above is a fidelity promise about a user's hand-edited file, and a second copy is a
/// second place for one of them to rot. `[ai_monitor]` inherits all three for free.
fn edit_table_toml(original: &str, table: &str, entries: &[(&str, String)]) -> String {
    let key_lines: Vec<String> = entries
        .iter()
        .map(|(key, value)| format!("{key} = {value}"))
        .collect();
    // `str::lines()` strips the `\r` of a `\r\n`, so rejoining with `\n` alone would
    // silently rewrite every line ending in a CRLF file — a change to every byte of every
    // line, which R8.2's "byte-identical" promise forbids.
    // Keyed on the file's FIRST line ending, not "contains a CRLF anywhere": a mostly-LF
    // file with one stray CRLF would otherwise be rewritten wholly to CRLF — the same
    // every-line-changed damage, in the other direction.
    let newline = match original.find('\n') {
        Some(i) if i > 0 && original.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    };

    let mut out: Vec<String> = Vec::new();
    let mut in_table = false;
    let mut seen_table = false;
    let mut written = vec![false; entries.len()];
    // Where the table ended, so missing keys land inside it (rule 2).
    let mut table_end: Option<usize> = None;

    for line in original.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            // Leaving the table: remember where to append any missing key.
            if in_table {
                table_end = Some(out.len());
                in_table = false;
            }
            if is_table_header(trimmed, table) {
                in_table = true;
                seen_table = true;
            }
            out.push(line.to_string());
            continue;
        }
        // Keep any trailing comment the user wrote beside the value — replacing the
        // whole line would delete `# I like it narrow`, which R8.2 promises not to do.
        let hit = in_table
            .then(|| {
                entries
                    .iter()
                    .enumerate()
                    .find_map(|(i, (key, _))| key_indent(line, key).map(|indent| (i, indent)))
            })
            .flatten();
        if let Some((i, indent)) = hit {
            let (before, comment) = split_trailing_comment(line);
            out.push(join_comment(indent, &key_lines[i], before, comment));
            written[i] = true;
            continue;
        }
        out.push(line.to_string());
    }
    if in_table {
        table_end = Some(out.len());
    }

    let missing: Vec<String> = key_lines
        .iter()
        .zip(&written)
        .filter(|(_, done)| !**done)
        .map(|(line, _)| line.clone())
        .collect();

    if !missing.is_empty() {
        if seen_table {
            // Rule 2: append inside the existing table, at its end.
            let at = table_end.unwrap_or(out.len());
            // Skip back over trailing blank lines so the keys hug the table's content.
            let at = trim_back_blanks(&out, at);
            for (i, line) in missing.iter().enumerate() {
                out.insert(at + i, line.clone());
            }
        } else {
            // Rule 3: no table — append a fresh one.
            if !out.is_empty() && !out.last().is_some_and(|l| l.trim().is_empty()) {
                out.push(String::new());
            }
            out.push(format!("[{table}]"));
            out.extend(missing);
        }
    }

    let mut text = out.join(newline);
    // Preserve the original's trailing-newline convention; default to having one.
    if original.is_empty() || original.ends_with('\n') {
        text.push_str(newline);
    }
    text
}

/// Re-attach a preserved trailing comment to a rewritten key line, reusing the **original**
/// gap between the value and the `#` so the line's spacing is untouched (R8.2). `before` is
/// the original text left of the comment; only its trailing whitespace is read.
fn join_comment(indent: &str, key_line: &str, before: &str, comment: &str) -> String {
    if comment.is_empty() {
        return format!("{indent}{key_line}");
    }
    let gap = &before[before.trim_end().len()..];
    let gap = if gap.is_empty() { " " } else { gap };
    format!("{indent}{key_line}{gap}{comment}")
}

/// Whether the file declares `name` as a **top-level inline table or dotted key**
/// rather than a `[name]` section — a shape [`edit_table_toml`] cannot edit safely.
///
/// Only top-level assignments count: a `sidebar = ...` line *inside* some other table is
/// a different key entirely and is none of our business.
fn has_non_table_key(original: &str, name: &str) -> bool {
    let quoted_d = format!("\"{name}\"");
    let quoted_s = format!("'{name}'");
    let mut at_top_level = true;
    for line in original.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            at_top_level = false;
            // `[[sidebar]]` is an array-of-tables — a third door into the same duplicate-key
            // corruption, since `is_table_header` does not match it and the editor would
            // append a plain `[sidebar]` alongside it. Refuse rather than corrupt.
            if t.starts_with("[[") {
                let head = t.trim_start_matches('[');
                if unquote(head.split(']').next().unwrap_or("").trim()) == name {
                    return true;
                }
            }
            continue;
        }
        if !at_top_level {
            continue;
        }
        // Accept a quoted key too (`"sidebar" = { .. }`) — TOML treats it as the same key,
        // and missing it produces the same duplicate-table corruption this guard exists
        // to prevent.
        let key = t
            .strip_prefix(name)
            .or_else(|| t.strip_prefix(quoted_d.as_str()))
            .or_else(|| t.strip_prefix(quoted_s.as_str()));
        if let Some(rest) = key {
            let rest = rest.trim_start();
            // `sidebar = {..}` (inline table) or `sidebar.width_px = ..` (dotted key).
            if rest.starts_with('=') || rest.starts_with('.') {
                return true;
            }
        }
    }
    false
}

/// [`edit_table_toml`] bound to the sidebar's two keys — the shape the fidelity tests were
/// written against, kept so they keep asserting the rules rather than the generalisation.
#[cfg(test)]
fn edit_sidebar_toml(original: &str, width_px: f32, collapsed: bool) -> String {
    edit_table_toml(
        original,
        "sidebar",
        &[
            ("width_px", fmt_f32(width_px)),
            ("collapsed", collapsed.to_string()),
        ],
    )
}

/// The same, bound to the panel's two keys (D6).
#[cfg(test)]
fn edit_ai_monitor_toml(original: &str, width_px: f32, open: bool) -> String {
    edit_table_toml(
        original,
        "ai_monitor",
        &[("width_px", fmt_f32(width_px)), ("open", open.to_string())],
    )
}

/// Split a line into `(before_comment, comment_including_hash)` so a rewrite can keep a
/// trailing comment the user wrote beside the value.
///
/// The `#` must be outside a quoted string — `width_px = "a#b"` has no comment. Only
/// basic double/single quotes are tracked, which is all a scalar value can contain here.
fn split_trailing_comment(line: &str) -> (&str, &str) {
    let (mut in_d, mut in_s) = (false, false);
    for (i, c) in line.char_indices() {
        match c {
            '"' if !in_s => in_d = !in_d,
            '\'' if !in_d => in_s = !in_s,
            '#' if !in_d && !in_s => return (&line[..i], &line[i..]),
            _ => {}
        }
    }
    (line, "")
}

/// Whether `line` is the table header `[name]` — tolerating inner spaces (`[ sidebar ]`)
/// and a quoted key (`["sidebar"]`), both of which TOML treats as identical to `[sidebar]`.
///
/// The quoting case is not pedantry: missing it made the editor believe there was no
/// `[sidebar]` table, append a second one, and produce a file with a duplicate key — which
/// fails to parse, so the next launch silently falls back to *all* defaults and the user's
/// whole config stops taking effect.
fn is_table_header(line: &str, name: &str) -> bool {
    line.strip_prefix('[')
        .and_then(|s| s.split(']').next())
        .is_some_and(|s| unquote(s.trim()) == name)
}

/// Strip one layer of matching `"` or `'` quotes, if present.
fn unquote(s: &str) -> &str {
    for q in ['"', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// If `line` assigns `key`, return the line's leading whitespace so the rewrite keeps the
/// file's indentation style. `None` when the line assigns something else.
fn key_indent<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix(key)?;
    // `width_px_extra = 1` must not match `width_px`.
    if !rest.trim_start().starts_with('=') {
        return None;
    }
    Some(&line[..line.len() - trimmed.len()])
}

/// The index at or before `at` that skips back over trailing blank lines.
fn trim_back_blanks(lines: &[String], at: usize) -> usize {
    let mut i = at;
    while i > 0 && lines[i - 1].trim().is_empty() {
        i -= 1;
    }
    i
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
    fn save_config_persists_and_reloads_cleanly() {
        let dir = std::env::temp_dir().join(format!("term-cfg-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let mut cfg = Config::default();
        cfg.font.family = "Monaco".to_string();
        cfg.font.size_pt = 18.0;
        cfg.font.ligatures = true;
        cfg.grid.columns = 100;
        cfg.grid.rows = 40;
        cfg.theme.background = 0x1A1B26;
        cfg.theme.foreground = 0xC0CAF5;
        cfg.sidebar.width_px = 350.0;
        cfg.sidebar.show_git_branch = false;
        cfg.ai_monitor.width_px = 500.0;
        cfg.ai_monitor.open = true;

        save_config(&path, &cfg).expect("save_config succeeds");

        let loaded = Config::load_from(&path).expect("reloaded config");
        assert_eq!(loaded.font.family, "Monaco");
        assert_eq!(loaded.font.size_pt, 18.0);
        assert!(loaded.font.ligatures);
        assert_eq!(loaded.grid.columns, 100);
        assert_eq!(loaded.grid.rows, 40);
        assert_eq!(loaded.theme.background, 0x1A1B26);
        assert_eq!(loaded.theme.foreground, 0xC0CAF5);
        assert_eq!(loaded.sidebar.width_px, 350.0);
        assert!(!loaded.sidebar.show_git_branch);
        assert_eq!(loaded.ai_monitor.width_px, 500.0);
        assert!(loaded.ai_monitor.open);

        let _ = std::fs::remove_dir_all(&dir);
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

    /// D6/R6.1 — the panel's two persisted keys parse, default to the design's values, and
    /// survive a table that names only one of them.
    #[test]
    fn ai_monitor_width_and_open_default_and_parse() {
        assert_eq!(Config::default().ai_monitor.width_px, 430.0);
        assert!(!Config::default().ai_monitor.open);

        let c: Config = toml::from_str("[ai_monitor]\nwidth_px = 520.0\nopen = true\n").unwrap();
        assert_eq!(c.ai_monitor.width_px, 520.0);
        assert!(c.ai_monitor.open);

        // The pre-existing key keeps working beside them, and the absent ones default —
        // an old config.toml written before this feature must still load unchanged.
        let c: Config = toml::from_str("[ai_monitor]\ncontext_window = 1000000\n").unwrap();
        assert_eq!(c.ai_monitor.width_px, 430.0);
        assert!(!c.ai_monitor.open);
        // …and a file with no [ai_monitor] table at all is still a valid config.
        let c: Config = toml::from_str("[grid]\ncolumns = 80\n").unwrap();
        assert_eq!(c.ai_monitor.width_px, 430.0);
    }

    /// D2 — the panel's width is clamped at load like the sidebar's, so a hand-typed 5000
    /// cannot reach the inset arithmetic.
    #[test]
    fn ai_monitor_width_is_clamped_at_load() {
        let p = write_tmp_config("mon-clamp-up", "[ai_monitor]\nwidth_px = 10.0\n");
        assert_eq!(
            Config::load_from(&p).expect("loads").ai_monitor.width_px,
            MONITOR_WIDTH_MIN
        );
        let p = write_tmp_config("mon-clamp-down", "[ai_monitor]\nwidth_px = 5000.0\n");
        assert_eq!(
            Config::load_from(&p).expect("loads").ai_monitor.width_px,
            MONITOR_WIDTH_MAX
        );
        let p = write_tmp_config("mon-in-band", "[ai_monitor]\nwidth_px = 520.0\n");
        assert_eq!(
            Config::load_from(&p).expect("loads").ai_monitor.width_px,
            520.0
        );
        // A non-finite value falls back to the default rather than poisoning the inset.
        let p = write_tmp_config("mon-nan", "[ai_monitor]\nwidth_px = nan\n");
        assert_eq!(
            Config::load_from(&p).expect("loads").ai_monitor.width_px,
            AiMonitorConfig::default().width_px
        );
    }

    /// R6.1 — a released drag's width reaches the file, and everything else in it survives
    /// byte-for-byte: the panel inherits the sidebar's whole fidelity contract because it
    /// goes through the same editor, not a second one.
    #[test]
    fn save_ai_monitor_writes_both_keys_and_preserves_the_rest() {
        let original = "\
# my config
[font]
size_pt = 13.0

[ai_monitor]
context_window = 1000000  # 1M session
width_px = 430.0
open = false
";
        let updated = edit_ai_monitor_toml(original, 520.0, true);
        assert!(updated.contains("width_px = 520.0"));
        assert!(updated.contains("open = true"));
        assert!(updated.starts_with("# my config\n"), "comments survive");
        assert!(
            updated.contains("context_window = 1000000  # 1M session"),
            "an unrelated key and its comment are untouched: {updated:?}"
        );
        let c: Config = toml::from_str(&updated).expect("still parses");
        assert_eq!(c.ai_monitor.width_px, 520.0);
        assert!(c.ai_monitor.open);
        assert_eq!(c.font.size_pt, 13.0);

        // No [ai_monitor] table yet → one is appended, not merged into [sidebar].
        let updated = edit_ai_monitor_toml("[sidebar]\nwidth_px = 340.0\n", 300.0, true);
        let c: Config = toml::from_str(&updated).expect("parses");
        assert_eq!(
            c.sidebar.width_px, 340.0,
            "the sidebar's width is not stolen"
        );
        assert_eq!(c.ai_monitor.width_px, 300.0);
        assert!(c.ai_monitor.open);

        // And through a real file, atomically.
        let path = write_tmp_config("mon-save", "[grid]\ncolumns = 80\n");
        save_ai_monitor(&path, 520.0, true).expect("saved");
        let c = Config::load_from(&path).expect("reloads");
        assert_eq!(c.ai_monitor.width_px, 520.0);
        assert!(c.ai_monitor.open);
        assert_eq!(c.grid.columns, 80);

        // The same refusal the sidebar gets: an inline table is not corrupted.
        let path = write_tmp_config("mon-inline", "ai_monitor = { width_px = 430.0 }\n");
        assert!(save_ai_monitor(&path, 520.0, true).is_err());
    }

    /// The two writers touch different tables — a panel drag must never rewrite
    /// `[sidebar]`, which is the failure the generalisation could most easily introduce.
    #[test]
    fn the_two_writers_do_not_touch_each_others_tables() {
        let original = "[sidebar]\nwidth_px = 340.0\ncollapsed = false\n\n[ai_monitor]\nwidth_px = 430.0\nopen = false\n";
        let after_panel = edit_ai_monitor_toml(original, 520.0, true);
        let c: Config = toml::from_str(&after_panel).expect("parses");
        assert_eq!(c.sidebar.width_px, 340.0);
        assert!(!c.sidebar.collapsed);
        assert_eq!(c.ai_monitor.width_px, 520.0);

        let after_sidebar = edit_sidebar_toml(&after_panel, 400.0, true);
        let c: Config = toml::from_str(&after_sidebar).expect("parses");
        assert_eq!(c.sidebar.width_px, 400.0);
        assert!(c.sidebar.collapsed);
        assert_eq!(c.ai_monitor.width_px, 520.0, "the panel's write survives");
        assert!(c.ai_monitor.open);
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
        // The file promises "omitted fields fall back to the defaults shown here", and
        // Settings seeds a new user's config from it — so every value it states must BE
        // the default. Checking two fields let `ligatures = true` ship against a `false`
        // default, turning ligatures on for everyone whose config was seeded.
        assert_eq!(
            c,
            Config::default(),
            "config.example.toml drifted from defaults"
        );
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

    // -----------------------------------------------------------------
    // tab-resize: sidebar write-back (D8)
    // -----------------------------------------------------------------

    /// A temp dir with NO config file written — for the fresh-install path.
    fn empty_tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mac-term-save-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_preserves_comments_key_order_and_unknown_keys() {
        // R8.2 — the whole reason this is surgical instead of a serde round-trip.
        let original = "\
# my terminal config
# (hand-written, do not clobber)

[font]
size_pt = 14.0   # a bit bigger

[sidebar]
# how wide the tab list is
width_px = 340.0
show_git_branch = true
accent = 0x86B0A1
collapsed = false

[grid]
columns = 120
";
        let updated = edit_sidebar_toml(original, 400.0, false);
        assert!(updated.contains("# my terminal config"));
        assert!(updated.contains("# (hand-written, do not clobber)"));
        assert!(updated.contains("# how wide the tab list is"));
        assert!(updated.contains("size_pt = 14.0   # a bit bigger"));
        assert!(updated.contains("accent = 0x86B0A1"));
        assert!(updated.contains("columns = 120"));
        assert!(updated.contains("width_px = 400.0"));
        // Key ORDER is untouched: sidebar still precedes grid, width_px still first.
        let i_font = updated.find("[font]").unwrap();
        let i_side = updated.find("[sidebar]").unwrap();
        let i_grid = updated.find("[grid]").unwrap();
        assert!(i_font < i_side && i_side < i_grid);
        assert!(updated.find("width_px").unwrap() < updated.find("show_git_branch").unwrap());
        // And it still parses back to the value we wrote.
        let c: Config = toml::from_str(&updated).expect("still valid TOML");
        assert_eq!(c.sidebar.width_px, 400.0);
        assert_eq!(c.grid.columns, 120);
    }

    #[test]
    fn save_rewrites_only_the_target_lines() {
        // R8.2 — byte-level: exactly the two lines differ.
        let original = "[sidebar]\nwidth_px = 340.0\ncollapsed = false\naccent = 0x86B0A1\n";
        let updated = edit_sidebar_toml(original, 400.0, true);
        assert_eq!(
            updated, "[sidebar]\nwidth_px = 400.0\ncollapsed = true\naccent = 0x86B0A1\n",
            "only the two managed values change"
        );
    }

    #[test]
    fn save_inserts_a_missing_sidebar_table() {
        // R8.3 — a config with no [sidebar] table at all.
        let original = "[grid]\ncolumns = 80\n";
        let updated = edit_sidebar_toml(original, 400.0, false);
        let c: Config = toml::from_str(&updated).expect("valid after insert");
        assert_eq!(c.sidebar.width_px, 400.0);
        assert!(!c.sidebar.collapsed);
        assert_eq!(c.grid.columns, 80, "the existing table is untouched");
    }

    #[test]
    fn save_appends_missing_keys_inside_the_existing_table() {
        // A [sidebar] table that has only SOME of the keys: the missing one must land
        // inside that table, not at the file's end (where it would fall under [grid] and
        // mean something entirely different).
        let original = "[sidebar]\nwidth_px = 340.0\n\n[grid]\ncolumns = 80\n";
        let updated = edit_sidebar_toml(original, 400.0, true);
        let c: Config = toml::from_str(&updated).expect("valid");
        assert_eq!(c.sidebar.width_px, 400.0);
        assert!(c.sidebar.collapsed);
        assert_eq!(c.grid.columns, 80);
        assert!(
            updated.find("collapsed").unwrap() < updated.find("[grid]").unwrap(),
            "collapsed must sit inside [sidebar], not after [grid]:\n{updated}"
        );
    }

    #[test]
    fn save_preserves_indentation_style() {
        let original = "[sidebar]\n    width_px = 340.0\n    collapsed = false\n";
        let updated = edit_sidebar_toml(original, 300.0, true);
        assert!(updated.contains("    width_px = 300.0"));
        assert!(updated.contains("    collapsed = true"));
    }

    #[test]
    fn save_does_not_match_a_key_that_merely_starts_the_same() {
        // `width_px_extra` must not be mistaken for `width_px`. (deny_unknown_fields
        // would reject it on load, but the text transform must still be honest.)
        let original = "[sidebar]\nwidth_px_extra = 1\nwidth_px = 340.0\n";
        let updated = edit_sidebar_toml(original, 400.0, false);
        assert!(updated.contains("width_px_extra = 1"));
        assert!(updated.contains("width_px = 400.0"));
    }

    #[test]
    fn save_only_touches_keys_inside_the_sidebar_table() {
        // A `width_px` under a DIFFERENT table must be left alone.
        let original = "[other]\nwidth_px = 1.0\n\n[sidebar]\nwidth_px = 340.0\n";
        let updated = edit_sidebar_toml(original, 400.0, false);
        assert!(updated.contains("[other]\nwidth_px = 1.0"));
        assert!(updated.contains("width_px = 400.0"));
    }

    #[test]
    fn saved_float_round_trips_exactly() {
        // R8.7 / D15 — the echo predicate compares the RE-PARSED value against what we
        // wrote. A lossy format would make every one of our own writes look like a human
        // edit and clear the runtime override on every drag.
        for w in [260.0_f32, 300.5, 333.33333, 400.0, 439.99997, 440.0] {
            let updated = edit_sidebar_toml("[sidebar]\nwidth_px = 1.0\n", w, false);
            let c: Config = toml::from_str(&updated).expect("valid");
            assert_eq!(c.sidebar.width_px, w, "round-trip of {w} via {updated:?}");
        }
    }

    #[test]
    fn whole_numbers_are_written_as_floats() {
        // `width_px = 400` is a TOML *integer* and fails f32 deserialization — the value
        // must carry a decimal point.
        let updated = edit_sidebar_toml("[sidebar]\nwidth_px = 1.0\n", 400.0, false);
        assert!(updated.contains("width_px = 400.0"), "got {updated:?}");
    }

    #[test]
    fn save_sidebar_creates_dirs_and_file_on_a_fresh_install() {
        // R8.4 — Config::load returns defaults WITHOUT creating a file, so the first
        // save may be the first time the directory exists at all.
        let dir = empty_tmp_dir("fresh");
        let path = dir.join("config.toml");
        assert!(!dir.exists());
        save_sidebar(&path, 400.0, true).expect("save on a fresh install");
        let c = Config::load_from(&path).expect("loads back");
        assert_eq!(c.sidebar.width_px, 400.0);
        assert!(c.sidebar.collapsed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_sidebar_leaves_no_temp_file_and_uses_a_distinct_name() {
        // R8.5 — the temp must not be named config.toml (the watcher matches by NAME
        // over the parent dir, so a same-named temp would trip it twice), and the rename
        // must leave nothing behind.
        assert_ne!(TMP_FILE, CONFIG_FILE);
        let path = write_tmp_config("atomic", "[sidebar]\nwidth_px = 340.0\n");
        let dir = path.parent().unwrap().to_path_buf();
        save_sidebar(&path, 400.0, false).expect("save");
        assert!(
            !dir.join(TMP_FILE).exists(),
            "temp file cleaned up by rename"
        );
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .collect();
        assert_eq!(names, vec![CONFIG_FILE.to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_sidebar_round_trips_through_a_real_file() {
        let path = write_tmp_config(
            "roundtrip",
            "# keep me\n[sidebar]\nwidth_px = 340.0\ncollapsed = false\n",
        );
        save_sidebar(&path, 420.0, true).expect("save");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# keep me"), "comment survived: {text:?}");
        let c = Config::load_from(&path).expect("loads");
        assert_eq!(c.sidebar.width_px, 420.0);
        assert!(c.sidebar.collapsed);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_trailing_comment_on_the_managed_line_survives() {
        // R8.2 — found by the Gate-3 checker: the first cut replaced the whole line and
        // deleted the user's note, while the doc comment claimed it was preserved.
        let original =
            "[sidebar]\nwidth_px = 340.0  # I like it narrow\ncollapsed = false # nope\n";
        let updated = edit_sidebar_toml(original, 400.0, true);
        assert!(
            updated.contains("width_px = 400.0  # I like it narrow"),
            "got {updated:?}"
        );
        // The ORIGINAL gap before each `#` is reused, not normalised: two spaces on the
        // first line, one on the second — exactly as the user wrote them.
        assert!(
            updated.contains("collapsed = true # nope"),
            "got {updated:?}"
        );
        let c: Config = toml::from_str(&updated).expect("valid");
        assert_eq!(c.sidebar.width_px, 400.0);
        assert!(c.sidebar.collapsed);
    }

    #[test]
    fn a_quoted_table_header_is_recognised_not_duplicated() {
        // The residual corruption escape the Gate-3 re-check found: `["sidebar"]` is
        // identical TOML to `[sidebar]`, but the first cut did not match it, appended a
        // SECOND table, and produced a duplicate key — an unparseable file, which silently
        // resets the user to full defaults at next launch.
        let original = "[\"sidebar\"]\nwidth_px = 340.0\n";
        let updated = edit_sidebar_toml(original, 400.0, false);
        assert_eq!(
            updated.matches("sidebar").count(),
            1,
            "no second table appended: {updated:?}"
        );
        let c: Config = toml::from_str(&updated).expect("still parses");
        assert_eq!(c.sidebar.width_px, 400.0);
        // A quoted top-level inline table is likewise refused rather than corrupted.
        assert!(has_non_table_key(
            "\"sidebar\" = { width_px = 340.0 }\n",
            "sidebar"
        ));
        // …and so is an array-of-tables — the third door into the same duplicate-key bug.
        assert!(has_non_table_key(
            "[[sidebar]]\nwidth_px = 340.0\n",
            "sidebar"
        ));
        assert!(has_non_table_key("[[ \"sidebar\" ]]\n", "sidebar"));
        // A DIFFERENT array-of-tables is none of our business.
        assert!(!has_non_table_key("[[other]]\nx = 1\n", "sidebar"));
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        // The comment splitter must not truncate a value that legitimately contains `#`.
        assert_eq!(split_trailing_comment("a = \"x#y\"").1, "");
        assert_eq!(split_trailing_comment("a = 1 # real").1, "# real");
        assert_eq!(split_trailing_comment("a = 'x#y' # real").1, "# real");
    }

    #[test]
    fn a_stray_crlf_does_not_convert_the_whole_file() {
        // Pins the first-line-ending rule. The pure-CRLF test below passed under BOTH the
        // buggy `contains("\r\n")` implementation and the fixed one, so the fix was
        // unpinned — a "simplification" back to `contains` would have gone unnoticed.
        let original = "# note\n[sidebar]\r\nwidth_px = 340.0\n";
        let updated = edit_sidebar_toml(original, 400.0, false);
        assert!(
            !updated.starts_with("# note\r\n"),
            "a mostly-LF file must stay LF: {updated:?}"
        );
        assert!(updated.contains("width_px = 400.0"));
    }

    #[test]
    fn a_crlf_file_keeps_its_line_endings() {
        // R8.2 — `str::lines()` strips `\r`, so rejoining with `\n` would rewrite every
        // line ending in the file. Found by the Gate-3 checker.
        let original = "# note\r\n[sidebar]\r\nwidth_px = 340.0\r\n";
        let updated = edit_sidebar_toml(original, 400.0, false);
        assert!(updated.contains("\r\n"), "CRLF preserved: {updated:?}");
        assert!(
            !updated.replace("\r\n", "").contains('\n'),
            "no bare LF introduced: {updated:?}"
        );
        let c: Config = toml::from_str(&updated).expect("valid");
        assert_eq!(c.sidebar.width_px, 400.0);
    }

    #[test]
    fn an_inline_table_sidebar_is_refused_not_corrupted() {
        // The worst outcome this writer could produce: appending a second [sidebar] to a
        // file that already defines `sidebar` as an inline table makes the file
        // UNPARSEABLE, and a config that fails to parse silently drops the user back to
        // full defaults. Refusing (D14: warn and keep the session value) is the safe path.
        assert!(has_non_table_key(
            "sidebar = { width_px = 340.0, collapsed = false }\n",
            "sidebar"
        ));
        assert!(has_non_table_key("sidebar.width_px = 340.0\n", "sidebar"));
        // A `[sidebar]` section is fine, and so is a `sidebar = ...` key inside ANOTHER
        // table — that is a different key entirely.
        assert!(!has_non_table_key(
            "[sidebar]\nwidth_px = 340.0\n",
            "sidebar"
        ));
        assert!(!has_non_table_key("[other]\nsidebar = 1\n", "sidebar"));

        let path = write_tmp_config("inline", "sidebar = { width_px = 340.0 }\n");
        assert!(
            save_sidebar(&path, 400.0, false).is_err(),
            "must refuse rather than corrupt"
        );
        // The file is untouched, so it still parses exactly as before.
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "sidebar = { width_px = 340.0 }\n");
        assert!(toml::from_str::<Config>(&text).is_ok());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn save_sidebar_reports_a_write_failure_rather_than_panicking() {
        // R8.6's precondition: the failure must arrive as an Err the caller can warn on.
        // A path whose parent cannot be created (a file where a directory must go).
        let dir = empty_tmp_dir("readonly");
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let path = blocker.join("config.toml");
        assert!(save_sidebar(&path, 400.0, false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------
    // terminal-font-size: R1.1 – R1.4
    // -----------------------------------------------------------------

    #[test]
    fn font_size_constants_and_default_match_spec() {
        // R1.1 — exported constants and FontConfig::default().size_pt.
        assert_eq!(FONT_SIZE_MIN, 8.0);
        assert_eq!(FONT_SIZE_MAX, 32.0);
        assert_eq!(FONT_SIZE_DEFAULT, 14.0);
        assert_eq!(FONT_SIZE_STEP, 1.0);
        assert_eq!(FontConfig::default().size_pt, FONT_SIZE_DEFAULT);
    }

    #[test]
    fn font_size_is_clamped_and_normalized_at_load() {
        // R1.2 — out-of-range font.size_pt clamps to [8.0, 32.0] via Config::load_from.
        let p_low = write_tmp_config("font-clamp-up", "[font]\nsize_pt = 4.0\n");
        assert_eq!(
            Config::load_from(&p_low).expect("loads").font.size_pt,
            FONT_SIZE_MIN
        );
        let _ = std::fs::remove_dir_all(p_low.parent().unwrap());

        let p_high = write_tmp_config("font-clamp-down", "[font]\nsize_pt = 60.0\n");
        assert_eq!(
            Config::load_from(&p_high).expect("loads").font.size_pt,
            FONT_SIZE_MAX
        );
        let _ = std::fs::remove_dir_all(p_high.parent().unwrap());

        let p_in = write_tmp_config("font-in-band", "[font]\nsize_pt = 18.0\n");
        assert_eq!(Config::load_from(&p_in).expect("loads").font.size_pt, 18.0);
        let _ = std::fs::remove_dir_all(p_in.parent().unwrap());

        // R1.3 — non-finite font.size_pt falls back to FONT_SIZE_DEFAULT (14.0).
        let p_nan = write_tmp_config("font-nan", "[font]\nsize_pt = nan\n");
        assert_eq!(
            Config::load_from(&p_nan).expect("loads").font.size_pt,
            FONT_SIZE_DEFAULT
        );
        let _ = std::fs::remove_dir_all(p_nan.parent().unwrap());

        let p_inf = write_tmp_config("font-inf", "[font]\nsize_pt = inf\n");
        assert_eq!(
            Config::load_from(&p_inf).expect("loads").font.size_pt,
            FONT_SIZE_DEFAULT
        );
        let _ = std::fs::remove_dir_all(p_inf.parent().unwrap());
    }

    #[test]
    fn save_font_size_surgically_updates_only_size_pt() {
        // R1.4 — surgical [font].size_pt write preserves comments, family, ligatures, and
        // unrelated tables byte-for-byte.
        let original = "\
# header comment
[font]
family = \"JetBrains Mono\"
size_pt = 14.0  # baseline
ligatures = true

[sidebar]
width_px = 340.0
collapsed = false

[ai_monitor]
width_px = 430.0
open = true
";
        let path = write_tmp_config("font-save", original);
        save_font_size(&path, 16.0).expect("save_font_size succeeds");
        let text = std::fs::read_to_string(&path).unwrap();
        let expected = "\
# header comment
[font]
family = \"JetBrains Mono\"
size_pt = 16.0  # baseline
ligatures = true

[sidebar]
width_px = 340.0
collapsed = false

[ai_monitor]
width_px = 430.0
open = true
";
        assert_eq!(text, expected);
        let reloaded = Config::load_from(&path).expect("reloads");
        assert_eq!(reloaded.font.family, "JetBrains Mono");
        assert_eq!(reloaded.font.size_pt, 16.0);
        assert!(reloaded.font.ligatures);
        assert_eq!(reloaded.sidebar.width_px, 340.0);
        assert_eq!(reloaded.ai_monitor.width_px, 430.0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());

        // Fresh install without [font] table synthesizes [font] without touching [grid].
        let path_missing = write_tmp_config("font-save-insert", "[grid]\ncolumns = 110\n");
        save_font_size(&path_missing, 18.0).expect("save_font_size inserts [font]");
        let reloaded = Config::load_from(&path_missing).expect("reloads");
        assert_eq!(reloaded.font.size_pt, 18.0);
        assert_eq!(reloaded.grid.columns, 110);
        let _ = std::fs::remove_dir_all(path_missing.parent().unwrap());

        // Refuses inline table `font = { ... }` rather than corrupting the file.
        let path_inline = write_tmp_config("font-inline", "font = { size_pt = 14.0 }\n");
        assert!(save_font_size(&path_inline, 16.0).is_err());
        let _ = std::fs::remove_dir_all(path_inline.parent().unwrap());
    }
}
