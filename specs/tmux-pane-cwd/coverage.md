# coverage.md — requirement → evidence trace

> Grounds for `loop.md` Gate 2's "sub-requirements with automated evidence" metric.
> Measured at iteration 4: **27 of 37 automated**, 3 excluded by name as human-only.
> `AUTOMATED-gate1` = proven by a hardened Gate-1 command rather than a test.

| Sub-req | Evidence class | Grounds |
|---------|----------------|---------|
| R0.1 | **HUMAN-VERIFIED 2026-07-30** | Confirmed on a live session by the author: app launched from `$HOME` (tab shell cwd `/Users/hyunhokim`, whose `~/.claude/projects/-Users-hyunhokim` does not exist), attached to a tmux pane in `~/IdeaProjects/terminal` (26 logs, live `claude`); Cmd+Shift+M rendered populated AI status rows. cwd half also covered by `overlay::a_tmux_tab_opens_on_the_pane_not_the_client` |
| R0.2 | code-traced | tabs.rs:196 titles() reads s.cwd(meta.get(s.id)); no test over titles() |
| R0.3 | AUTOMATED | tmux::a_tab_with_no_tmux_is_returned_unchanged_and_spawns_nothing + worker::a_non_tmux_tab_costs_exactly_the_gate_and_spawns_nothing |
| R1.1 | AUTOMATED | mod::default_tabmeta_is_blank + worker::both_toggles_off_still_resolves_cwd_but_does_no_git_or_agent_work |
| R1.2 | code-traced | worker.rs:83-92 cwd_of_pid(effective_root(...)); pane-level asserted only in sibling tmux.rs tests |
| R1.3 | AUTOMATED | tmux::m_tabs_in_one_tick_share_one_fetch_and_n_stats (memoized path) |
| R1.4 | AUTOMATED | worker::both_toggles_off_still_resolves_cwd_but_does_no_git_or_agent_work |
| R1.5 | AUTOMATED | worker::a_cwd_only_change_publishes_and_wakes |
| R2.1 | AUTOMATED | session::a_published_cwd_wins_and_costs_no_fallback |
| R2.2 | AUTOMATED | session::absence_falls_back_to_the_direct_lookup |
| R2.3 | AUTOMATED | session::cwd_is_never_emptier_than_the_direct_lookup |
| R3.1 | AUTOMATED | overlay::a_tmux_tab_opens_on_the_pane_not_the_client |
| R3.2 | AUTOMATED | overlay::repeated_identical_notifications_never_restart_the_watcher — drives the real Repoint transition; verified to FAIL against the injected bug |
| R3.3 | AUTOMATED | overlay::no_tmux_yields_none_so_the_caller_keeps_its_own_cwd + a_pane_that_died_between_resolve_and_read_yields_none |
| R4.1 | AUTOMATED-gate1 | Gate-1 trigger grep over feed_agent_states (validated both directions) |
| R4.2 | AUTOMATED | overlay::a_changed_cwd_repoints_an_open_overlay |
| R4.3 | AUTOMATED | overlay::an_unchanged_cwd_is_a_no_op + a_hidden_overlay_never_repoints |
| R4.4 | EXCLUDED-human | live tmux window switch; resolver half via tmux::a_new_tick_refetches_so_a_window_switch_is_followed |
| R5.1 | code-traced | overlay.rs:137 project_dir(None) path is pre-existing behaviour, untouched |
| R5.2 | AUTOMATED | tmux::a_failed_fetch_is_not_retried_within_the_same_tick + tmux_list_counts_attempts_not_successes |
| R5.3 | code-traced | follows from R5.2 + publish_if_changed; no direct test |
| R5.4 | AUTOMATED | overlay::a_tab_with_no_shell_pid_resolves_nothing + tmux::a_dead_pid_is_returned_unchanged_without_panicking |
| R6.1 | AUTOMATED | session::session_cwd_performs_zero_syscalls_when_published_and_one_when_not — a MEASURED delta on a real Session (100 reads = 0 syscalls; the unpublished path bumps the counter, proving it is live) |
| R6.2 | AUTOMATED | worker::a_non_tmux_tab_costs_exactly_the_gate_and_spawns_nothing |
| R6.3 | AUTOMATED | tmux::m_tabs_in_one_tick_share_one_fetch_and_n_stats |
| R6.4 | AUTOMATED-gate1 | Gate-1 hot-path grep (validated both directions) |
| R7.1 | EXCLUDED-human | split-pane focus needs a live window |
| R7.2 | AUTOMATED | tmux::a_tmux_tab_resolves_to_its_own_clients_pane + select_pane_matches_on_device_number |
| R8.1 | code-traced | worker.rs:28 TICK=500ms + should_refresh; cadence not asserted for cwd specifically |
| R8.2 | AUTOMATED | session::cwd_is_never_emptier_than_the_direct_lookup + proc::cwd_of_dead_pid_is_none |
| R8.3 | code-traced | worker.rs:164-168 prev carry-forward; 2s bound not asserted |
| R8.4 | code-traced | single derivation site at worker.rs:83-92; structural |
| R9.1 | AUTOMATED-gate1 | Gate-1 dependency-landed command |
| R9.2 | AUTOMATED-gate1 | Gate-1 declared-surface diff command |
| R10.1 | AUTOMATED | compile-enforced (exhaustive literals) + mod::default_tabmeta_is_blank |
| R10.2 | AUTOMATED | worker::both_toggles_off_still_... + a_cwd_only_change_publishes_and_wakes (both rewritten deliberately) |
| R10.3 | AUTOMATED-gate1 | Gate-1 suite-with-tmux-off-PATH (855 pass) |

## Excluded by name (must never be reported as passed by the loop)
- ~~**R0.1 (render half)**~~ — **CLOSED 2026-07-30** by live observation; see the table.
- **R4.4** — end-to-end tmux window switch needs a human in a live session.
- **R7.1** — split-pane focus follow needs a live split window.

## Code-traced only (no assertion)
R0.2, R1.2 (worker level), R5.1, R5.3, R8.1, R8.3, R8.4
