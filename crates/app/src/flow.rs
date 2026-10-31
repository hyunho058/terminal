//! The Agent Monitor overlay (session-overview-ui, revising session-flow-graph): a
//! Cmd+Shift+F full-window overlay showing this window's agent sessions as an overview table —
//! tiles, a `Waiting on you` banner, filter tabs and one row per session — and, for one
//! session, a detail screen with its timeline. Registered through the SD4 `Overlay` seam and
//! drawn by the renderer's `overview_panel` over the whole viewport, the ai-monitor included
//! (carried D4).
//!
//! Two halves, kept apart on purpose:
//!
//! * [`build_overview`] and [`build_detail`] are the whole policy, pure, in
//!   `overview.rs` / `overview_detail.rs` (D20, D23).
//! * [`FlowOverlay`] is the impure shell: the open/close edges, the graph request to the
//!   tabmeta worker, the digest thread, the view state (overview / detail), the key, press,
//!   wheel and hover routes, and the per-frame "did the key move" check that decides whether
//!   the policy runs at all (D18).
//!
//! **Status is the card's, by construction** (carried D3): `build_overview` takes each
//! session's state from the sidebar card's own join; this module classifies nothing.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use renderer::overview_model::{Filter, OverviewHit, OverviewModel, View};
use renderer::overview_panel::{OverviewHandle, OverviewPanel};
use renderer::sidebar_row::{SidebarRow, TabModelHandle};
use renderer::{DrawLayer, FrameCtx};
use winit::keyboard::{Key, NamedKey};

use crate::flow_telemetry::{self, DigestSlot, FlowTelemetry, SessionDigest};
use crate::input::{self, WheelDelta};
use crate::overview::{admits, build_overview, session_refs, ExpandSet};
use crate::overview_detail::build_detail;
use crate::seam::{AppCtx, Handled, KeyInput, Overlay, OverlayCommand};
use crate::session::TabId;
use crate::tabmeta::{TabMeta, TabMetaHandle, TabMetaSnapshot};

/// Which screen the overlay shows (D10). Opening always shows the overview.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewState {
    #[default]
    Overview,
    /// One session's detail, and the lane the user picked (`None`: the newest, D13).
    Detail {
        tab: u64,
        pane: u32,
        selected: Option<usize>,
    },
}

/// D18's cache key: the model is rebuilt only when one of these moved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RebuildKey {
    tabmeta: u64,
    /// Beside the generation, not folded into it: a snapshot stood in through
    /// [`FlowOverlay::sync_snapshot`] may flip readiness at an unchanged generation.
    ready: bool,
    rows_rev: u64,
    active: usize,
    /// The digest thread's publish count.
    digests: u64,
    /// Bumped on every filter, expand, view or selection change.
    ui: u64,
    /// `now_ms / 60_000`: every elapsed text is in whole minutes, so a minute boundary is the
    /// only moment the clock alone can change the model (D18).
    minute: u64,
}

/// Whether `snap` says this open's repositories are resolved (carried D10/D16): ready, and —
/// when there is a worker — ready for the open that asked (`epoch`), not an earlier one.
fn snapshot_ready(snap: &TabMetaSnapshot, epoch: Option<u64>) -> bool {
    snap.graph_ready() && epoch.is_none_or(|e| snap.graph_epoch() == e)
}

