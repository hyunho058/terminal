# coherence-report.md — tmux-pane-resolution

> Emitted by `coherence-audit` (T2). This is a **flag-only** artifact: the checker
> WRITES this report and nothing else — it never edits or rewrites a `spec.md`
> ("never push through the fence", R7.3). The human (or the calling skill —
> build-order / decompose) resolves every finding; the checker only surfaces them.
> The per-pair block below is the R3.3 verdict schema: `{verdict, cited evidence, block|warn}`.

- generated_at: 2026-07-29T09:41:00Z  (re-audit #2 — build-order Phase 1 step 5)
- verdict: OK
- specs_audited: 3   (declared: 3 | undeclared: 0)
- input_ref: tabmeta-tmux-reroot@specs/tabmeta-tmux-reroot/spec.md + tabsession-tmux-reroot@specs/tabsession-tmux-reroot/spec.md + tmux-root-resolution@specs/tmux-root-resolution/spec.md

## Summary

| tier | classification | findings | severity |
|------|----------------|----------|----------|
| contradiction | incompatible (same role, conflicting choice) | 0 | **BLOCK** |
| overlap | unordered (no `depends_on` path between them) | 0 | **BLOCK** |
| redundancy | near-duplicate Requirements/goal | 0 | warn |
| undeclared | no `## Declared Surface` / empty `declared-surface:` | 0 | warn |

- pairs sharing a surface (non-empty glob intersection): 0
- **block-tier findings: 0**  → verdict is `BLOCK` iff this is > 0 (R4.1)

## Findings   (R3.3 — one block per conflicting pair / flagged spec)

(none)

## Considered, not flagged   (informative — audit completeness; these do NOT affect the verdict)

- **ordered overlaps** (a `depends_on` path makes the shared write sequential — D2/R2.3, NOT flagged):
  - (none — every surface intersection is empty; see below)

- **glob set-intersection, all three pairs** (deterministic, maker-run; every pair disjoint):
  - `tmux-root-resolution` ⨯ `tabmeta-tmux-reroot` — ORDERED (F2 → F1) — intersection: none
  - `tmux-root-resolution` ⨯ `tabsession-tmux-reroot` — ORDERED (F3 → F1) — intersection: none
  - `tabmeta-tmux-reroot` ⨯ `tabsession-tmux-reroot` — **unordered** — intersection: none
    (an unordered pair with an empty intersection is not a hazard; the gate only fires on a
    non-empty intersection, R2.3)

- **complementary shares** (different roles on one contract — D7/R3.2, NOT a conflict). The
  deterministic tier found **zero shared file surfaces**, so the contradiction tier's usual
  trigger was empty. Because the three features are nonetheless **contract-coupled** through
  `depends_on` edges, the judge subagent was run over all three dependency-coupled pairs rather
  than returning a clean `OK` over an unjudged coupling (rule 3: never silently pass the
  contradiction tier). All three came back **complementary**:
  - `tmux-root-resolution` (defines the resolver + `Memo` type) ⨯ `tabmeta-tmux-reroot`
    (instantiates and consumes it) under the `effective_root(&mut Memo, pid, tick) -> u32`
    contract — F1 `D10: Two entry points` / `D7: the memo … Memo::new()` vs F2 `D3: The memo is a
    Deriver field`. Different roles: definition vs instantiation.
  - `tmux-root-resolution` (accepts an un-timed-out spawn for the *worker* thread and explicitly
    delegates the overlay placement) ⨯ `tabsession-tmux-reroot` (discharges that delegation with
    a bounded wait inside its own file) under the `effective_root_uncached` contract — F1 `D8:
    synchronous and un-timed-out; blast radius is the metadata worker thread` vs F3 `D8: The
    UI-thread hang exposure is owned here, bounded, and surfaced`. Disjoint threads, disjoint
    blast radii; a consistent division of one risk, not two answers to one question.
  - `tabmeta-tmux-reroot` ⨯ `tabsession-tmux-reroot` — both re-root the same tab from F1's
    resolver but write to **different published surfaces** (F2 → `TabMeta.branch/agent/task` on
    the card; F3 → a `SessionHint` gating `Resolved.owned` in the overlay) — F2 `D9: pane death
    falls back to the tab's own root` vs F3 `D7: admit a pane result only on a positive cwd
    match`. Both degrade in the **same direction** (the tab's own shell pid), so neither reverses
    the other. Their independently-resolved roots *can* diverge across a tmux window switch, but
    the result is a conservative mismatch (card shows the pane; overlay falls back to an unowned
    ranked guess), not a contradictory one — and neither spec asserts they must agree.

- **near-duplicate-looking requirements that are NOT redundancy**: F2 `R5.1` ("pre-existing cost
  assertions hold unchanged") and F3 `R2.4` ("every pre-existing test passes unmodified") share
  wording but target **disjoint test suites** on disjoint surfaces. This is one convention applied
  twice, not duplicated work.

## Re-audit note (build-order Phase 1 step 5)

This report was regenerated because two spec edits landed after the first audit. Both were
re-judged and found **consistent**:
- `tabmeta-tmux-reroot` D8 gained a ledger row for "pane already dead at resolution"
  (`PROC_CWD` 1 → 2, **no** re-root). Traced against `tmux-root-resolution` D13 + D4/R6.3 and
  `proc.rs:26-28` (unmemoized, bumps unconditionally): the liveness read happens, `.is_some()`
  is false, `effective_root` returns the input pid, so the D9 fallback arm is not taken and the
  worker reads cwd once. Both claims correct.
- `tmux-root-resolution`'s "No timeout on `list-clients`" gap was rescoped to the worker path and
  now cross-references `tabsession-tmux-reroot` D8. No contradiction with its own D8, whose
  rejection is of a watchdog *inside the resolver* and whose parenthetical already deferred the
  overlay placement.

Deterministic tiers re-run and unchanged: 0 shared surfaces, 0 undeclared, 0 block-tier overlaps.

## Non-blocking observations from the contradiction judge

Surfaced for the human; **none rises to a contradiction** and none affects the verdict. The
checker does not apply them.

1. **F2's D8 syscall ledger has a missing row, not a wrong one.** It omits the case where F1's
   D13 liveness check *itself* fails (pane already dead at resolution time): `effective_root`
   returns `tp.pid`, the worker reads cwd once, and `PROC_CWD == 2` for the tick with **no**
   re-root. The value 2 already appears in the table but is attributed only to the alive path.
2. **F1's Known Gap "No timeout on `list-clients` … freezes tab-card *updates* (not the UI)" is
   now only half-true.** F3's D8 establishes that a UI-thread call path exists and bounds it, so
   the residual risk there is a leaked thread rather than a frozen UI. F1's gap text predates F3
   discharging the deferral and should cross-reference F3's D8.
3. **Three manifest corrections are surfaced but unapplied, and the specs agree with each other
   on all three** — F1: SD1's `devname`-unavailable note is factually wrong, and SD6's "two
   counters" should be three (`TTY_STAT`); F3: the accept-seed's "mtime-ranking" should be
   event-time ranking. Each spec defers to the human rather than silently rewriting the manifest.
4. **F2 and F3 share process-global `#[cfg(test)]` probe statics despite disjoint file surfaces.**
   F3's D8 can leak a thread that bumps `TMUX_LIST`/`PROC_TTY`/`TTY_STAT` asynchronously after a
   budget expiry. No assertion is currently exposed (F2's counter tests hold `probe::serial()`;
   F3's tests inject closures via `resolved_root_with` and never call the real resolver), but the
   coupling exists and is worth knowing before either suite grows.
5. **F1's D8 title is broader than its own rescoped Known Gap** — "the blast radius is the
   metadata worker thread" is unqualified in the title while the gap now says that framing covers
   only the worker path. Self-consistent via D8's parenthetical; narrowing the title would remove
   the apparent opposition.
6. **Two senses of "budget" now collide lexically** — F1 D10's "needs no budget" means a
   *spawn-cadence* budget; F3's `RESOLVE_BUDGET` is a *latency* bound. Different quantities, but a
   reader could misread D10 as licensing no timeout.
7. **F2's new ledger row is unasserted and numerically redundant.** "as above but `PROC_CWD` 1 → 2"
   matches the pane-alive row; the only real delta is "no re-root", and no requirement covers that
   state (R2.2 covers only the mid-derivation death at `PROC_CWD == 3`). Documentation without a
   check — a coverage gap, not an inconsistency.
8. **The ledger omits one reachable tmux state** — a tmux tab whose client row does not match (a
   no-match tty, or D14's non-default socket) pays `PROC_TTY +1`, `TMUX_LIST +1`, `TTY_STAT +N`
   with `PROC_CWD` unchanged at 1, since `select_pane` returned `None` and no D13 read occurs.

## Verdict

**OK** — no findings ("no findings, OK"). Every declared surface is disjoint; the one unordered
pair has an empty intersection; all three dependency-coupled pairs were judged **complementary**
by an independent subagent that read only the spec files' `## Declared Surface` / `## Decisions` /
`## Constraints` / `## Requirements` and was denied the partition manifest.

> If **BLOCK** and the caller is an unattended / autopilot path: this **HARD-STOPS** the run.
> A WARN there would be an unread annotation (approval was pre-batched), so a known write-collision
> or contradiction must not proceed unseen (R4.3). A human resolves it; the checker never auto-resolves.
