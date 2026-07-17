//! Live settings application (settings feature, D3/R3).
//!
//! The [`common::watch::ConfigWatcher`] (spawned by the app loop) delivers a
//! fresh [`Config`] on change; this module applies it to the running renderer and
//! tabs without a restart:
//! - `renderer.apply_config` re-derives cell metrics, theme clear color, ligature
//!   state, and rebuilds the glyph atlas (R0.1/R3.2),
//! - the grid re-fits to the new [`Renderer::grid_size`] and every tab's PTY is
//!   resized (R3.1),
//! - each tab's default cell colors are re-themed (R3.3).
//!
//! Registered through the app owner's config-reload wake (the SD4 seam covers key
//! events; a background file-change is delivered as `UserEvent::ConfigReloaded`).

use common::config::Config;
use renderer::tab_bar;
use renderer::Renderer;
use term_core::Color;

use crate::tabs::Tabs;

/// Apply a reloaded config to the live renderer + tabs.
pub fn apply(cfg: &Config, renderer: &mut Renderer, tabs: &mut Tabs) {
    renderer.apply_config(cfg);
    // Font size may have changed the cell height → keep the tab-bar band in step
    // before reading grid_size (so the reserved strip matches the new metrics).
    renderer.set_top_inset(tab_bar::height_px(renderer.cell_size().1));
    // Cell metrics may have changed (font size) → re-fit every tab (R3.1).
    let (cols, rows) = renderer.grid_size();
    tabs.resize_all(cols, rows);
    // Re-theme default cell colors so new output uses the new theme (R3.3).
    tabs.set_default_colors_all(Color(cfg.theme.foreground), Color(cfg.theme.background));
}