/// The wall clock in epoch ms.
fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The local UTC offset at `now_ms`, in seconds east of UTC, for the detail's `HH:MM` labels.
/// `0` when the C library cannot answer.
fn local_utc_offset_s(now_ms: u64) -> i32 {
    let t = libc::time_t::try_from(now_ms / 1000).unwrap_or(libc::time_t::MAX);
    // SAFETY: `tm` is a plain C struct for which all-zero is a valid value; `localtime_r` is
    // the re-entrant form, writing only into the `tm` we own and reading only `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let out = unsafe { libc::localtime_r(&t, &mut tm) };
    if out.is_null() {
        return 0;
    }
    i32::try_from(tm.tm_gmtoff).unwrap_or(0)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// What the overlay and its draw layer share.
///
/// Shared because the rebuild runs in **two** places: `Overlay::poll` (a worker or digest
/// wake) and the layer's `record`, once per frame. The second is what "a session's state
/// always equals its tab's sidebar card" needs: a blocking dialog appearing changes
/// `rows_rev` on the PTY-output path, which wakes no overlay — and it is also where the
/// minute term is noticed. The check is a few integer compares; the policy runs only when one
/// differs.
struct Core {
    handle: OverviewHandle,
    tabmeta: Option<TabMetaHandle>,
    snap: TabMetaSnapshot,
    /// Kept across close and reopen within one run, pruned on every ready rebuild (D8, D10).
    expanded: ExpandSet,
    /// Kept across close and reopen within one run; `All` at startup (D6).
    filter: Filter,
    view: ViewState,
    /// The overview's scroll, saved while a detail is shown (D10: Back restores it).
    overview_scroll: usize,
    /// The `ui` part of the key.
    ui_rev: u64,
    /// The current spawn's slot (old D10: replaced on every open).
    slot: DigestSlot,
    /// Monotonic across opens (old D10).
    generation: Arc<AtomicU64>,
    telemetry: Option<FlowTelemetry>,
    /// What the thread was last told to follow.
    logs: Vec<PathBuf>,
    key: Option<RebuildKey>,
    /// How many times the key moved and the policy ran (R10.1, R10.2).
    rebuilds: u64,
    /// The test seam's clock; `None` reads the wall clock.
    now_ms: Option<u64>,
    /// The shown detail's `Copy resume cmd` text (D22).
    resume: Option<String>,
    open: bool,
    /// The epoch this open asked the worker for (carried D10), `None` with no worker. A
    /// snapshot's readiness counts only under this epoch: a reopen's first `sync` can copy
    /// the previous open's `graph_ready` and repositories before the worker has reached the
    /// queued close and open, and those are exactly the stale groups D17 forbids showing.
    epoch: Option<u64>,
}

impl Core {
    fn now(&self) -> u64 {
        self.now_ms.unwrap_or_else(wall_ms)
    }

    /// A filter, expand, view or selection change: the next check rebuilds.
    fn touch(&mut self) {
        self.ui_rev = self.ui_rev.wrapping_add(1);
    }

    fn metas(&self) -> HashMap<TabId, TabMeta> {
        self.snap.iter().map(|(id, m)| (id, m.clone())).collect()
    }

    /// Whether the session `(tab, pane)` exists **now** — the current rows and snapshot, not
    /// the last rebuild's (D10: a press on a session closed since does nothing).
    fn exists(&mut self, tab: u64, pane: u32) -> bool {
        if let Some(h) = &self.tabmeta {
            h.sync(&mut self.snap);
        }
        if !snapshot_ready(&self.snap, self.epoch) {
            return false;
        }
        let rows: Vec<SidebarRow> = TabModelHandle::shared().with(|m| m.rows.clone());
        session_refs(&rows, &self.metas())
            .iter()
            .any(|s| s.tab == tab && s.pane == pane)
    }

    /// Show the overview again, its saved scroll restored (D10).
    fn show_overview(&mut self) {
        self.view = ViewState::Overview;
        self.handle.restore_scroll(self.overview_scroll);
    }

    /// Rebuild iff the key moved; `true` when the model changed.
    fn refresh(&mut self) -> bool {
        if !self.open {
            return false;
        }
        if let Some(h) = &self.tabmeta {
            h.sync(&mut self.snap);
        }
        let now = self.now();
        let shared = TabModelHandle::shared();
        let (rows_rev, active) = shared.with(|m| (m.rows_rev, m.active));
        let key = RebuildKey {
            tabmeta: self.snap.generation(),
            ready: snapshot_ready(&self.snap, self.epoch),
            rows_rev,
            active,
            digests: self.generation.load(Ordering::SeqCst),
            ui: self.ui_rev,
            minute: now / 60_000,
        };
        if self.key == Some(key) {
            return false;
        }
        self.key = Some(key);
        self.rebuilds += 1;
        let rows: Vec<SidebarRow> = shared.with(|m| m.rows.clone());
        let metas = self.metas();
        let refs = session_refs(&rows, &metas);
        let mut logs: Vec<PathBuf> = refs.iter().filter_map(|s| s.log.clone()).collect();
        logs.sort();
        logs.dedup();
        if logs != self.logs {
            if let Some(t) = &self.telemetry {
                t.update_logs(logs.clone());
            }
            self.logs = logs;
        }
        let before = self.handle.rev();
        if !key.ready {
            // R2.5 / D17: no rows until every tab's repository is known, so a session never
            // appears ungrouped and then jumps.
            self.resume = None;
            self.handle.set(OverviewModel {
                resolving: true,
                filter: self.filter,
                ..OverviewModel::default()
            });
            return self.handle.rev() != before;
        }
        let slot = self.slot.clone();
        let digests = lock(&slot);
        // `$HOME` at the point of use, as `SidebarMetaSource::fill` reads it, so the rows
        // resolve exactly as the sidebar's do.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        // Built unfiltered so a detail can find its session under any filter; the filter is
        // applied to the table rows below, through the same predicate.
        let (mut model, kept) = build_overview(
            &rows,
            &metas,
            home.as_deref(),
            &digests,
            &self.expanded,
            Filter::All,
            active,
            now,
        );
        self.expanded = kept;
        self.resume = None;
        if let ViewState::Detail {
            tab,
            pane,
            selected,
        } = self.view
        {
            match model.rows.iter().find(|r| r.tab == tab && r.pane == pane) {
                Some(row) => {
                    let digest: Option<&SessionDigest> = refs
                        .iter()
                        .find(|s| s.tab == tab && s.pane == pane)
                        .and_then(|s| s.log.as_ref())
                        .and_then(|l| digests.get(l));
                    let tz = local_utc_offset_s(now);
                    let detail = build_detail(row, digest, selected, now, tz);
                    self.resume.clone_from(&detail.resume_cmd);
                    model.view = View::Detail(Box::new(detail));
                }
                // R7.5: the session vanished — no stale detail.
                None => self.show_overview(),
            }
        }
        drop(digests);
        let filter = self.filter;
        model.rows.retain(|r| admits(filter, r.bucket));
        model.filter = filter;
        self.handle.set(model);
        self.handle.rev() != before
    }
}

/// The Agent Monitor overlay (carried D4/D5, D10, D18).
pub struct FlowOverlay {
    visible: bool,
    wake: Arc<dyn Fn() + Send + Sync>,
    core: Rc<RefCell<Core>>,
    /// Sub-line wheel travel, in lines, carried between events.
    wheel_residue: f32,
}

impl FlowOverlay {
    /// `wake` is the capture-style loop wake the digest thread calls after a publish;
    /// `tabmeta` is the worker's handle (`None` in tests: the overview then builds from the
    /// snapshot a test stands in through [`sync_snapshot`](Self::sync_snapshot)); `handle` is
    /// shared with the draw layer.
    pub fn new(
        wake: Arc<dyn Fn() + Send + Sync>,
        tabmeta: Option<TabMetaHandle>,
        handle: OverviewHandle,
    ) -> FlowOverlay {
        // Starts closed and is never restored open (carried D4); say so on the handle rather
        // than trust its default.
        handle.set_visible(false);
        let core = Core {
            handle,
            tabmeta,
            snap: TabMetaSnapshot::default(),
            expanded: ExpandSet::new(),
            filter: Filter::All,
            view: ViewState::Overview,
            overview_scroll: 0,
            ui_rev: 0,
            slot: Arc::new(Mutex::new(HashMap::new())),
            generation: Arc::new(AtomicU64::new(0)),
            telemetry: None,
            logs: Vec::new(),
            key: None,
            rebuilds: 0,
            now_ms: None,
            resume: None,
            open: false,
            epoch: None,
        };
        FlowOverlay {
            visible: false,
            wake,
            core: Rc::new(RefCell::new(core)),
            wheel_residue: 0.0,
        }
    }

    /// How many times the model was rebuilt — the key moved and the policy ran (R10.1,
    /// R10.2). A rebuild that produced an identical model still counts.
    pub fn rebuilds(&self) -> u64 {
        self.core.try_borrow().map_or(0, |c| c.rebuilds)
    }

    /// Whether the overlay holds a digest thread that has not exited (R10.2).
    pub fn telemetry_running(&self) -> bool {
        self.core.try_borrow().is_ok_and(|c| {
            c.telemetry
                .as_ref()
                .is_some_and(|t| !FlowTelemetry::is_finished(t))
        })
    }

    /// Pin the clock (test seam, R10.1): every later rebuild-key check and build reads
    /// `now_ms` instead of the wall clock.
    pub fn set_now_ms(&mut self, now_ms: u64) {
        if let Ok(mut core) = self.core.try_borrow_mut() {
            core.now_ms = Some(now_ms);
        }
    }

    /// Stand `snap` in as the worker's latest publish (test seam). `poll` reaches the same
    /// state through the worker handle's own `sync`; the key is dropped here because a
    /// stood-in snapshot need not move the generation.
    pub fn sync_snapshot(&mut self, snap: TabMetaSnapshot) {
        if let Ok(mut core) = self.core.try_borrow_mut() {
            core.snap = snap;
            core.key = None;
        }
    }

    /// Is this the Cmd+Shift+F chord (carried D5)? Case-folded like the monitor's
    /// `is_toggle`.
    fn is_toggle(input: &KeyInput) -> bool {
        if !(input.mods.super_key() && input.mods.shift_key()) {
            return false;
        }
        if input.mods.control_key() || input.mods.alt_key() {
            return false;
        }
        matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("f"))
    }

    /// Show or hide the overlay — the only place `visible` changes. Edge-triggered: the open
    /// path runs only on hidden → visible and the close path only on visible → hidden, so a
    /// repeat never respawns the digest thread on a fresh slot.
    fn set_shown(&mut self, shown: bool) {
        if shown && !self.visible {
            self.open();
        } else if !shown && self.visible {
            self.close();
        }
        self.visible = shown;
    }

    /// The hidden → visible edge (carried D4/D10): ask the worker for panes and
    /// repositories, start a digest thread on a fresh slot, and show the layer — the
    /// overview at the top, reading `resolving…` until the worker's first full pass for this
    /// open. The filter and the expanded set are kept (D6, D10).
    fn open(&mut self) {
        self.wheel_residue = 0.0;
        let Ok(mut core) = self.core.try_borrow_mut() else {
            return;
        };
        core.epoch = core.tabmeta.as_ref().map(|h| h.set_graph_wanted(true));
        let slot: DigestSlot = Arc::new(Mutex::new(HashMap::new()));
        core.slot = slot.clone();
        // The first rebuild hands the thread its logs; spawning with none keeps one
        // derivation of "which logs" (`session_refs`) rather than two.
        core.logs = Vec::new();
        core.telemetry = Some(flow_telemetry::spawn(
            Vec::new(),
            slot,
            core.generation.clone(),
            self.wake.clone(),
        ));
        core.snap = TabMetaSnapshot::default();
        core.key = None;
        // D10: a reopen shows the overview, every scroll at the top.
        core.view = ViewState::Overview;
        core.overview_scroll = 0;
        core.resume = None;
        let filter = core.filter;
        core.handle.set(OverviewModel {
            resolving: true,
            filter,
            ..OverviewModel::default()
        });
        core.handle.restore_scroll(0);
        core.open = true;
        core.handle.set_visible(true);
    }

    /// The visible → hidden edge: the reverse, and the thread is told to stop, never joined.
    fn close(&mut self) {
        let Ok(mut core) = self.core.try_borrow_mut() else {
            return;
        };
        if let Some(h) = &core.tabmeta {
            h.set_graph_wanted(false);
        }
        if let Some(t) = core.telemetry.take() {
            t.stop();
        }
        core.open = false;
        core.key = None;
        core.handle.set_hover(None);
        core.handle.set_visible(false);
    }

    /// A jump hit (`Open`, `Banner`, `Focus`): switch to the tab — panes are not focusable
    /// from here (carried) — and close.
    fn jump(&mut self, tab: u64, ctx: &mut AppCtx) {
        ctx.activate_tab(TabId(tab));
        self.set_shown(false);
    }

    /// D10/D20: act on what a press landed on. Session-addressed hits act only when their
    /// `(tab, pane)` exists at press time.
    fn dispatch(&mut self, hit: OverviewHit, ctx: &mut AppCtx) {
        let session = match hit {
            OverviewHit::Row(t, p)
            | OverviewHit::Expand(t, p)
            | OverviewHit::Open(t, p)
            | OverviewHit::Banner(t, p)
            | OverviewHit::Focus(t, p) => Some((t, p)),
            _ => None,
        };
        if let Some((tab, pane)) = session {
            let alive = self
                .core
                .try_borrow_mut()
                .is_ok_and(|mut c| c.exists(tab, pane));
            if !alive {
                return;
            }
        }
        match hit {
            OverviewHit::Open(tab, _)
            | OverviewHit::Banner(tab, _)
            | OverviewHit::Focus(tab, _) => {
                self.jump(tab, ctx);
                return;
            }
            OverviewHit::CopyResume => {
                let cmd = self.core.try_borrow().ok().and_then(|c| c.resume.clone());
                if let Some(cmd) = cmd {
                    ctx.copy_to_clipboard(cmd);
                }
                return;
            }
            _ => {}
        }
        let Ok(mut core) = self.core.try_borrow_mut() else {
            return;
        };
        match hit {
            OverviewHit::Filter(f) => {
                // D6: the table starts at the top under a new filter.
                core.filter = f;
                core.handle.set_scroll(0);
            }
            OverviewHit::Row(tab, pane) => {
                core.overview_scroll = core.handle.scroll();
                core.view = ViewState::Detail {
                    tab,
                    pane,
                    selected: None,
                };
                core.handle.restore_scroll(0);
            }
            OverviewHit::Expand(tab, pane) => {
                let key = (TabId(tab), pane);
                if !core.expanded.remove(&key) {
                    core.expanded.insert(key);
                }
            }
            OverviewHit::Back => {
                if core.view == ViewState::Overview {
                    return;
                }
                core.show_overview();
            }
            OverviewHit::Lane(i) => match &mut core.view {
                ViewState::Detail { selected, .. } => *selected = Some(i),
                ViewState::Overview => return,
            },
            // Handled above.
            OverviewHit::Open(..)
            | OverviewHit::Banner(..)
            | OverviewHit::Focus(..)
            | OverviewHit::CopyResume => return,
        }
        core.touch();
    }
}

