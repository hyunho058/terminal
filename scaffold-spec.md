# Scaffold Spec: mac-terminal (Rust GPU Terminal)

Created: 2026-07-16
Goal: Set up a Rust + pure wgpu/Metal foundation for a macOS-only GPU terminal that an agent can extend into features (image paste, ai-monitor, settings).

## L0: Goal

**Confirmed Goal**: Set up a Rust foundation (pure wgpu/Metal GPU stack, no GUI crates) with a clear render-loop/PTY architecture and a vertical-slice exemplar that an agent can extend to build terminal features (zsh cell-grid rendering, scroll, tabs, image paste, native ai-monitor overlay, settings).

**Non-Goals**:
- Feature implementation (image paste, ai-monitor, settings UI → later via `/specify`)
- Cross-platform support (macOS ARM64 only)
- SSH / remote sessions, non-zsh shells
- Production packaging / notarization pipeline (baseline CI only)

## L1: Environment

- **Directory**: greenfield (only `requirements-interview-outputs/mac-terminal/insights.md` present)
- **Rust**: rustc 1.95.0, cargo 1.95.0 (via rustup) — auto-resolves language/runtime/package-manager
- **Platform**: macOS 26.3.1, Darwin 25.3.0, Apple Silicon (ARM64 / T8103)
- **Tools**: git (initialized, clean, branch `feat/terminal-core`), docker available (not needed for native app)
- **Complexity**: Medium-Complex — signals: real-time render+PTY I/O, external AI CLI integration, background log watching (ai-monitor), background PTY reader threads

## L2: Architecture Decisions

### Decisions

| ID | Decision | Rationale | Assumed? |
|----|----------|-----------|----------|
| D1 | Rust 1.95 + cargo | Installed toolchain; requirement | No |
| D2 | macOS ARM64 only, no cross-platform | Requirement (macOS 전용) | No |
| D3 | Pure wgpu/Metal GPU stack, hand-rolled cell rendering, no GUI/widget crates | Requirement (순정 스택, 셀 수동 제어) | No |
| D4 | winit for window/event-loop/IME + objc2 for macOS-specific (clipboard image, native menu) | wgpu ecosystem fit, IME + low boilerplate; drop to objc2 where needed (Alacritty/Wezterm pattern) | No |
| D5 | `vte` crate for escape-sequence parsing + own grid/scrollback model | Keep "manual cell control" freedom without reinventing VT parsing (scope survival) | No |
| D6 | `swash` for shaping + glyph rasterization + own GPU glyph atlas | ligature support required; GPU cell rendering stays hand-rolled | No |
| D7 | Threads + channels (PTY reader thread → channel → main render thread; separate watcher thread) | No async runtime; proven terminal pattern, simplest for few I/O sources | No |
| D8 | cargo workspace: `term-core` (vte·grid·pty, GPU-free) / `renderer` (wgpu) / `app` (winit binary); `ai-monitor` later | Enforces dependency direction → core is headless-testable | No |
| D9 | TOML + serde config at `~/.config/<app>/config.toml` | Standard terminal convention, human-editable | No |
| D10 | `cargo test` + `insta` snapshots (headless core: vte→grid, scrollback) + wgpu headless render smoke | Test GPU app's logic without a GPU | No |
| D11 | GitHub Actions (macos runner: fmt + clippy + test + build) + local `.app` bundle script, no signing/notarization | Baseline drift resistance; notarize is non-goal | No |
| D12 | zsh + local PTY only | Requirement | No |

### Constraints

- **C1**: cargo only — never other build tools
- **C2**: No GUI/widget crates (egui, iced, gtk, druid, …). Pure wgpu rendering. *Explicit exception*: winit (windowing) and objc2 (platform bindings) are allowed — they are not widget toolkits.
- **C3**: macOS ARM64 target only — no `#[cfg]` branches for other platforms
- **C4**: Dependency direction `term-core ← renderer ← app`. `term-core` MUST stay GPU-free and headless-testable; it must not depend on wgpu or winit.
- **C5**: zsh + local PTY only — no SSH, no remote, no other shells
- **C6**: Render loop is winit-event-loop-driven (redraw requests); PTY/data arrival wakes it via `EventLoopProxy`, not a free-running loop
- **C7**: Performance budgets are acceptance gates (exact numbers TBD — see Known Gaps G1)

### Activated Extensions

- [x] **Runtime Patterns** — long-running app: event loop, graceful shutdown, PTY lifecycle, window resize handling
- [x] **Internal Contract** (adapted Type Contracts) — the `term-core` → `renderer` boundary passes a cell-grid snapshot type; that type is the internal contract, versioned in `term-core`
- [ ] Data Layer — not activated (config file is the only persistence; no DB)
- [ ] Docker/Infra — not activated (native macOS app)

### Known Gaps

- **G1**: Performance target numbers not finalized (120fps / input→screen 8ms / 100k-line scroll are *examples* from the requirements interview) — must be confirmed before they harden into TF acceptance gates
- **G2**: Inline image protocol (iTerm2 vs Kitty vs custom) undecided — deferred to feature phase (`/specify`), not a scaffold blocker
- **G3**: `swash` covers Latin/ligatures; color-emoji + complex-script coverage and glyph-atlas eviction policy are render-phase concerns, deferred

## L3: Harness Setup

