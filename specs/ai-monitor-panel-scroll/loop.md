# Loop Contract — ai-monitor-panel-scroll

**Goal:** verify the implemented feature against `spec.md`'s 13 requirements /
67 GWT sub-requirements and 12 constraints. Implementation is complete (T1–T11);
this contract exists to prove coverage, not to build.

## Gate 1 — Pass/Fail (100% required)

| # | Command | What it holds |
|---|---------|---------------|
| G1.1 | `cargo build --workspace --all-targets` | builds |
| G1.2 | `cargo test --workspace` | every test green |
| G1.3 | `cargo clippy --workspace --all-targets -- -D warnings` | lint clean |
| G1.4 | `cargo fmt --all -- --check` | formatted |
| G1.5 | `git diff --quiet -- crates/term-core crates/ai-monitor` | **C6/C8/R12.1** — the engine and the telemetry crate are untouched. A hard constraint expressed as a command rather than trusted to review. |
| G1.6 | `! git diff -- '*Cargo.toml' \| grep -qE "^\+.*(egui\|iced\|slint\|gtk\|gpui\|dioxus\|druid\|tao)"` | **C5/R12.2** — no GUI/widget crate crept in. |

## Gate 2 — Quantitative

| Metric | Threshold |
|--------|-----------|
| sub-requirements with **no executable backing** | **≤ 2**, and they must be exactly `R12.1` and `R12.2` — the two the spec itself declares diff-verified facts. Any *other* uncovered sub-req fails the gate. |
| constraints C1–C12 traceable to a check or a named diff fact | **12 / 12** |
| workspace test failures | **0** |

Coverage is measured by `scripts/coverage.py` (written by this loop): for each
`#### RN.M` in `spec.md`, look for a test whose name carries `rN_M` — including the
range (`rA_B_to_rA_C`) and pair (`rA_B_and_rA_C`) forms — or an explicit `RN.M`
citation inside a test body.

## Gate 3 — Qualitative (checker-scored; maker ≠ checker)

- **C4 compliance** — the scroll state stays out of stage one's key and inside stage
  two's. The spec names this the feature's top trap: doing only one half gives,
  respectively, the per-frame re-layout debt back, or a panel that never repaints.
- **Decision ↔ code fidelity** — D17 (rung snap), D22 (three-case `max_index`),
  D31 (slot consumption), D37 (mode derived from the index, not latched) mean in the
  code what they say in the spec.
- **Reversal durability** — the two deliberate reversals (`overlay.rs` key policy,
  `monitor_layout.rs` R1.4 footer) carry an argument at their site, so the next reader
  does not "restore" them.
- **Idiom fit** — comment density, naming and test style match the surrounding code.

## Autonomy

Auto-fix: adding tests for sub-reqs already implemented, comments, local renames,
behaviour-preserving refactors that keep Gate 1 green.

**STOP and escalate:** any change to a decision in `spec.md`, any schema change, any
edit to `crates/term-core` or `crates/ai-monitor`, or a finding that a requirement
cannot be met as specified.

## Lessons

### L1 — 커버리지 게이트가 남의 기능 테스트를 세고 있었다
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `python3 specs/ai-monitor-panel-scroll/scripts/coverage.py`
- note: 이 저장소는 기능마다 `RN.M` 번호를 재사용한다 — 이름만 매칭하면 다른 기능의 테스트가
  잡혀 65/67로 부풀었고 정직한 수는 57/67이었다. 스크립트는 이제 diff로 새로 추가된 `fn`만
  보고, 그것을 `cargo test --workspace -- --list`와 교집합해 헬퍼와 `#[ignore]`를 배제한다.

### L2 — 퇴화 케이스를 막는 가드가 도달성을 깨뜨렸다
- cause: gate-fail
- source: human-approved
- form: gate1-command
- status: active
- check: `cargo test -p renderer --lib monitor_scroll`
- note: `max_index`의 퇴화 가드를 "꼬리가 들어가는가"로 걸었더니 본문 rung이 큰 모양에서
  rung 5–8이 통째로 도달 불가가 됐다. D22의 문장대로 "**어떤** rung이 밴드에 들어가는가"를
  물어야 한다. 고친 뒤 버그를 되살려 정확히 한 테스트만 깨지는지 확인했다 — 그 확인이
  이 check를 실효 있게 만드는 부분이다.

### L3 — 절대 실패할 수 없는 단언이 그 버그를 가리고 있었다
- cause: gate-fail
- source: human-approved
- form: gate3-item
- status: active
- check: 이번 diff의 새 테스트 중, 구성상 항상 참인 것(빈 문서·기본 배경처럼)을 단언해
  통과하는 것이 있는가? 있으면 그 단언을 **두 상태의 비교**로 바꿔라 — 한 상태의 절댓값이
  아니라.
- note: `r3_7`이 `top=0`에서 두 번 합성해 합이 0이 아님을 단언했는데, 그건 `panel_bg`가
  보장하는 값이라 L2의 버그를 통과시켰다. `assert_ne!(compose(…,0), compose(…,max))`로 교체.