impl Overlay for FlowOverlay {
    /// Above rename (200) and the ai-monitor (100): an open overlay covers both, so it sees
    /// every key first (carried D5).
    fn priority(&self) -> i32 {
        300
    }

    fn visible(&self) -> bool {
        self.visible
    }

    /// D10. Cmd+Shift+F toggles the overlay from either view. Open, it owns the keyboard:
    /// `Esc` returns from the detail to the overview and closes the overview; every other key
    /// is swallowed, since typing into a covered screen is invisible input. Closed, every
    /// other key is someone else's.
    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        if Self::is_toggle(input) {
            self.set_shown(!self.visible);
            ctx.request_redraw();
            return Handled::Consumed;
        }
        if !self.visible {
            return Handled::Passthrough;
        }
        if input.key == Key::Named(NamedKey::Escape) {
            let in_detail = self
                .core
                .try_borrow()
                .is_ok_and(|c| c.view != ViewState::Overview);
            if in_detail {
                if let Ok(mut core) = self.core.try_borrow_mut() {
                    core.show_overview();
                    core.touch();
                }
            } else {
                self.set_shown(false);
            }
            ctx.request_redraw();
        }
        Handled::Consumed
    }

    /// An IME syllable or composition is a key too (carried D5): `Ime::Commit` bypasses
    /// `on_key` entirely, so without this a Hangul syllable typed under the open overlay
    /// would land in the covered shell. Everything else — `RestoreMonitor` above all, which
    /// is first-wins and must reach the ai-monitor below — passes.
    fn on_command(&mut self, cmd: &OverlayCommand) -> Handled {
        match cmd {
            OverlayCommand::ImeCommit(_) | OverlayCommand::ImePreedit(_) if self.visible => {
                Handled::Consumed
            }
            _ => Handled::Passthrough,
        }
    }

    /// The overlay covers the window while open (carried D4/D6), so the app routes every
    /// press, wheel and hover here first.
    fn covers_window(&self) -> bool {
        true
    }

    /// One notch is one line of the shown list (the table, or the detail's lanes), clamped
    /// against the published bound by the handle. Positive notches move down, while winit's
    /// positive wheel is toward the top — hence the negation. Pixel (trackpad) travel banks
    /// in lines, one line per notch's worth of terminal travel (`LINES_PER_NOTCH` lines), so
    /// a mouse and a trackpad move the list alike. Always consumed while open.
    fn on_wheel(&mut self, delta: WheelDelta, cell_h: f32) -> Handled {
        if !self.visible {
            return Handled::Passthrough;
        }
        let notch_px = input::LINES_PER_NOTCH * cell_h;
        if notch_px > 0.0 {
            self.wheel_residue -= input::panel_wheel_px(delta, cell_h) / notch_px;
        }
        let notches = self.wheel_residue.trunc();
        self.wheel_residue -= notches;
        if notches != 0.0 {
            if let Ok(core) = self.core.try_borrow() {
                let at = core.handle.scroll();
                let moved = notches.abs() as usize;
                let to = if notches > 0.0 {
                    at.saturating_add(moved)
                } else {
                    at.saturating_sub(moved)
                };
                core.handle.set_scroll(to);
            }
        }
        Handled::Consumed
    }

    /// D10/D20: the press resolves against the last composed frame's hits and acts through
    /// [`dispatch`](Self::dispatch). Every press is consumed while open — one falling through
    /// would act on a screen the user cannot see.
    fn on_press(&mut self, point: [f32; 2], ctx: &mut AppCtx) -> Handled {
        if !self.visible {
            return Handled::Passthrough;
        }
        let hit = self
            .core
            .try_borrow()
            .ok()
            .and_then(|c| c.handle.hit_at(point));
        if let Some(hit) = hit {
            self.dispatch(hit, ctx);
            ctx.request_redraw();
        }
        Handled::Consumed
    }

    /// The layer resolves the point against the frame it draws.
    fn on_hover(&mut self, point: Option<[f32; 2]>) {
        if let Ok(core) = self.core.try_borrow() {
            core.handle.set_hover(point);
        }
    }

    fn layer(&self) -> Box<dyn DrawLayer> {
        let handle = self.core.borrow().handle.clone();
        Box::new(FlowLayer {
            panel: OverviewPanel::new(handle),
            core: self.core.clone(),
        })
    }

    /// A worker or digest wake: rebuild iff the key moved, and report whether the model did.
    /// A hidden overlay does nothing (R10.2).
    fn poll(&mut self) -> bool {
        if !self.visible {
            return false;
        }
        match self.core.try_borrow_mut() {
            Ok(mut core) => core.refresh(),
            Err(_) => false,
        }
    }
}

