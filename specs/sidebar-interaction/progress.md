## Run State
- feature: sidebar-interaction (feature 5/5 — the FINAL integration; registers the layers + wires live click/hover/collapse/scroll)
- phase: VERIFY (Gate-3 checker in flight)
- iteration: 2
- gates: ALL FOUR GREEN — cargo build -p app ✓; cargo test --workspace ✓ (no failures); cargo clippy --workspace --all-targets -D warnings ✓; cargo fmt --check ✓. deps: sidebar-row-redesign 0200d34 ✓, sidebar-chrome-collapse b3065fe ✓
- next: await Gate-3 maker≠checker verdict → if PASS commit feat(sidebar-interaction) + update build_order ledger; if NEEDS_FIX apply minimal fixes, re-gate, re-check.
- anti_spin: last_progress_iter=2, stuck=0
- /run-term smoke: PASS — Metal up, surface configured (1800x1080), cols=65 (sidebar inset reserved), SidebarChromeLayer registers, alive >4s, no panic/error in log.
- WORK COMPLETE (T3–T7 all landed, uncommitted):
  - input.rs: SidebarAction/WheelDelta/sidebar_press/sidebar_hover/wheel_scroll wired (dead_code allows all removed); tab_target + its test DELETED; `renderer::tab_bar` import removed (was tab_target-only).
  - settings.rs: refit() is the single shared helper; apply() drops set_left_inset (D4/D6).
  - terminal_app.rs:
    - Running gained: chrome_handle: SidebarChromeHandle, sidebar_cfg: SidebarConfig, scroll: f32, last_hit: Option<usize>.
    - start(): dropped dead set_left_inset; registers SidebarChromeLayer via SidebarChromeLayer::new() → add_layer (D14).
    - helpers: sidebar_input() (D3 — no app geometry; row_h from sidebar_row::layout().card_h), collapsed() (D5 — left_inset()<=0), publish_scroll() (D10 — set_list_scroll(header_h, scroll)), toggle_sidebar() (D6 — set_sidebar_collapsed→refit→reclamp→publish→refresh_hover→redraw), refresh_hover() (D8 — raw cursor to chrome_handle + region-debounced set_hovered), clear_hover().
    - arms: MouseInput→sidebar_press+action match (Switch/NewTab through do_tab_action with exit propagation; Collapse/Expand→toggle_sidebar; Inert swallowed). CursorMoved→last_cursor+refresh_hover. CursorLeft + Focused(false)→clear_hover. MouseWheel→WheelDelta both units→wheel_scroll, publish+refresh_hover+request_redraw on change. Resized→settings::refit + reclamp + publish + refresh_hover + redraw. ConfigReloaded→refresh sidebar_cfg + reclamp + publish + refresh_hover.
    - do_tab_action: scroll_to_reveal(active_index) + publish_scroll + refresh_hover after sync_tab_model (D9/D10).
- D13 RECONCILIATIONS applied as shipped (siblings differed from spec assumptions):
  1. hit_test is `hit_test(p:[f32;2], scroll:f32, &SidebarLayoutInput, collapsed:bool)` — app builds feature-4 SidebarLayoutInput, passes scroll+collapsed explicitly. Single ordered entry.
  2. Chrome hover uses RAW cursor on SidebarChromeHandle (layer resolves via chrome_hit); card hover via TabModelHandle::set_hovered. No hovered_chrome field.
  3. Region enums shipped as SidebarRegion{Chrome(ChromeRegion),Card,Panel} + ChromeRegion{NewTab,HideSidebar,Restore} — NO Filter variant. Match adapted to shipped reality (D13).
  4. Roster ALREADY pushed by feature 3's tabs.rs::titles()/set_roster on every sync_tab_model — T7 satisfied, not duplicated.
  5. titles NOT renamed to rows (feature 3 kept titles deliberately) — rename skipped.
- KNOWN GAP (human-verify, NOT a blocker): wheel scroll sign (new = scroll - delta_y) + Lines→row_stride needs /run-term hardware confirmation — one char to flip if inverted (D10).
- contract_ref: sidebar-interaction | cargo build + cargo test --workspace + cargo clippy --workspace --all-targets -D warnings + cargo fmt --check