### Domain Context
Agent-inferred from `requirements-interview-outputs/mac-terminal/insights.md` → written to `docs/domain.md`. Terms: PTY, cell grid, scrollback, glyph atlas, VT/ANSI escape sequence, ligature, IME, ai-monitor toggle overlay, active-session log watching. Invariants: manual per-cell control, GPU-free `term-core`, macOS/zsh/local-only.

### Team Conventions
Solo project — no commit/PR/branch conventions enforced.

### Rules (from Constraints)
| Constraint | Rule File | Status |
|-----------|-----------|--------|
| C2: No GUI/widget crates (winit/objc2 excepted) | `.claude/rules/no-gui-crates.md` | Approved |
| C3: macOS ARM64 only, no cross-platform `#[cfg]` | `.claude/rules/macos-only.md` | Approved |
| C4: Dependency direction; `term-core` stays GPU-free | `.claude/rules/dependency-direction.md` | Approved |
| C5: zsh + local PTY only | `.claude/rules/zsh-local-only.md` | Approved |

### Skills
| Skill | Description | Source |
|-------|-------------|--------|
| `/run-term` | `cargo run -p app` to build & launch the terminal and eyeball a render change | D3/D4 (GPU app) |
| `/insta-review` | `cargo insta review` to accept/reject snapshot diffs after grid/parser changes | D10 (insta) |

### Hooks
| Hook | Type | Trigger |
|------|------|---------|
| `rustfmt` | PostToolUse | Edit/Write to `*.rs` |
| Block edit | PreToolUse | Edit/Write to `Cargo.lock` |

## L4: Plan

### Requirements

| ID | Requirement | Source | Conditional? |
|----|------------|--------|-------------|
| R1 | Code Structure — cargo workspace (`term-core`/`renderer`/`app`) + vertical-slice exemplar with importable utils (config, errors, logging) | L2 | No |
| R2 | Test Infrastructure — `cargo test` + `insta` snapshots, headless `term-core` test harness | L2 | No |
| R3 | Guard Rails — lean CLAUDE.md + `docs/*` + rustfmt/clippy + CI + `config.example.toml` | L2 | No |
| R4 | Internal Contract — cell-grid snapshot type in `term-core` consumed by `renderer` (the internal boundary) | L2 | Activated |
| R7 | Runtime Patterns — winit event loop, graceful shutdown, PTY lifecycle, window resize handling | L2 | Activated |
| R8 | Project Rules — C2/C3/C4/C5 → `.claude/rules/` | L3 | Approved |
| R9 | Domain Skills — `/run-term`, `/insta-review` | L3 | Approved |
| R10 | Project Hooks — rustfmt (PostToolUse), block `Cargo.lock` (PreToolUse) | L3 | Approved |

### Task DAG

| ID | Task | Fulfills | Depends On | Status |
|----|------|----------|------------|--------|
| T1 | Workspace init — root `Cargo.toml` (workspace, 4 crates), `rustfmt.toml`, `rust-toolchain.toml`, `.gitignore` | R1 | — | ✅ done |
| T2 | Guard Rails — CLAUDE.md, `docs/domain.md`, `docs/architecture.md`, `.claude/rules/*`, `config.example.toml`, CI workflow, `scripts/bundle-app.sh` | R3, R8 | T1 | ✅ done |
| T4 | Test infra — `insta` dev-dep, `tests/` layout, headless `term-core` snapshot harness (7 tests + ignored PTY smoke) | R2 | T1 | ✅ done |
| T3 | **Vertical-slice exemplar** — PTY spawn zsh → read → `vte` parse → grid update → `renderer` draws cells → window; importable `config`/`error`/`logging` (in `common`); `GridSnapshot` contract (R4) | R1, R4 | T2, T4 | ✅ done |
| T8 | Runtime patterns — graceful shutdown, resize, PTY lifecycle, `EventLoopProxy` wake, `ControlFlow::Wait` | R7 | T3 | ✅ done |
| T_SKILL | Skills — `/run-term`, `/insta-review` SKILL.md with real cargo commands | R9 | T3 | ✅ done |
| T_HOOK | Hooks — `.claude/settings.json` + `.claude/hooks/*.sh` (rustfmt PostToolUse, block Cargo.lock PreToolUse) | R10 | T1 | ✅ done |
| TF | Scaffold verification — fmt/clippy(`-D warnings`)/test all green; exemplar launches (Metal adapter, zsh, 105×31 grid); `.app` bundles | — | all | ✅ done |

### Implementation notes (a 4th crate was added)

`common` was split out as a 4th crate (below `term-core`) to give the importable
utilities (config/error/logging) a clean home that both `renderer` and `app` can
share without breaking the dependency direction. Final graph:
`common ← term-core ← renderer ← app`.

### Quality Criteria

- **Agent extensibility**: vertical-slice exemplar T3 (full pty→grid→render flow to copy)
- **Testability**: headless `term-core` tests T4 — GPU app logic tested without a GPU (enabled by C4 dependency direction)
- **Drift resistance**: CLAUDE.md + 4 rules + rustfmt/clippy + CI (T2), rustfmt hook (T_HOOK)
- **Type safety**: cell-grid snapshot contract in `term-core` (R4/T3)
- **Cross-session continuity**: CLAUDE.md + `docs/domain.md` + `docs/architecture.md` (T2)
- **Task automation**: `/run-term`, `/insta-review` (T_SKILL)
- **Code quality enforcement**: rustfmt + Cargo.lock-protect hooks (T_HOOK)

### Open Gaps carried into build
- G1 performance numbers unconfirmed → TF checks *build/run*, not fps/latency budgets, until numbers are locked via `/specify`.
