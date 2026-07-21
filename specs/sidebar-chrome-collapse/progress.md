## Run State
- feature: sidebar-chrome-collapse (feature 4/5 — header + scroll model + collapse; DARK until feature 5 registers the layer, D14)
- phase: DONE (pending Gate-3 + commit)
- iteration: 1
- gates: T1–T4 PASS. build + workspace tests (sidebar_layout 14 + sidebar_header 7, all headless per R0.4) + clippy -D warnings + fmt all green. deps: sidebar-foundations b3a398c ✓, sidebar-row-redesign 0200d34 ✓
- next: Gate-3 (maker≠checker) → on PASS commit feat(sidebar-chrome-collapse) on build/sidebar-redesign, update ledger. Then feature 5 (sidebar-interaction) registers this layer + the row layer, wires click-switch/hover/collapse/wheel — the LAST feature, where header/scroll/collapse become live.
- anti_spin: last_progress_iter=1, stuck=0
- checkpoint: T1–T4 in-tree (uncommitted). sidebar_layout.rs (pure: tokens block D15, Rect/ChromeRegion/SidebarRegion, SidebarLayoutInput all-pub, header_h/header_rect/button_rect/filter_icon_rect/search_slot_rect/list_viewport/row_stride/content_height/max_scroll/clamp_scroll/scroll_to_reveal/row_rect/row_at/visible_rows/restore_button_rect/chrome_hit/hit_test). sidebar_header.rs (SidebarChromeLayer/Handle/Model, device-free build_shapes, SIDEBAR_CHROME_Z=30 compile-time invariant, record writes list_top). lib.rs +2 pub mod lines. D13 verified: SidebarModel already carries list_top/scroll_px (added in feature 3), chrome layer writes list_top via with_mut — no unilateral sidebar_row.rs edit needed.
- known-followups: R6.9 (scale once) honored — header_h/restore take scale directly, input fns use pre-scaled inp.metrics. Registration is feature 5's (terminal_app.rs add_layer). ICON_PX=16 is a judgement (Known Gap — human confirm with mock).
- contract_ref: sidebar-chrome-collapse | cargo build + cargo test --workspace + cargo clippy --workspace --all-targets -D warnings + cargo fmt --check