/// The overlay's layer: D18's per-frame key check, then the renderer's full-window panel.
struct FlowLayer {
    panel: OverviewPanel,
    core: Rc<RefCell<Core>>,
}

impl DrawLayer for FlowLayer {
    fn z_order(&self) -> i32 {
        self.panel.z_order()
    }

    fn record(&mut self, ctx: &mut FrameCtx<'_>) {
        // `try_borrow_mut`: an interleaving that ever made this re-entrant skips a rebuild
        // for one frame instead of aborting.
        if let Ok(mut core) = self.core.try_borrow_mut() {
            core.refresh();
        }
        self.panel.record(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabmeta::{Agent, AgentKind, AgentState};
    use winit::keyboard::ModifiersState as M;

    fn row(id: u64) -> SidebarRow {
        SidebarRow {
            tab_id: id,
            title: format!("t{id}"),
            ..SidebarRow::default()
        }
    }

    fn chord(ch: &str, mods: M) -> KeyInput {
        KeyInput {
            key: Key::Character(ch.into()),
            text: Some(ch.into()),
            mods,
            physical: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyF),
        }
    }

    fn esc() -> KeyInput {
        KeyInput {
            key: Key::Named(NamedKey::Escape),
            text: Some("\u{1b}".into()),
            mods: M::empty(),
            physical: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Escape),
        }
    }

