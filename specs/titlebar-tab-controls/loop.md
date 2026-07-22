# loop.md — titlebar-tab-controls

## Goal
Feature ② of sidebar-titlebar. **Increment 2 (native top band):** the tab controls
(`+` new-tab, `×` close, sidebar collapse toggle) live in the TRUE topmost band of the
window — the native macOS title bar made transparent with a full-size content view, so
content reaches the true top — NOT an in-content strip stacked below the OS title bar.
The macOS traffic lights stay; a left gutter keeps the controls clear of them; an
empty-band press initiates a native window drag. Overrides the original spec D12
(in-content strip / native-titlebar non-goal) at explicit user request; spec amended in
the same change (D12 → resolved, +D15/R8/T5).

Layout (user-approved): toggle just right of the traffic lights; `+`/`×` right-aligned.

> Increment 1 (shipped, commit 406968c): the title bar + its geometry, top_inset shift of
> every top-relative y, and removal of the relocated controls from the sidebar (T1–T4).

## Gate 1 — Pass/Fail (binary, 100%)
- `cargo build` succeeds
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo test --workspace` green

## Gate 2 — Quantitative (numbers vs thresholds)
- ≥ **2** new/updated titlebar geometry tests, all passing: the `ToggleSidebar` button's
  left edge ≥ the reserved traffic-light gutter at scale 1.0 AND 2.0; and no title-bar
  button rect intersects the gutter band `[0, gutter)`.
- **Zero** edits under `crates/term-core/**` (SD2).

## Gate 3 — Qualitative (judged, with grounds + action)
- **native-top-band** — the window is created with `with_titlebar_transparent(true)` +
  `with_fullsize_content_view(true)` + `with_title_hidden(true)` (winit
  `WindowAttributesExtMacOS`), so the content view (and the wgpu surface / winit
  inner_size) spans the full window height and `set_top_inset` reserves the band at the
  TRUE window top — one band, no strip stacked under a visible OS title bar.
- **traffic-light-clearance** — every title-bar button rect lies clear of the reserved
  left gutter; the native traffic lights stay visible and clickable and no drawn control
  is occluded by them.
- **drag-preserved + band-ownership (D13)** — every `MouseInput` Pressed with
  `cursor.y < top_inset` is consumed before sidebar/resize/selection: a button → its
  action; an empty band → `window.drag_window()` (native move), never a fall-through to
  `begin_selection`/resize/grid.
- **no-regression / fit (D8/SD1/SD2/SD3)** — top_inset grid shift, the toggle's
  collapse/glyph behaviour, and the sidebar are unchanged; no GUI crates; `term-core`
  untouched; title-bar hit geometry stays one pure module shared by draw + hit-test.

## Autonomy note
This increment intentionally OVERRIDES spec D12 (an original non-goal) at the user's
explicit request; the spec is amended in the same change so the contract and spec agree
(no lingering conflict — the one autonomy-fence "conflicts with spec" item, resolved by
the human, not pushed through). No STOP-list items (no schema/auth/payment/migration).
The traffic-light vertical alignment and the single-band look are a live-run visual check
(`/run-term`), not unit-testable; gates cover the pure geometry (gutter clearance), the
window-attribute wiring, the press routing, build/clippy/tests.

## Lessons

### L1 — transparent titlebar needs alpha_mode = Opaque
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: The on-screen surface forces `CompositeAlphaMode::Opaque` (not `Auto`) so the premultiplied glyph output composites correctly once the native titlebar is transparent.
- note: A transparent titlebar makes the CAMetalLayer non-opaque, so `get_default_config`'s `alpha_mode: Auto` resolves to `PostMultiplied`; the compositor then treats premultiplied glyph output as straight alpha and darkens ALL terminal text to invisible. Forcing `Opaque` fixes it (fix commit 085ac9e).
