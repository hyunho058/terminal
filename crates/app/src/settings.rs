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

/// The one thing [`refit`] does with the tab set (R0.2).
///
/// A trait rather than a bare `&mut Tabs` so R0.2's middle clause — "`resize_all` was
/// called **with the new count**" — can be asserted at all. `Tabs` cannot be built under
/// `cargo test`: it holds an `EventLoopProxy`, which only an `EventLoop` mints, which winit
/// builds only on the process's main thread, while libtest runs tests on worker threads.
/// [`Tabs`] is the sole production implementor; the only other one is the spy below.
pub trait RefitTarget {
    fn resize_all(&mut self, cols: u16, rows: u16);
}

impl RefitTarget for Tabs {
    fn resize_all(&mut self, cols: u16, rows: u16) {
        // Qualified, not `self.resize_all(..)`: the inherent method and this one share a
        // name, and while inherent methods do win resolution, relying on that to avoid
        // infinite recursion is not a thing to leave to a reader.
        Tabs::resize_all(self, cols, rows)
    }
}

/// Re-fit the grid to the renderer's current geometry and resize **every** tab (D6).
/// Reads [`Renderer::grid_size`] itself — after any inset change is already applied — so
/// it can never be handed a stale column count; resizes background tabs too, since a tab
/// left at the old width renders wrapped garbage the moment it is switched to. Returns the
/// new `(cols, rows)`.
pub fn refit(renderer: &Renderer, tabs: &mut impl RefitTarget) -> (u16, u16) {
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

    /// Records what [`refit`] handed it, in order.
    #[derive(Default)]
    struct SpyTabs(Vec<(u16, u16)>);

    impl RefitTarget for SpyTabs {
        fn resize_all(&mut self, cols: u16, rows: u16) {
            self.0.push((cols, rows));
        }
    }

    /// R0.2 — the reflow reaches the tabs, and it reaches them with the **new** count.
    ///
    /// Opening the ai-monitor panel is the change under test because it is the one R0.2
    /// names: the inset moves, `grid_size` reports fewer columns, and `refit` must carry
    /// *that* number rather than the one it saw last frame. A `refit` that re-sent the old
    /// count would still return something plausible, which is why the spy asserts the whole
    /// call sequence and not just the final value.
    ///
    /// **Two coverage limits, stated rather than implied.**
    ///
    /// * **The PTY half of R0.2 is not asserted here, and is not reachable under
    ///   `cargo test`.** "The PTY received the matching window size" lives in
    ///   `Session::resize`, and a `Session` needs an `EventLoopProxy` for its reader
    ///   thread — hence an `EventLoop`, which winit builds only on the process's main
    ///   thread while libtest runs tests on workers. `Tabs`'s own fan-out over *background*
    ///   tabs (R5.1) is unreachable for the same reason: it is a loop over `Session`s.
    ///   Both are recorded as limits in `specs/ai-monitor-right-panel/spec.md`.
    /// * **This test needs a GPU adapter and a system font** for `Renderer::new_headless`,
    ///   and skips without either — the same guard the renderer crate's own GPU tests use.
    #[test]
    fn r0_2_refit_hands_the_live_grid_size_to_the_tabs() {
        let cfg = Config::default();
        let Ok(mut renderer) = Renderer::new_headless(1800, 900, 1.0, &cfg) else {
            eprintln!("skipping r0_2_refit_hands_the_live_grid_size_to_the_tabs: no GPU/font");
            return;
        };
        let mut tabs = SpyTabs::default();

        let closed = refit(&renderer, &mut tabs);
        assert_eq!(
            closed,
            renderer.grid_size(),
            "refit must return the size it fitted to"
        );
        assert_eq!(tabs.0, vec![closed], "…and hand that same size to the tabs");

        // Open the panel: the band is reserved, so the column count drops (R0.1).
        renderer.set_monitor_open(true);
        assert!(
            renderer.right_inset() > 0.0,
            "premise: the panel reserved a band at this window size"
        );
        let open = refit(&renderer, &mut tabs);
        assert!(
            open.0 < closed.0,
            "the panel must cost the terminal columns: {} → {}",
            closed.0,
            open.0
        );
        assert_eq!(open.1, closed.1, "rows are unaffected (R0.1)");
        assert_eq!(open, renderer.grid_size());
        assert_eq!(
            tabs.0,
            vec![closed, open],
            "the second refit must carry the NEW count, not repeat the first"
        );

        // Closing gives the columns back, exactly (R0.3).
        renderer.set_monitor_open(false);
        let reopened = refit(&renderer, &mut tabs);
        assert_eq!(reopened, closed, "closing restores the original size");
        assert_eq!(tabs.0, vec![closed, open, closed]);
    }

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