    fn ctx_run<R>(f: impl FnOnce(&mut AppCtx) -> R) -> (R, Vec<crate::seam::AppRequest>) {
        let mut pty = Vec::new();
        let mut ctx = AppCtx::new(&mut pty, 1, 0, None);
        let r = f(&mut ctx);
        let reqs = ctx.take_requests();
        (r, reqs)
    }

    /// An open overlay whose last frame published `max` lines of scroll.
    fn open_over(max: usize) -> (FlowOverlay, OverviewHandle) {
        let handle = OverviewHandle::new();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        o.set_shown(true);
        handle.publish(Vec::new(), max);
        (o, handle)
    }

    /// One unsplit Claude tab (id 1) on `log`, published to this thread's sidebar model and
    /// stood in as the worker's ready snapshot.
    fn publish_one_session(o: &mut FlowOverlay, log: &std::path::Path) {
        TabModelHandle::shared().set(renderer::sidebar_row::TabBarModel {
            titles: vec![row(1)],
            ..Default::default()
        });
        o.sync_snapshot(TabMetaSnapshot::for_test(
            vec![(
                TabId(1),
                TabMeta {
                    agent: Some(Agent {
                        kind: AgentKind::Claude,
                        state: AgentState::Busy,
                    }),
                    log: Some(log.to_path_buf()),
                    ..TabMeta::default()
                },
            )],
            true,
        ));
    }

    #[test]
    fn the_chord_needs_exactly_cmd_and_shift() {
        assert!(FlowOverlay::is_toggle(&chord("F", M::SUPER | M::SHIFT)));
        assert!(FlowOverlay::is_toggle(&chord("f", M::SUPER | M::SHIFT)));
        assert!(!FlowOverlay::is_toggle(&chord(
            "F",
            M::SUPER | M::SHIFT | M::ALT
        )));
        assert!(!FlowOverlay::is_toggle(&chord(
            "F",
            M::SUPER | M::SHIFT | M::CONTROL
        )));
        assert!(!FlowOverlay::is_toggle(&chord("F", M::SUPER)));
        assert!(!FlowOverlay::is_toggle(&chord("F", M::SHIFT)));
        assert!(!FlowOverlay::is_toggle(&chord("g", M::SUPER | M::SHIFT)));
    }

