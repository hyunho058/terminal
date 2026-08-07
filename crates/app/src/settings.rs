//! Live settings application (settings feature, D3/R3) + the shared grid-refit helper.
//!
//! The [`common::watch::ConfigWatcher`] (spawned by the app loop) delivers a fresh
//! [`Config`] on change; [`apply`] applies it to the running renderer, tabs and preedit
//! layer without a restart:
//! - `renderer.apply_config` re-derives cell metrics, theme clear color, ligature state,
//!   rebuilds the glyph atlas, and re-fits **both** side insets — the sidebar's via
//!   `sync_left_inset` (sidebar-row-redesign D4 — so this module no longer touches
//!   `set_left_inset`) and the ai-monitor panel's via `sync_right_inset`
//!   (ai-monitor-right-panel D21/R4.6, so a reload for an unrelated key cannot drop the
//!   panel's band while the panel keeps painting),
//! - [`refit`] re-fits the grid to the new [`Renderer::grid_size`] and resizes every tab,
//! - each tab's default cell colors are re-themed,
//! - the inline IME preedit layer is re-themed (hangul-ime R7.5): its colors ride on the
//!   shared handle rather than `FrameCtx`, so nothing else would update them.
//!
//! [`refit`] is the ONE place the grid is re-fitted (D6): every path that can change a
//! reserved inset or the window size — startup, config reload, sidebar collapse/expand,
//! the ai-monitor panel's toggle and edge drag, and `Resized` — calls it, so the "change
//! the inset first, then read `grid_size`" ordering has a single enforcement point instead
//! of seven conventions.

use common::config::Config;
use renderer::preedit_layer::PreeditHandle;
use renderer::Renderer;
use term_core::Color;

use crate::tabs::Tabs;

/// Re-fit the grid to the renderer's current geometry and resize **every** tab (D6).
/// Reads [`Renderer::grid_size`] itself — after any inset change is already applied — so
/// it can never be handed a stale column count; resizes background tabs too, since a tab
/// left at the old width renders wrapped garbage the moment it is switched to. Returns the
/// new `(cols, rows)`.
pub fn refit(renderer: &Renderer, tabs: &mut Tabs) -> (u16, u16) {
    let (cols, rows) = renderer.grid_size();
    tabs.resize_all(cols, rows);
    (cols, rows)
}

/// Apply a reloaded config to the live renderer + tabs + preedit layer.
pub fn apply(cfg: &Config, renderer: &mut Renderer, tabs: &mut Tabs, preedit: &PreeditHandle) {
    // `apply_config` re-derives cell metrics + atlas AND re-fits the sidebar inset
    // (`sync_left_inset`, D4), so the old `set_left_inset(width_px(..))` call — a no-op
    // computing a stale `cell_w × 14` under D4 — is deleted (R7.1).
    renderer.apply_config(cfg);
    refit(renderer, tabs);
    tabs.set_default_colors_all(Color(cfg.theme.foreground), Color(cfg.theme.background));
    // Re-theme the inline IME composition (hangul-ime R7.5/D14). The preedit paints an
    // opaque theme-background fill under its glyphs; leaving it on the old theme would
    // stamp a stale-colored block over the newly themed grid.
    let (bg, fg) = preedit_theme(cfg);
    preedit.set_theme(bg, fg);
}

/// The preedit layer's `(bg, fg)` for a config (R7.5/D14).
///
/// Split out so the mapping is testable without a GPU or a PTY: `set_theme`'s two
/// arguments are the same type, so a transposition compiles cleanly and would paint
/// foreground-colored blocks over background-colored glyphs — invisible text that no
/// other test in the workspace would catch.
fn preedit_theme(cfg: &Config) -> (u32, u32) {
    (cfg.theme.background, cfg.theme.foreground)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R7.5: the preedit rides the reloaded theme, background first.
    #[test]
    fn preedit_theme_is_background_then_foreground() {
        let mut cfg = Config::default();
        cfg.theme.background = 0x11_22_33;
        cfg.theme.foreground = 0xAA_BB_CC;
        assert_eq!(preedit_theme(&cfg), (0x11_22_33, 0xAA_BB_CC));
    }

    /// A theme whose fg and bg differ must not collapse — guards the transposition
    /// directly (equal values would make the swap undetectable).
    #[test]
    fn preedit_theme_does_not_transpose() {
        let cfg = Config::default();
        let (bg, fg) = preedit_theme(&cfg);
        assert_eq!(bg, cfg.theme.background, "fill uses the theme background");
        assert_eq!(fg, cfg.theme.foreground, "glyphs use the theme foreground");
    }
}