    #[test]
    fn an_open_overlay_takes_ime_text_and_a_closed_one_passes_it() {
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, OverviewHandle::new());
        let commit = OverlayCommand::ImeCommit("하".into());
        let preedit = OverlayCommand::ImePreedit("ㅎ".into());
        assert_eq!(o.on_command(&commit), Handled::Passthrough);
        assert_eq!(o.on_command(&preedit), Handled::Passthrough);
        o.set_shown(true);
        assert_eq!(o.on_command(&commit), Handled::Consumed);
        assert_eq!(o.on_command(&preedit), Handled::Consumed);
        assert_eq!(o.on_command(&OverlayCommand::Cancel), Handled::Passthrough);
        assert!(o.visible(), "Cancel does not close the overlay (D5)");
    }

    #[test]
    fn the_wheel_moves_whole_lines_and_banks_trackpad_travel() {
        let (mut o, handle) = open_over(40);
        // One notch down (winit negative) is one line.
        assert_eq!(o.on_wheel(WheelDelta::Lines(-1.0), 10.0), Handled::Consumed);
        assert_eq!(handle.scroll(), 1);
        // A notch's worth of terminal travel is 3 lines × 10 px: 29 px banks…
        assert_eq!(
            o.on_wheel(WheelDelta::Pixels(-29.0), 10.0),
            Handled::Consumed
        );
        assert_eq!(handle.scroll(), 1);
        // …and the thirtieth completes one line, leaving nothing behind.
        o.on_wheel(WheelDelta::Pixels(-1.0), 10.0);
        assert_eq!(handle.scroll(), 2);
        o.on_wheel(WheelDelta::Pixels(-15.0), 10.0);
        assert_eq!(handle.scroll(), 2, "half a notch does not move");
        // Up toward the top, clamped there, and still the overlay's.
        assert_eq!(o.on_wheel(WheelDelta::Lines(9.0), 10.0), Handled::Consumed);
        assert_eq!(handle.scroll(), 0);
        // Down past the bound: clamped to it.
        o.on_wheel(WheelDelta::Lines(-90.0), 10.0);
        assert_eq!(handle.scroll(), 40);
    }

    #[test]
    fn a_degenerate_cell_height_moves_nothing_and_poisons_nothing() {
        let (mut o, handle) = open_over(40);
        for cell_h in [0.0, -10.0, f32::NAN] {
            assert_eq!(
                o.on_wheel(WheelDelta::Pixels(-25.0), cell_h),
                Handled::Consumed
            );
            assert_eq!(
                o.on_wheel(WheelDelta::Lines(-2.0), cell_h),
                Handled::Consumed
            );
            assert_eq!(handle.scroll(), 0, "cell_h {cell_h}");
        }
        o.on_wheel(WheelDelta::Lines(-2.0), 10.0);
        assert_eq!(handle.scroll(), 2, "the residue survived");
    }

    #[test]
    fn a_reopen_starts_with_no_banked_wheel_travel() {
        let (mut o, handle) = open_over(40);
        o.on_wheel(WheelDelta::Pixels(-20.0), 10.0);
        assert_eq!(handle.scroll(), 0, "two thirds of a notch banks");
        o.set_shown(false);
        o.set_shown(true);
        o.on_wheel(WheelDelta::Pixels(-20.0), 10.0);
        assert_eq!(
            handle.scroll(),
            0,
            "the last open's travel is not carried over"
        );
    }

    #[test]
    fn a_closed_overlay_passes_the_wheel_and_the_press() {
        let handle = OverviewHandle::new();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        handle.publish(vec![(OverviewHit::Open(1, 0), [0.0, 0.0, 10.0, 10.0])], 9);
        assert_eq!(
            o.on_wheel(WheelDelta::Lines(-1.0), 10.0),
            Handled::Passthrough
        );
        assert_eq!(handle.scroll(), 0);
        let (h, reqs) = ctx_run(|ctx| o.on_press([1.0, 1.0], ctx));
        assert_eq!(h, Handled::Passthrough);
        assert!(reqs.is_empty());
    }

    #[test]
    fn hover_reaches_the_handle_and_closing_clears_it() {
        let (mut o, handle) = open_over(0);
        o.on_hover(Some([3.0, 4.0]));
        assert_eq!(handle.hover(), Some([3.0, 4.0]));
        o.set_shown(false);
        assert_eq!(handle.hover(), None);
    }

    #[test]
    fn opening_shows_resolving_at_the_top_and_closing_stops_the_thread() {
        let (mut o, handle) = open_over(60);
        handle.set(OverviewModel {
            rows: vec![Default::default()],
            ..OverviewModel::default()
        });
        handle.set_scroll(7);
        o.set_shown(false);
        assert!(!handle.visible());
        assert!(
            o.core.borrow().telemetry.is_none(),
            "close stops the thread"
        );
        assert!(!o.telemetry_running());
        o.set_shown(true);
        assert!(handle.visible());
        assert!(o.core.borrow().telemetry.is_some());
        let m = handle.model();
        assert!(
            m.resolving && m.rows.is_empty(),
            "never the previous open's rows"
        );
        assert_eq!(m.view, View::Overview);
        assert_eq!(handle.scroll(), 0);
    }

    /// The next command the detached worker inbox holds, as `(wanted, epoch)`.
    fn wanted(rx: &std::sync::mpsc::Receiver<crate::tabmeta::Cmd>) -> (bool, u64) {
        match rx.try_recv() {
            Ok(crate::tabmeta::Cmd::SetGraphWanted { wanted, epoch }) => (wanted, epoch),
            Ok(_) => panic!("not a SetGraphWanted"),
            Err(e) => panic!("no command queued: {e}"),
        }
    }

    #[test]
    fn d10_a_reopen_reads_resolving_until_the_worker_answers_this_open() {
        // The race: close then reopen before the worker has run either queued command. The
        // shared cache still holds the previous open's `graph_ready` (and its repositories),
        // and the reopen's snapshot starts at generation 0, so its first `sync` copies them.
        let (h, rx) = TabMetaHandle::detached();
        let handle = OverviewHandle::new();
        let mut o = FlowOverlay::new(Arc::new(|| {}), Some(h.clone()), handle.clone());
        let resolving = |o: &FlowOverlay| {
            o.core.borrow_mut().refresh();
            handle.model().resolving
        };

        o.set_shown(true);
        assert_eq!(wanted(&rx), (true, 1));
        assert!(resolving(&o), "nothing published yet");
        h.publish_for_test(Vec::new(), true, 1);
        assert!(!resolving(&o), "the first open's own readiness is taken");

        o.set_shown(false);
        o.set_shown(true);
        assert_eq!(wanted(&rx), (false, 0));
        assert_eq!(wanted(&rx), (true, 2));
        assert!(
            resolving(&o),
            "a reopen took the previous open's readiness before the worker reached it"
        );
        // The worker reaches the close: still not this open's.
        h.publish_for_test(Vec::new(), false, 1);
        assert!(resolving(&o));
        // ... then the open, and only now does the overview resolve.
        h.publish_for_test(Vec::new(), true, 2);
        assert!(!resolving(&o));
    }

    #[test]
    fn set_shown_opens_and_closes_only_on_an_edge() {
        let handle = OverviewHandle::new();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        // Hidden and staying hidden: opens nothing.
        o.set_shown(false);
        assert!(!o.visible());
        assert!(!handle.visible());
        assert!(o.core.borrow().telemetry.is_none());
        o.set_shown(true);
        let slot = o.core.borrow().slot.clone();
        // Shown and staying shown: no close, and no second spawn on a fresh slot.
        o.set_shown(true);
        assert!(o.visible());
        assert!(handle.visible());
        assert!(o.telemetry_running());
        assert!(Arc::ptr_eq(&slot, &o.core.borrow().slot));
    }

    #[test]
    fn d18_refresh_rebuilds_only_while_open_and_only_when_the_key_moves() {
        let handle = OverviewHandle::new();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        o.set_now_ms(1_793_404_800_000);
        let log = PathBuf::from("/tmp/flow-unit/refresh.jsonl");
        publish_one_session(&mut o, &log);
        // Closed: no work at all, whatever has been published.
        assert!(!o.core.borrow_mut().refresh());
        assert!(o.core.borrow().key.is_none());
        assert_eq!(o.rebuilds(), 0);
        o.set_shown(true);
        publish_one_session(&mut o, &log);
        assert!(o.core.borrow_mut().refresh(), "the first open rebuild");
        assert_eq!(o.rebuilds(), 1);
        let m = handle.model();
        assert_eq!(
            m.rows.iter().map(|r| (r.tab, r.pane)).collect::<Vec<_>>(),
            [(1, 0)]
        );
        assert!(m.rows[0].selected, "strip index 0 is the active tab");
        // Old D10: the thread is told the logs the overview reads.
        assert_eq!(o.core.borrow().logs, vec![log]);
        // Same key: no rebuild, even with a stale key poked into the expanded set.
        let stale = ExpandSet::from([(TabId(9), 0)]);
        o.core.borrow_mut().expanded = stale.clone();
        assert!(!o.core.borrow_mut().refresh());
        assert_eq!(o.rebuilds(), 1);
        assert_eq!(
            o.core.borrow().expanded,
            stale,
            "an unchanged key does not prune"
        );
        // The key moves (a UI revision) but the model does not: rebuilt — the prune ran —
        // and reported unchanged.
        o.core.borrow_mut().touch();
        assert!(!o.core.borrow_mut().refresh());
        assert_eq!(o.rebuilds(), 2);
        assert!(o.core.borrow().expanded.is_empty());
        // The active tab moves: the key moves and the selection with it.
        TabModelHandle::shared().set(renderer::sidebar_row::TabBarModel {
            titles: vec![row(1)],
            active: 3,
            ..Default::default()
        });
        assert!(o.core.borrow_mut().refresh());
        assert!(!handle.model().rows[0].selected);
    }

    #[test]
    fn the_filter_restricts_rows_and_a_detail_survives_a_filter_that_hides_its_row() {
        let handle = OverviewHandle::new();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        o.set_shown(true);
        publish_one_session(&mut o, std::path::Path::new("/tmp/flow-unit/filter.jsonl"));
        o.poll();
        // The Busy session is Working; an Idle filter hides its row but not its counts.
        o.core.borrow_mut().filter = Filter::Idle;
        o.core.borrow_mut().touch();
        o.poll();
        let m = handle.model();
        assert!(m.rows.is_empty());
        assert_eq!(
            (m.filter, m.counts.all, m.counts.working),
            (Filter::Idle, 1, 1)
        );
        // A detail opened on it still builds: the lookup is unfiltered.
        o.core.borrow_mut().view = ViewState::Detail {
            tab: 1,
            pane: 0,
            selected: None,
        };
        o.core.borrow_mut().touch();
        o.poll();
        assert!(matches!(handle.model().view, View::Detail(d) if d.tab == 1));
    }

    #[test]
    fn esc_in_the_detail_returns_and_esc_in_the_overview_closes() {
        let (mut o, handle) = open_over(10);
        o.core.borrow_mut().view = ViewState::Detail {
            tab: 1,
            pane: 0,
            selected: None,
        };
        o.core.borrow_mut().overview_scroll = 4;
        let (h, reqs) = ctx_run(|ctx| o.on_key(&esc(), ctx));
        assert_eq!(
            (h, reqs.is_empty(), o.visible()),
            (Handled::Consumed, true, true)
        );
        assert_eq!(o.core.borrow().view, ViewState::Overview);
        assert_eq!(handle.scroll(), 4, "the overview's scroll is restored");
        let (h, _) = ctx_run(|ctx| o.on_key(&esc(), ctx));
        assert_eq!((h, o.visible()), (Handled::Consumed, false));
    }

    #[test]
    fn a_session_press_needs_the_session_to_exist_now() {
        let (mut o, handle) = open_over(0);
        publish_one_session(&mut o, std::path::Path::new("/tmp/flow-unit/exists.jsonl"));
        handle.publish(
            vec![
                (OverviewHit::Expand(1, 0), [0.0, 0.0, 10.0, 10.0]),
                (OverviewHit::Expand(2, 0), [20.0, 0.0, 10.0, 10.0]),
                (OverviewHit::Lane(3), [40.0, 0.0, 10.0, 10.0]),
            ],
            0,
        );
        ctx_run(|ctx| o.on_press([5.0, 5.0], ctx));
        assert!(o.core.borrow().expanded.contains(&(TabId(1), 0)));
        let rev = o.core.borrow().ui_rev;
        ctx_run(|ctx| o.on_press([25.0, 5.0], ctx));
        assert!(!o.core.borrow().expanded.contains(&(TabId(2), 0)));
        assert_eq!(o.core.borrow().ui_rev, rev, "a stale press changes nothing");
        // A lane press outside a detail changes nothing either.
        ctx_run(|ctx| o.on_press([45.0, 5.0], ctx));
        assert_eq!(o.core.borrow().ui_rev, rev);
        assert_eq!(o.core.borrow().view, ViewState::Overview);
    }

    #[test]
    fn the_layer_draws_at_the_overlay_z_and_rebuilds_on_its_own_frame() {
        let handle = OverviewHandle::new();
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, handle.clone());
        assert_eq!(
            o.layer().z_order(),
            renderer::overview_panel::OVERVIEW_PANEL_Z
        );
        let Some(mut r) = renderer::testing::headless(
            64,
            64,
            &common::config::Config::default(),
            "the overview layer's per-frame refresh",
        ) else {
            return;
        };
        o.set_shown(true);
        publish_one_session(&mut o, std::path::Path::new("/tmp/flow-unit/layer.jsonl"));
        r.add_layer(o.layer());
        let t = term_core::Terminal::new(4, 1, term_core::Color(0xFFFFFF), term_core::Color(0));
        r.capture(&t.snapshot()).expect("capture");
        assert_eq!(handle.model().rows.len(), 1);
    }

    #[test]
    fn d5_the_key_policy_and_the_overlays_place_in_the_stack() {
        let mut o = FlowOverlay::new(Arc::new(|| {}), None, OverviewHandle::new());
        assert_eq!(o.priority(), 300);
        assert!(o.covers_window());
        let a = chord("a", M::empty());
        // Closed: only the chord is the overlay's.
        for k in [&a, &esc()] {
            let (h, reqs) = ctx_run(|ctx| o.on_key(k, ctx));
            assert_eq!(h, Handled::Passthrough);
            assert!(reqs.is_empty());
            assert!(!o.visible());
        }
        let (h, _) = ctx_run(|ctx| o.on_key(&chord("F", M::SUPER | M::SHIFT), ctx));
        assert_eq!(h, Handled::Consumed);
        assert!(o.visible());
        // Open: everything is swallowed, and Esc closes the overview.
        let (h, reqs) = ctx_run(|ctx| o.on_key(&a, ctx));
        assert_eq!(
            (h, reqs.is_empty(), o.visible()),
            (Handled::Consumed, true, true)
        );
        let (h, reqs) = ctx_run(|ctx| o.on_key(&esc(), ctx));
        assert_eq!(
            (h, reqs.is_empty(), o.visible()),
            (Handled::Consumed, true, false)
        );
    }

    #[test]
    fn the_local_offset_is_a_plausible_zone() {
        let off = local_utc_offset_s(1_793_404_800_000);
        assert!((-14 * 3600..=14 * 3600).contains(&off), "{off}");
    }
}
