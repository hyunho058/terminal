# Spec: ai-monitor-context-window

## Meta
- **Created**: 2026-09-11
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-09-11
- **L2-reviewer**: PASS (NEEDS_FIX x5 후 전부 반영; 소스 전수 재검증 완료)
- **Execution**: 미착수 — 사용자가 L4 게이트에서 "승인하고 여기서 멈춤"을 선택했다.

## Goal
ai-monitor 패널의 CONTEXT 타일이 컨텍스트 윈도우를 모델 id로 **추측**하지 않고, 세션
로그에 남은 `/context` 명령 출력에서 **실제 값을 판독**해 분모로 쓰도록 한다.

실측된 증상 (**2026-09-11, 세션 `600e6f90`의 used == 74,677 시점**): `/context`는 11%
(74.7k/700k), 같은 순간 패널은 37% (74.7k/200k). 분자는 토큰 단위까지 일치하므로
**틀리는 것은 분모뿐이다.**

> ⚠️ **이 사례는 이미 재현되지 않는다.** 같은 세션이 대화 도중 자라 high-water가
> **217,737**이 되었고, 200k를 넘자 `promoted_window`가 1M으로 승격해 패널은 이제
> **22%** (217.7k/1M), `/context`는 **31%** (217.7k/700k)를 표시한다. 방향이 과대에서
> 과소로 뒤집혔다. 살아 있는 로그에서 뽑은 수치는 전부 측정 시점 표기로 읽어야 한다.
> 불변인 것은 "분모만 틀리다"는 구조이지 특정 퍼센트가 아니다.

## Non-goals
- `/context`의 카테고리 분해(System prompt / System tools / Memory files / Skills /
  Messages)를 패널에 그리는 것 — 사용자가 "윈도우만 정확히"를 명시적으로 선택했다.
  표시 형태는 그대로 두고 숫자만 맞춘다.
- autocompact buffer를 별도로 모델링하는 것 — buffer는 판독값 **안에 포함된 조각**이므로
  따로 뺄 것이 없다. (초안은 "700k가 이미 buffer를 뺀 값"이라 적었는데 **틀렸다**:
  `33000/700000 = 4.714%` → 표시 `4.7%`로 일치하므로 buffer는 700k 내부의 슬라이스이고,
  `1,000,000 − 33,000 = 967k ≠ 700k`이므로 "raw 윈도우 − buffer"도 아니다. 결론은 같지만
  근거가 반대였고, 그 오해가 D6의 원래 오류를 낳았다 — L2 심사에서 교정.)
- 분자(`context_used`) 변경 — `input + cache_read + cache_creation`이 `/context`와
  이미 토큰 단위까지 일치함을 실측 확인했다 (74,677 == `/context`의 74.7k).
- `promoted_window` / `MODEL_WINDOWS` / `[ai_monitor] context_window` 중 어느 것도
  **코드에서 제거하지 않는다** — 판독값이 없는 세션(42개 중 37개)에서 그대로 동작해야 한다.
  다만 판독값이 **있는** 세션에서는 D6에 따라 승격·테이블이 참조되지 않으므로, 그 세션에
  한해 관계는 보완이 아니라 **배타**다. (초안은 "대체하지 않고 합성된다"고 적었는데,
  그것은 반전 전 D6의 문장이었다 — L2 심사에서 교정.)
- 패널이 `/context`를 대신 실행하거나 사용자에게 실행을 유도하는 것 — 패널은 수동적
  로그 리더다.

## Confirmed Goal
`ai-monitor`의 JSONL 파서가 `type == "system" && subtype == "local_command"` **이고 `content`에
`Context Usage`를 포함하는**(D1a) 레코드에서 윈도우를 판독해 `TelemetrySnapshot`에 싣고,
`dashboard.rs`의 `context_pct`가 그 값을 분모 우선순위에 삽입한다.

### ⚠️ 범위 변경 — L0 승인 범위보다 **넓어졌다** (재확인 필요)

L0 게이트에서 사용자에게 제시하고 승인받은 것은 *"`promoted_window`가 못 잡는 구간만 메우는,
40개 중 2개(5%)짜리 좁은 기능"*이었다. **그 진술은 반전 전 D6에 의존한 것이었고, 지금은 틀렸다.**

D6이 "판독값은 종착값"으로 바뀌면서 판독값은 승격을 **덮어쓴다**. 5개 로그 전수 시뮬레이션:

| 세션 | used | 오늘 분모 → % | 신 D6 → % | `/context` | 변화 |
|---|---|---|---|---|---|
| `1f5a3aff` | 389,695 | 1,000,000 → 39% | 700,000 → **56%** | 56% | **+17pp** |
| `fc8654a8` | 319,581 | 1,000,000 → 32% | 700,000 → **46%** | 46% | **+14pp** |
| `600e6f90` | 217,737 | 1,000,000 → 22% | 700,000 → **31%** | 31% | **+9pp** |
| `fe6386ae` | 82,932 | 200,000 → 41% | 700,000 → **12%** | 12% | −29pp |
| `77de50ce` | 40,323 | 200,000 → 20% | 700,000 → **6%** | 6% | −14pp |

`/context` 열 읽는 법: **3건은 로그에 실제로 찍힌 값**(`1f5a3aff` 56%, `fc8654a8` 46%,
`77de50ce` 6%), **2건은 계산값**(`600e6f90` 31%, `fe6386ae` 12% — 로그에 찍힌 값은 각각 11%, 9%
이고 그 뒤 usage가 자랐다). 5건 모두 일치하지만 셋만 관측이다.

세 가지가 달라졌다:
1. **3개(7%)가 아니라 5개(12%)** — 판독값이 있는 모든 세션이 바뀐다.
2. **승격이 이미 개입한 세션까지 덮는다** (위 3개), 그리고 하필 변화폭이 가장 크다.
3. **5개 전부 `/context`와 정확히 일치한다** — 완료 기준은 오히려 더 잘 달성된다.

**2026-09-11 L2 게이트에서 사용자가 이 넓어진 범위를 재승인했다.** 함께 검토한 대안 —
판독값을 `high_water <= 200k`인 세션에만 적용해 L0 범위(2개 변경)를 그대로 지키는 게이트 —
는 기각되었다. 기각 이유: 그렇게 하면 승격이 개입한 3개 세션이 **계속 `/context`와 어긋난
값**(39/32/22% vs 56/46/31%)을 보여주는데, 그것이 이 스펙이 고치려는 바로 그 증상이다.

**결정에 필요한 마지막 사실 — 음수 스윙 2건의 방향이 위험한 쪽이다.** `fe6386ae` −29pp,
`77de50ce` −14pp로 퍼센트가 절반 이하로 준다. D9 자신이 *"과소보고는 이 스펙이 가장 피하려는
방향"*이라 적었는데, 넓어진 부분이 마침 그 방향이다. 판독값이 틀릴 경로가 셋 남아 있으므로
(G-c 반올림 · G-h 미관측 200k 표기 · D9의 사용 불가 판정) 사용자는 이것을 알고 결정해야 한다.
다섯 건 모두 `/context`와 일치하므로 판독값이 **맞다면** 과소보고가 아니라 **교정**이다.

**완료 기준 (불변)**: `/context`를 한 번이라도 실행한 세션에서 패널 퍼센트가 `/context`와
일치한다. 실행하지 않은 세션(42개 중 37개)은 오늘 동작을 그대로 유지한다.

## Research

### 결함의 범위 — 분모뿐임을 실측 확인
- `context_used = input_tokens + cache_read + cache_creation` (`telemetry.rs:1079`). `output_tokens`를 제외한다.
- 이 세션 로그의 `msg_011CexvzSb1PkVRRYvp5fHyL`이 **74,677**을 보고했고, 같은 순간 `/context`가 **74.7k**를 표시했다. 분자는 토큰 단위까지 동일하며, `/context` 역시 output을 제외한다. → **분자는 손댈 이유가 없다.**
- 분모 결정은 `dashboard.rs::context_pct` (`dashboard.rs:946-981`): ① `env.context_window`(config) → ② `MODEL_WINDOWS` 추측 → ③ `promoted_window` 승격.
- `MODEL_WINDOWS`는 `claude-opus-` → `200_000` (`dashboard.rs:80-85`). 이 세션의 `message.model`은 `claude-opus-5` — `[1m]` 마커 없음.
- 계산: `74677*200/200000 = 74` → `div_ceil(2)` = **37%**. `/context`는 11%. 재현 완료.

### 윈도우는 `/context` stdout에만 존재한다 (전수 조사)
- `dashboard.rs:70-73`이 이미 문서화: *"model ids carry no bracketed context-window marker in any of the 28,925 logged values ... every `[1m]` occurrence in a real log is prose inside a message, never a field"*.
- 재검증: 이 프로젝트의 **모든** JSONL에서 `"<field>": 200000|700000|1000000` 패턴 **0건**. 윈도우를 담은 숫자 필드는 존재하지 않는다.
- `message.context_management`는 항상 `null`이거나 `{"applied_edits":[]}` — 윈도우 정보 없음.
- **유일한 출처**: `/context` 명령의 stdout.

### 판독 앵커 (구조화되어 있음)
```
type    == "system"
subtype == "local_command"
level   == "info"
content == <stdout, ANSI 포함>   // D1a: 여기에 "Context Usage" 포함까지 요구
```
- `content` 안의 두 위치가 같은 값을 담는다:
  - `"74.7k/700k tokens (11%)"` (헤더)
  - `"Auto-compact window: 700k tokens"` (전용 라인)

### 원문 바이트 — ANSI가 라벨과 숫자 **사이에** 끼어든다
`od -c`로 확인한 실제 바이트:
```
\033[1m Auto-compact window: \033[22m \033[38;2;153;153;153m 700k tokens \033[39m \n
```
- 따라서 `Auto-compact window: (\d+)` 류의 순진한 정규식은 **실패한다** — 라벨과 숫자 사이에 SGR 시퀀스 2개가 있다.
- 값은 `700k` — **반올림된 사람용 표기**이지 raw digit이 아니다. `k`/`M` 접미사와 소수점(`74.7k`) 처리가 필요하다.
- **SGR 인코딩이 한 종류가 아니다** (L2 심사에서 발견). 5개 로그 전수 확인 결과 두 형식이 공존한다:
  - truecolor `\033[38;2;153;153;153m` — 4개 로그
  - 256색 `\033[38;5;246m` — 1개 로그 (`77de50ce`)

  따라서 스트리퍼는 `38;2` 전용이 아니라 **일반 CSI-SGR**을 처리해야 한다. 한 샘플만 보고 구현하면 절반이 깨진다.
- 반면 `Auto-compact window: ` 가 원문에서 **연속 바이트**라는 점은 5개 로그 전부에서 참이다 (이스케이프는 라벨 앞과 콜론 뒤에만 온다).
- 정밀도 교차검증: `33000/700000 = 4.714%` → 표시 `4.7%` ✓, `74677/700000 = 10.67%` → 표시 `11%` ✓. 실제 윈도우는 정확히 700,000으로 일관된다. 다만 표기가 반올림이므로 파싱 결과는 **측정이 아니라 반올림된 읽기**다.

### prose 오염 — 이 스펙이 반드시 피해야 할 함정
- 전체 로그(42개)에서 `"Auto-compact window"` 문자열을 담은 레코드의 `type` 분포 (2026-09-11 실측):

  | type | 건수 | 권위 |
  |---|---|---|
  | `assistant` | 18 | 산문 |
  | `user` | 10 | 산문 |
  | `system` (`subtype=="local_command"`) | 6 | **유일한 권위** |
  | `attachment` | 1 | 산문 |
  | `queue-operation` | 1 | 산문 |

- 즉 산문이 권위 있는 레코드보다 **5배** 많고, 타입도 5종에 걸쳐 있다. (초안은 "`system` 2건 + `assistant` 2건"이라 적었으나 **크게 과소 기술**이었다 — L2 심사에서 교정. 방향은 D1을 오히려 강화한다.)
- 이 수치는 **고정값이 아니다**: 이 기능을 논의하는 대화 자체가 그 문자열을 로그에 계속 추가한다. 스펙에 박힌 숫자는 측정 시점의 스냅샷으로 읽어야 하며, 비율(산문 ≫ 권위)만이 안정적인 사실이다.
- 즉 `[1m]`이 "prose로만 나타나 판독 불가"였던 것과 **정확히 같은 함정**이 이 문자열에도 있다. 라인 전체 grep은 즉시 오독한다.
- **선례 있음**: `telemetry.rs`가 이미 같은 교훈을 문서화하고 있다 (task-notification 분기, D12/R4.3/R4.4) — *"Routed on the TAG, acted on by the parse"*, 그리고 *"a parse living there could be reached by anything that merely mentions a notification"*. 이 스펙은 그 doctrine을 그대로 따른다.

### 훅 지점
- 레코드 디스패치: `telemetry.rs:576` `match v.get("type").and_then(Value::as_str)` — 현재 `tool_use` / `tool_result`만 처리하고 나머지는 `_ => {}`. `system` 분기를 여기 추가한다.
- 스냅샷: `TelemetrySnapshot`에 `context_used`(`telemetry.rs:295`)·`context_high_water`(`:302`) 옆에 새 필드.
- 소비: `dashboard.rs::context_pct`의 `let window = match env.context_window {...}`.

### 인프라 / 관례
- **ANSI 스트리퍼가 워크스페이스에 없다.** `ai-monitor`·`app` 어디에도 없으므로 새로 써야 한다 (외부 크레이트 추가는 이 프로젝트의 보수적 의존성 관례상 정당화 필요).
- `ai-monitor` 의존성은 `serde_json` + `notify`뿐이고 내부 크레이트 의존이 없다. `app`만 그것을 의존한다 → **dependency-direction 위반 없음**.
- 테스트: `crates/ai-monitor/tests/fixtures/*.jsonl`을 `include_str!`로 읽는다 (`tests/telemetry.rs:21`). 주석이 *"never a live log, and no clock is read"*를 명시 — 라이브 로그를 읽는 테스트는 금지.
- 검증: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`. 커버리지는 `cargo mutants -f <file> -j 4` 생존자 수로 판정 (CLAUDE.md).
- `no unwrap()` outside tests.

### 커버리지 실측 (**전체 42개 로그** — 초안의 "최근 40개"는 표본이 잘려 1건을 놓쳤다)
| 조건 | 세션 수 |
|---|---|
| `/context` 레코드 보유 | **5 / 42 (12%)** |
| 그중 표시가 달라지는 것 | **5 — 전부** |
| 판독값이 없어 오늘 동작을 유지하는 것 | 37 |

- 반전된 D6에서는 **판독값 보유 == 표시 변경**이다. 승격 여부는 무관하다 — 판독값이 승격을 덮으므로. (초안은 승격이 이미 교정한 2개를 변화 집합에서 뺐는데, 그것은 반전 전 D6의 셈법이었다.)
- 따라서 *"200k를 넘으면 이 기능의 대상에서 빠진다"*, *"대상 집합이 세션이 자라면서 줄어든다"*, *"`promoted_window`와 배타가 아니라 보완이다"* — 초안의 이 세 문장은 **전부 거짓이 되었다.** `600e6f90`이 반례다: 217,737로 이미 200k를 넘겼는데도 22% → 31%로 바뀐다.
- 나머지 37개는 `/context`를 실행하지 않아 판독할 값이 없다 → fallback. 이 경로가 오늘과 바이트 동일이라는 것이 D6의 보존 성질이다.

### Antigravity (AG4)
- 해당 dialect는 토큰 usage를 전혀 기록하지 않아 `context_used`가 영구히 0이고, `context_pct`는 `context_used == 0`에서 즉시 `None`을 반환한다 (`dashboard.rs:947`). 따라서 이 경로와 구조적으로 무관하다 — 별도 방어 불필요.

## L2 Checkpoints (최종)

복잡도: **Medium (3 signals), brownfield** — L2 심사에서 신호 1개 누락이 지적되어 수정.
- **Data schema design** — 새 스냅샷 필드 + 4단 우선순위 재배치.
- **State machine / multi-stage flow** — *(심사에서 추가)* D3의 4단 캐스케이드(config → 판독 →
  승격 → 테이블) + D4의 분기 **순서 제약** + D5의 last-wins 누적 + D9의 사용 가능 술어. 우선순위
  캐스케이드 + 사용 가능 판정 + 순서 제약은 rubric의 "pipeline"에 정확히 해당한다.
- **External resource loading** — 크레이트 최초의 터미널 출력 텍스트 파싱. *(심사 지적대로 셋 중
  가장 약하다: `ai-monitor`는 그 JSONL을 이미 로드하고 있다. 신호로 세되 근거는 약하다고 적어 둔다.)*

등급은 Medium 유지(2–3 signals)이나 **밴드가 틀렸었다**: Medium은 dimension당 4–5개다. 초안은
Error/Edge에 8개(Complex 밴드), Implementation에 3개(Simple 밴드)를 생성했다. 아래는 교정판.
가중치(brownfield): Core 30% / Scope 20% / Error 25% / Data 15% / Impl 10%.

| Dimension | R | P | U | Score |
|---|---|---|---|---|
| Core Behavior | 5 | 0 | 0 | 1.00 |
| Scope Boundaries | 4 | 0 | 0 | 1.00 |
| Error/Edge | 6 | 0 | 0 | 1.00 |
| Data Model | 4 | 0 | 0 | 1.00 |
| Implementation | 5 | 0 | 0 | 1.00 |

Implementation 체크포인트 (3 → 5로 증설, 심사 지적):
- [x] 훅 지점 `telemetry.rs:576` 디스패치
- [x] 테스트는 `fixtures/*.jsonl` + `include_str!`
- [x] ANSI 스트리퍼의 **배치와 시그니처** (D11)
- [x] 새 필드의 **구체 타입** — `TelemetrySnapshot`이 `PartialEq/Eq/Default` 파생이므로 계약에 영향 (D12)
- [x] 픽스처에 raw ANSI를 넣는 방식 + 기존 `session.jsonl` 스냅샷 단언에 미치는 영향 (D13)

Error/Edge는 8 → 5 → 6으로 움직였다. 2차 심사가 D8의 전제를 실측으로 반증하면서 그 체크포인트와
거기서 파생된 pair tension이 함께 사라져 5가 됐고, 4차 심사에서 **D1a**(판독 게이트의 조용한
오독 경로) 한 칸이 더해져 6이 됐다. 둘 다 R이므로 점수는 1.00 유지 — Composite은 움직이지 않는다.

**Composite: 1.00** (threshold 0.80, per-dimension floor 0.60) · Unknown/Unknowns: 0
Inversion Probe 통과 · Unresolved Sweep 잔여 **0건**.

> 1.00을 의심하라는 것이 이 저장소의 기준이므로 근거를 적어 둔다. 이 점수가 1.00인 것은
> 체크포인트가 적어서가 아니라, 마지막까지 남아 있던 단 하나의 provisional(D10)이 **"모호해서"가
> 아니라 "사용자가 직접 고르지 않아서"**였고 2026-09-11 게이트에서 확정됐기 때문이다. 개정
> 과정에서 체크포인트는 오히려 늘었다(Impl 3→5, Error/Edge 5→6). 진짜 잔여 불확실성은 점수가
> 아니라 **Known Gaps 10건**에 있다.

*(개정 1차에서 Data Model을 4R/1.00으로 올려 Composite 0.99를 적었는데, D10의 Status가
provisional인 것과 모순이었다 — L2 심사 2차에서 교정. 0.99는 그 모순 위에서만 성립했다.)*

Unknown/Unknown 탐지 결과:
- Tier 1 (Actor): 새 필드는 기존 `TelemetrySnapshot` 갱신 경로를 그대로 타므로 미커버 액터 없음.
- Tier 2 (Implication): `context_pct`가 모델 미상이면 즉시 `None`인데 판독값은 모델과 무관 → **+1 Core** (D4).
- Tier 3 (Pair, 성능): **기각됨.** 초안은 `local_command`이 모든 슬래시 명령에 붙으므로 매회 ANSI
  스트립이 G1 부채를 되살린다고 주장했다. 실측이 반증했다 — 전체 12,068줄에서 `subtype` 분포는
  `turn_duration 183 / stop_hook_summary 177 / away_summary 27 / local_command 22 /
  informational 1 / compact_boundary 1`. `local_command`은 **0.18%**이고, 22개 레코드의 ANSI
  스트립은 이미 12,068회 도는 `serde_json::from_str` 옆에서 측정 불가능하다. G1(렌더 프레임마다)
  과의 유비가 성립하지 않는다. → **D8 철회.**

## Decisions

### D1: 판독 앵커는 `type`/`subtype` **필드**다 — 본문 grep이 아니다
- **Status**: resolved
- **Rationale**: `type == "system" && subtype == "local_command"` 레코드의 `content`만 읽는다.
  (D1a가 여기에 `content`의 `Context Usage` 포함을 **추가로** 요구한다 — 이 결정은 필요조건이지
  충분조건이 아니다.)
  이 앵커가 필수인 이유는 가설이 아니라 **현재 로그에 살아 있는 사실**이다: 문자열
  `"Auto-compact window"`가 **5종 레코드 타입**에 공존한다 — `assistant` 18 / `user` 10 /
  `system` 6 / `attachment` 1 / `queue-operation` 1 (Research의 분포표 참조). 권위 있는
  `system`보다 산문이 **5배** 많다. 라인 전체 grep은 산문을 읽고 오독한다.
  - 이것은 `dashboard.rs:70-73`이 `[1m]`에 대해 기록한 함정과 **같은 모양**이다 —
    *"every `[1m]` occurrence in a real log is prose inside a message, never a field"*.
  - **선례 계승**: `telemetry.rs`의 task-notification 분기가 이미 같은 doctrine을 명시한다 —
    *"Routed on the TAG, acted on by the parse"*, *"a parse living there could be reached by
    anything that merely mentions a notification"* (D12/R4.3/R4.4). 이 결정은 그 규칙의 재적용이다.
  - 기각: **`content` 전체를 `scan_command_name`류 범용 스캐너에 흘리기** — 위 선례가 이미
    배격한 설계다. 임의 산문이 파싱에 도달하는 경로를 다시 만든다.

### D1a: `Context Usage` 게이트는 **판독과 계수 양쪽**에 적용된다 (4차 심사에서 신설)
- **Status**: resolved
- **Rationale**: 3차 개정은 `Context Usage` 게이트를 D10(계수) 안에만 적어 놓고, 판독 경로의
  게이트는 D1/D2의 `type`/`subtype` **단독**인 채로 두었다. 두 게이트가 다른데 다르다는 말이
  없어 **조용한 오독 경로**가 남았다.
- **위험이 가설이 아니다**: 게이트를 통과하는 비-`/context` 16건 중 하나의 원문이
  `<local-command-stdout>Kept model as ` + "`Opus 5 (1M context) (default)`" + `</local-command-stdout>` 이다.
  D7의 문법 `<숫자>[.<숫자>][k|K|m|M]`은 `1M` → 1,000,000을 받고 하한 1,000도 통과한다.
  헤더 앵커가 걸러 줄 것 같지만 **스펙은 헤더 앵커의 패턴을 한 번도 쓴 적이 없다**(D2는 예시만
  적었다). 즉 "판독 무게이트" + "헤더 패턴 미명세" + "실패해도 무계수"가 겹쳐,
  `/model` 출력에서 1M을 윈도우로 읽어도 **아무 신호가 없다.**
- **채택**: 판독·계수 **모두** `subtype=="local_command"` **그리고** `content`에 `Context Usage`
  포함을 요구한다. 게이트는 하나이고 D10은 그것을 참조할 뿐이다.
- **검사 시점**: `strip_sgr` **전**, 원문에서 검사한다. 실측상 `Context Usage`는 6건 모두 원문에서
  연속 바이트(`<local-command-stdout>` + `\033[1m` + `Context Usage`)라 전/후 어느 쪽이든 동작하지만,
  원문 검사가 게이트 탈락 레코드에서 스트립 비용을 아예 내지 않게 한다.
  - D1의 독트린(*"임의 산문이 파싱에 도달하는 경로를 다시 만들지 않는다"*)이 `local_command`
    **내부**에서도 성립하게 된다. 3차까지는 그 독트린이 레코드 타입 경계에서만 지켜졌다.
  - D2의 드리프트 내성은 보존된다 — 두 앵커가 방어하는 사건은 **앵커 라벨** 드리프트이고,
    게이트가 의존하는 것은 **패널 타이틀**이라 서로 독립이다.
  - 대가: 타이틀이 바뀌면 판독과 계수가 **함께** 죽는다 → 오늘 동작으로 fallback. G-i에 정확히
    그렇게 기록한다(3차의 G-i는 "카운터가 침묵한다"고만 적어 위험을 절반만 기술했다).

### D2: 앵커는 **오늘 존재하는 둘**이다 — 미래 표기를 추측하지 않는다
- **Status**: resolved (사용자 선택 "두 포맷 모두 받기"를 정제)
- **Rationale**: 같은 `content` 안에 같은 값이 두 곳에 있다:
  1. 전용 라인 `Auto-compact window: 700k tokens` — **authoritative**
  2. 헤더의 **분모** — corroboration / fallback. 문법을 예시가 아니라 패턴으로 못 박는다
     (4차 심사: 예시만으로는 `1M context` 같은 문자열을 배제한다고 말할 수 없다):
     ```
     <num>[.<num>][kKmM] '/' <num>[.<num>][kKmM] ' tokens ('
                           ^^^^^^^^^^^^^^^^^^^^ 이것이 분모
     ```
     슬래시와 뒤따르는 리터럴 ` tokens (` 가 **필수**다. 이 둘이 `Kept model as \`Opus 5 (1M
     context)\`` 같은 문자열을 문법 수준에서 배제한다.
  둘 다 시도하면 한쪽 표기가 바뀌어도 다른 쪽이 살아남을 수 있어, 사용자가 요구한 드리프트
  내성을 **실재하는 중복**으로 얻는다.
  - 기각: **아직 존재하지 않는 표기(`Context window: 700,000` 등)를 미리 지원** — 이 저장소는
    같은 판단을 이미 내렸다: 관측되지 않은 표기를 위한 규칙은 *"would be unreachable code"*
    (`dashboard.rs:70-72`). 지지할 포맷이 실재하지 않으므로 테스트도 픽스처도 만들 수 없고,
    검증 불가능한 코드가 남는다.
  - 두 앵커가 **서로 다른 값**을 내면 전용 라인을 채택하고 `context_window_anchor_conflicts`로
    계수한다(D10). 초안은 D7이라 잘못 참조했고, 이어 *"파싱 이상으로 계수"*라 적었는데 그 표현
    자체를 D10이 교정했다 — 앵커 불일치는 파싱 실패가 아니라 **소스 불일치**다 (3차 심사).
  - **사전 필터를 두지 않는다.** 초안의 D8은 `"Auto-compact window:"` 원문 부분문자열로 레코드를
    걸렀는데, 심사가 이것이 D2를 **정확히 무력화**함을 지적했다: 헤더 `74.7k/700k tokens (11%)`는
    전용 라인과 다른 줄이고 그 라벨을 포함하지 않으므로, 전용 라인의 라벨이 바뀌면 레코드가
    필터에서 탈락해 **두 번째 앵커가 영영 실행되지 않는다.** D2가 사려던 내성이 0이 된다.
    D8의 성능 전제도 실측에 반증되었으므로(0.18%) 필터는 존재 이유가 없다 — 제거.

### D3: 분모 우선순위는 `config > 판독값 > promoted > 테이블`
- **Status**: resolved (사용자 결정)
- **Rationale**: 사용자가 `[ai_monitor] context_window`를 썼다면 그것이 최우선 — 기존 doctrine
  (*"A user who has stated the window has knowledge neither of them has"*, `dashboard.rs:950-955`)을
  **그대로 보존**한다. 판독값은 추론이 아니라 읽기이므로 추측·승격보다는 강하지만, 사용자 진술보다는 약하다.
  - 기각: **판독값이 config를 이긴다** — 사용자에게 제시했고 거부되었다. 세션별 증거가 전역
    기본값보다 구체적이라는 논거는 성립하나, 이미 선 doctrine을 뒤집는 비용이 더 크다고 판단됨.

### D4: 판독값은 **모델 요구를 단락한다**
- **Status**: resolved (Tier 2 implication에서 발견)
- **Rationale**: 현재 `context_pct`는 `context_window(t.model.as_deref()?)?` — 모델을 모르면
  (`<synthetic>`만 있거나 모델 줄이 없는 로그) 즉시 `None`을 반환해 퍼센트를 아예 생략한다.
  판독값은 모델 id와 무관하게 성립하므로, 판독값이 있으면 모델 미상이어도 퍼센트를 낼 수 있다.
  구조상 판독값 분기가 `context_window(...)?` **앞에** 와야 한다 — 뒤에 두면 도달하지 못한다.

### D5: 같은 세션에 `/context`가 여럿이면 **last-wins**
- **Status**: resolved
- **Rationale**: 선례가 이미 이 저장소 안에서 갈라져 있고, 그 기준이 이 경우를 결정한다.
  `session_id`는 **first-wins** — *"A session id is fixed for the life of a file"*. `model`은
  **last-wins** — *"`/model` switches it mid-session ... first-wins pins the panel to whatever
  the session happened to start with"* (`telemetry.rs:548-556`).
  윈도우는 모델 티어를 따라가므로 세션 수명 동안 고정이 **아니다** → `model`과 같은 편, last-wins.

### D6: 판독값은 **종착값이다** — `promoted_window`를 통과시키지 않는다
- **Status**: resolved (**초안에서 전면 반전** — L2 심사 steelman이 옳았다)
- **초안의 오류**: "판독값을 `promoted_window(assumed, used)`의 `assumed` 자리에 넣는다"였다.
  근거는 *"판독값이 틀렸다는 증거가 나오면 측정이 이긴다"*. 이 논증은 **단위가 같을 때만** 성립하는데,
  로그 실측 결과 두 수는 **서로 다른 축 위에 있다**:
  - `/context`가 보고하는 700k는 **auto-compact 예산**이다. raw API 윈도우 1,000,000이 아니고,
    `1M − 33k buffer = 967k`도 아니다. buffer는 700k **안에 포함된** 조각이다
    (`33000/700000 = 4.714%` → 표시 `4.7%`로 일치).
  - `WINDOW_TIERS`는 `[200_000, 1_000_000]` — **raw 티어**다 (`dashboard.rs:92`).
  - 따라서 `promoted_window(700_000, 750_000)` → **`1_000_000`**. `/context`가 결코 표시하지
    않는 분모이며, 완료 기준("패널 퍼센트가 `/context`와 일치한다")을 **정면으로 위반**한다.
    게다가 하필 CONTEXT 타일이 가장 중요한 고점유 구간에서 그렇게 한다.
  - 요컨대 초안 D6은 **측정된 유일한 진실을 이 스펙이 제거하려던 바로 그 추측 테이블 값으로
    조용히 교체**했다. Non-goals의 buffer 오해(위에서 교정)가 그 뿌리였다.
- **반전이 실제로 바꾸는 것은 미관측 구간뿐이다 (L2 게이트에서 계산으로 확인)**. `promoted_window`는
  `used <= assumed`이면 `assumed`를 그대로 돌려주므로, 판독값 700,000을 `assumed`로 넣어도
  high-water가 700k 미만인 한 결과가 **판독값 그대로**다. 관측된 5개 세션의 high-water가 전부
  700k 미만이라 **구 D6과 신 D6의 출력은 5개 전부 동일하다**(56/46/31/12/6%).
  둘이 갈리는 것은 `used > 700k`일 때뿐이고 — 구 D6은 `1,000,000`(/context가 결코 표시하지 않는
  분모), 신 D6은 `>100%`(참인 진술) — 그 구간은 코퍼스에 없다.
  → 즉 반전이 **범위를 넓힌 것이 아니다.** 넓어진 것으로 보였던 것은 초안 D6의 셈법을 잘못
  기술한 탓이고, 실제로 반전이 산 것은 **미관측 엣지에서 거짓 분모를 내지 않는 것**이다.
- **채택**: 판독값이 있으면 그것이 분모다. 승격도, 테이블도 참조하지 않는다.
  - `context_used`가 판독값을 넘으면 기존 `>100%` 분기(`dashboard.rs:473`)가 뜬다. 이때 그 표시는
    추측이 아니라 **참인 진술**이 된다 — "auto-compact 임계를 넘었다". `>100%`의 의미가
    (테이블이 틀렸다 → 임계를 넘었다) 로 바뀌므로 해당 주석을 갱신해야 한다.
  - 판독값이 없으면 오늘과 **바이트 동일**하게 동작한다(테이블 추측 → 승격). 이 성질은 유지된다.
  - 기각: **비율로 스케일해 승격** — 두 축 사이의 환산 계수가 로그에 없다. 또 하나의 추측이 된다.
  - 기각: **승격시키되 `>100%`만 막기** — 증상만 가리고 분모는 여전히 `/context`와 불일치한다.

### D7: 파싱은 `k`/`M` 접미사와 소수점을 받고, **1,000 미만은 거부한다**
- **Status**: resolved (`assumed: true` — 하한값은 사용자 확인 없이 정함)
- **Rationale**: 표기는 `700k`, `74.7k`, `1.0M` 형태이며 raw digit이 아니다.
  문법은 `<숫자>[.<숫자>][k|K|m|M]` — 접미사 없는 맨 정수는 이 문법의 자연스러운 부분집합이므로
  받는다(D2의 "미관측 표기 추측 금지"에 저촉되지 않는다: 새 *표기*를 지어내는 것이 아니라
  같은 문법의 경계 사례다). `74.7k → 74,700`으로 반올림한다.
- **하한 1,000의 근거 — 초안의 설명은 틀렸다** (L2 심사에서 교정). 초안은 *"같은 라인에 `(11%)`가
  있어"*라고 썼으나, 전용 라인 원문에는 **퍼센트가 없다**:
  `\033[1mAuto-compact window: \033[22m\033[38;5;246m700k tokens\033[39m`.
  `(11%)`는 몇 줄 위 **헤더 라인**에 있다. D2가 그 헤더를 두 번째 앵커로 읽으므로 하한은 여전히
  방어적이다 — 헤더 `74.7k/700k tokens (11%)`에서 느슨한 정규식이 `11`을 분모로 집을 수 있다.
  실제 윈도우는 모두 1,000 이상이고 퍼센트는 0–100이므로 1,000이 두 값을 깨끗이 가른다.
  (`assumed: true` — 하한 숫자는 사용자 확인 없이 정했다.)
  - 파싱 결과는 **측정이 아니라 반올림된 읽기**다. `700k`는 697,000도 700,499도 될 수 있다.
    교차검증으로 이 세션은 정확히 700,000과 일관됨을 확인했으나(`33000/700000 = 4.714% → 4.7%`,
    `74677/700000 = 10.67% → 11%`), 그 일관성은 증명이 아니다. 필드 문서에 명시한다.

### D8: ~~ANSI 스트립 전 사전 필터~~ — **철회됨**
- **Status**: withdrawn (L2 심사에서 전제가 실측에 반증됨)
- 초안은 `subtype == "local_command"`가 모든 슬래시 명령에 붙으므로 매회 ANSI 스트립이 G1 부채를
  되살린다며 원문 부분문자열 사전 필터를 두었다. 두 가지가 틀렸다:
  1. **성능 전제 반증**: `local_command`은 전체 12,068줄 중 **22건(0.18%)**이다. 22회 스트립은
     이미 12,068회 도는 `serde_json::from_str` 옆에서 측정 불가능하다.
  2. **D2를 무력화**: 필터 키가 전용 라인의 라벨이므로, 그 라벨이 바뀌면 레코드가 통째로 탈락해
     두 번째 앵커(헤더)가 실행될 기회를 잃는다. 드리프트 내성을 위해 만든 D2를 드리프트 시점에
     정확히 꺼 버린다.
- 번호는 공석으로 두지 않고 **철회 기록으로 남긴다** — 나중에 "성능상 필터가 필요하지 않나"가
  다시 제기될 때 재조사를 막기 위해서다.

### D11: ANSI 스트리퍼는 **직접 구현**하고, 일반 CSI-SGR을 처리한다
- **Status**: resolved
- **Rationale**: 워크스페이스에 기존 구현이 없고(`ai-monitor`/`app` 전수 확인), `ai-monitor`의
  의존성은 `serde_json`+`notify` 둘뿐이다. 크레이트 추가를 정당화하지 못한다.
- **`38;2` 전용으로 짜면 안 된다** (L2 심사에서 발견): 5개 로그 전수 확인 결과 두 인코딩이 공존한다 —
  truecolor `\033[38;2;r;g;b m` 4건, 256색 `\033[38;5;246m` 1건(`77de50ce`). 한 샘플만 보고
  구현하면 절반이 깨진다. 따라서 **CSI(`\033[` … 최종 바이트 `m`) 일반형**을 제거한다.
- **CSI-`m`만 처리하면 충분함이 코퍼스로 확정된다** (2026-09-11 실측): 5개 로그의 `local_command`
  `content` 전체에서 이스케이프 **574건, 최종 바이트 100% `m`**, OSC 0건. 즉 "CSI + 최종 바이트
  `m` 제거"가 필요충분이다. 이 수치를 남기는 이유는 D8 철회 기록과 같다 — 나중에 "OSC도 벗겨야
  하지 않나"가 재조사되는 것을 막는다.
- 배치: `telemetry.rs` 내부의 private 자유 함수. 시그니처는 `fn strip_sgr(s: &str) -> String`.
  모듈을 새로 만들지 않는 이유는 소비자가 이 파일 하나뿐이기 때문이다.

### D9: 판독값은 **읽힌 시점의 모델과 짝지어** 저장하고, 현재 모델과 일치하지 않으면 **사용하지 않는다**
- **Status**: resolved (Inversion Probe에서 발견)
- **Rationale**: Inversion이 드러낸 실재 구멍 — 세션 초반에 `/context`를 1M에서 한 번 실행하고
  `/model`로 200k 모델로 바꾸면, last-wins(D5)는 새 `/context`가 없으므로 **구제하지 못한다.**
  판독값 1M이 굳은 채 패널이 **과소보고**한다. 과소보고는 이 스펙이 가장 피하려는 방향이다
  (CONTEXT 타일의 존재 이유가 "곧 찬다"를 알리는 것이므로).
  - 따라서 `(window, model_at_read)` 쌍으로 저장하고, 현재 `model`이 그와 다르면 판독값을
    사용하지 않고 D3의 다음 단계(승격/테이블)로 떨어뜨린다.
  - 기각: **Known Gap으로 기록만 하기** — 고칠 수 없어서가 아니라 상태 한 칸이 아까워서
    남기는 구멍이 되고, 하필 틀리는 방향이 위험한 쪽이다.
- **비교 규칙 (L2 심사 지적 — 초안은 "바뀌면"이 미정의였다)**:
  - **문자열 완전 일치**로 비교한다. 티어 비교는 채택하지 않는다 — 어떤 id가 어떤 티어인지가
    바로 이 스펙이 "판독 불가"라고 확정한 정보이므로, 티어 비교는 제거하려는 추측을 되불러온다.
    대가는 `claude-opus-5` → `claude-opus-5-20260301` 같은 동일 티어 내 변경에서 정당한 판독값을
    잃는 것이다. 잃는 방향이 안전한 쪽(과대보고)이므로 수용한다.
  - **술어로 정의한다 — 상태 변이가 아니다 (3차 심사에서 재작성)**. `TelemetrySnapshot`은 판독
    시점의 쌍을 그대로 싣고, 사용 가능 여부는 **소비 시점에 판정**한다:

    ```
    사용 가능  <=>  model_at_read.is_none() || model_at_read == current_model
    ```

    | `at_read` → `current` | 판정 | 이유 |
    |---|---|---|
    | `None` → `None` | 사용 | 모델 미상 로그. D4가 상정한 사례 |
    | `None` → `Some(x)` | 사용 | `None`은 "다른 모델"이 아니라 **"아직 증거 없음"** |
    | `Some(a)` → `Some(b)`, `a != b` | 불가 | 진짜 모델 전환 |
    | `Some(a)` → `None` | 불가 | 오늘은 도달 불가(`is_real_model` 필터가 `None`으로 되돌리지 않고 `restarted()`는 누산기를 통째로 버린다 — `telemetry.rs:558-566`). 그래도 네 번째 팔을 명시한다 |

    **4행의 대가를 정직하게 적는다 (4차 심사에서 교정)**: 3차는 이 칸의 이유를 *"확인 불가면
    쓰지 않는 쪽이 안전(과대) 방향"*이라 적었는데 **틀렸다**. 판독값을 쓰지 않으면 D3의 다음
    단계가 `context_window(t.model.as_deref()?)?`인데 `current_model == None`이라 `?`가 즉시
    `None`을 반환한다(`dashboard.rs:969`). 결과는 과대도 과소도 아닌 **퍼센트 자체의 소멸**이고,
    그것은 **D4가 없애려고 존재하는 바로 그 실패 모드**다. 즉 D9 4행과 D4는 긴장 관계이며,
    갈리는 기준은 `model_at_read`가 채워졌는지 — 판독이 언제 일어났는지라는 우연뿐이다.
    오늘 도달 불가이므로 수용하되, **이 팔을 도달 가능하게 만드는 사람은 D4와 함께 재검토할 것.**

    **술어를 택한 이유 셋**:
    1. 초안의 *"폐기"*와 *"무효화"*가 서로 다른 것을 뜻했고(저장값 삭제 vs 읽는 시점 판정) 어느
       쪽인지 정해지지 않아 GWT를 쓸 수 없었다. 관측 가능한 차이도 있다 — 모델이 A → B → **A**로
       돌아오면 술어에서는 판독값이 되살아나고 변이에서는 영구 소실이다. 판독값은 "A에서 측정된
       사실"이므로 A로 돌아오면 다시 유효한 것이 맞다.
    2. 세션 시작 직후 어떤 assistant 턴보다 먼저 `/context`를 치는 평범한 사용에서, 초안의 단순
       `!=` 판정은 첫 assistant 턴이 모델을 넣는 순간 정당한 판독값을 **조용히** 버렸다(모델
       전환은 일어나지도 않았고 D10이 그 경로를 계수하지도 않는다).
    3. 술어면 `model_at_read`를 **단독 갱신할 일이 없다** — 2차 개정의 "pairing 승격"은 쌍의 한쪽만
       바꾸는 변이라 D12의 원자성 근거를 스스로 무너뜨리고 있었다(3차 심사 지적).
    (실측: 5개 로그 전부에서 `/context` 레코드가 첫 모델 라인 **뒤**에 와 `model_at_read`는 항상
    `Some`이다. 2·4행은 코퍼스에 없지만 도달 가능하다 — 그래서 네 칸을 다 적는다.)
- **한계 (L2 심사에서 발견, 숨기지 않고 기록)**: 사용하지 않기로 판정된 뒤 떨어지는 다음 단계는
  `promoted_window(context_window(model), context_high_water.max(...))`인데 `context_high_water`는
  **monotone, never reset**이다(`telemetry.rs:302`). 1M 시절 250k를 쓴 세션이 200k 모델로 바뀌면
  `promoted_window(200_000, 250_000)` → `1_000_000`이 되어 **과소보고가 그대로 남는다.**
  즉 D9는 high-water가 200k 미만인 구간에서만 목표를 달성한다. 이는 `promoted_window`의
  기존 성질이지 이 스펙이 만든 결함은 아니므로 G-f로 기록한다.

### D10: 두 앵커가 모두 실패하면 **조용히 기존 동작으로 떨어지되, 진단 카운터를 남긴다**
- **Status**: resolved — **2026-09-11 사용자가 L2 게이트에서 채택을 확정**했다. 그 전까지는
  `assumed`였고, 계약 자체는 2차 심사에서 완전히 명세된 상태였다.
- **Rationale**: 판독 실패는 "값이 없음"과 동일하게 취급되어 오늘 동작으로 복귀한다 — 패널이
  거짓을 말하지는 않는다(추측임을 이미 아는 상태로 돌아갈 뿐).
  그러나 카운터 없이는 이 경로가 **관측 불가능**하다: 두 앵커가 모두 깨져도 아무 신호가 없어
  테스트가 "포맷이 바뀌었다"를 단정할 수 없고, CLAUDE.md의 변이 판정 기준에서 이 경로는
  생존자를 만든다. `usage_conflicts`(`telemetry.rs:349`)가 같은 목적의 선례다.
- **계수 계약 (2차 심사에서 전면 재작성 — 초안은 "정확히 한 조건"이라 써 놓고 조건이 둘이었다)**:

  **두 개의 필드로 나눈다.** 초안은 `context_window_parse_failures` 하나에 파싱 실패와 앵커 불일치를
  함께 넣었는데, 이 저장소의 독트린이 정확히 그것을 금지한다 (`telemetry.rs:342-349`):
  *"Kept apart from the `parse_errors` family below: those count PARSER faults, which is a different
  fact — displaying one under the other's label would be a lie about what failed."*
  앵커 불일치는 파싱 실패가 아니라 **소스 불일치**다.

  | 필드 | 증가 조건 (각각 **정확히 하나**) |
  |---|---|
  | `context_window_parse_failures: u64` | `/context` 레코드에서 **두 앵커가 모두** 값을 내지 못했거나, 낸 값이 하한(1,000) 미만일 때 |
  | `context_window_anchor_conflicts: u64` | 두 앵커가 **모두 성공했으나 서로 다른 값**을 냈을 때 (전용 라인을 채택하되 신호는 남긴다) |

  둘 다 계수하지 **않는** 경우: 한 앵커만 실패하고 다른 앵커가 성공한 경우(D2의 중복이 의도대로
  작동한 것), 그리고 `/context`가 아닌 `local_command` 레코드.

- **게이트 정의 — 두 번 틀렸고, 3차 심사에서 제3의 답이 나왔다**:
  - **1차 오류(라벨 게이트)**: 전제를 *"두 앵커 중 최소 하나의 라벨이 존재하는 레코드"*로 걸었는데
    **헤더 앵커에는 라벨이 없다**(원문 `74.7k/700k tokens (11%)`). 게이트가 전용 라인의
    `Auto-compact window:` 하나에 걸리고, 그 라벨이 드리프트하는 순간 레코드가 게이트를 통과하지
    못해 **계수되지 않는다** — 카운터의 유일한 존재 이유가 정확히 그 사건에서 죽는다. 철회한 D8이
    D2를 무력화하던 것과 동형의 자기부정.
  - **2차 오류(무게이트)**: `subtype=="local_command"` 단독으로 넓히고 *"기준선이 상수가 된다"*고
    적었다. **실측이 반증했다** — `local_command` 22건 중 윈도우 보유는 **6건뿐**이고 16건(73%)이
    통과한다. 내역: `/context` **에코 레코드 6건**(`<command-name>/context</command-name>`) ·
    빈 레코드 6건 · `/model` 에코 2건 · 기타 2건. 즉 **`/context`를 성공적으로 읽을 때마다 실패
    카운터가 같이 1 오르고**, `/context`를 한 번도 안 쓴 세션에서도 오른다. 기준선은 상수가 아니라
    **슬래시 명령 사용량에 비례하는 변수**이고 신호 대 잡음이 16:1이다.
  - **채택 (3차, 4차에서 D1a로 승격)**: 게이트 정의는 **D1a가 소유한다** — `subtype=="local_command"`
    이고 `content`에 `Context Usage` 포함. D10은 그것을 참조할 뿐이며, 정의를 여기 중복 서술하지
    않는다(두 곳에 적으면 다음 개정에서 갈라진다).
  - 기각: 헤더용 라벨로 `tokens (` — `System prompt: 4.7k tokens (0.7%)` 등 카테고리 라인 7줄이
    전부 걸려 게이트가 의미를 잃는다.
  - 남는 위험: 패널 타이틀이 바뀌면 카운터가 침묵할 뿐 아니라 **판독까지 함께 죽는다**(D1a가
    게이트를 두 경로에 공유시켰으므로). 3차의 이 자리는 "카운터가 침묵한다"고만 적어 위험을
    절반만 기술했다 → G-i가 정본이다.

### D12: 새 필드는 `Option<ContextWindowReading>` — 두 값을 한 구조체로 묶는다
- **Status**: resolved (Implementation 체크포인트 증설에서 발견)
- **Rationale**: `TelemetrySnapshot`은 `PartialEq/Eq/Default`를 파생하므로 필드 형태가 계약이다.
  D9가 `(window, model_at_read)`를 **함께** 읽어야 하므로 둘은 **원자적으로** 갱신되어야 한다.
  `Option<ContextWindowReading { tokens: u64, model_at_read: Option<String> }>`로 묶으면 한 번의
  대입으로 둘이 같이 바뀌므로, 윈도우만 갱신되고 모델 기록은 옛것으로 남는 **어긋남이 발생할 수
  없다** — 두 개의 독립 필드는 그 어긋남을 코드 실수 하나로 만들어낼 수 있다.
  - D9가 **술어**로 정의되면서(3차) `model_at_read`를 단독 갱신하는 지점이 사라졌다. 쌍은 판독
    시점에 한 번 통째로 쓰이고 더 새 판독(D5)에만 통째로 교체된다 — 원자성이 설계상 보장된다.
  - **정정 (L2 심사 2차)**: 초안은 이 근거를 *"윈도우는 있는데 모델 기록은 없는 상태가 구성상
    불가능해진다"*로 적었는데 **거짓이다**. `Some(ContextWindowReading { tokens: 700_000,
    model_at_read: None })`이 정확히 그 상태이고, D9가 그것을 **요구한다**(모델 미상 로그).
    이 타입이 사는 것은 unrepresentability가 아니라 **원자성**이다. 따라서 atlas D1의
    *"불변식이 아니라 우연"* 유비도 성립하지 않으므로 철회한다.
- 문서 주석에 **반올림된 읽기**임을 명시한다 (D7/G-c).

### D13: 픽스처는 기존 `session.jsonl`을 건드리지 않고 **새 파일**로 추가한다
- **Status**: resolved (2차 심사에서 provisional 사유가 해소됨)
- **Rationale**: 기존 `fixture_transcript_folds_to_expected_telemetry`는 `session.jsonl` 전체를
  스냅샷으로 단언하므로, 거기에 `local_command` 레코드를 끼워 넣으면 이 기능과 무관한 스냅샷이
  전부 흔들린다. 새 픽스처(예: `context_command.jsonl`)로 분리하면 영향이 0이다.
  raw ANSI는 JSON 문자열 이스케이프 **`\u001b`**로 체크인한다 (실측: 로그에 `\u001b` **328건**, `\u001B` **2건**이
  공존하며 `serde_json`은 둘을 동일 디코드한다 — 초안의 "실제 로그의 형태"라는 단정은 코퍼스가
  반증했다, 3차 심사). 어느 쪽이든 디코드 결과가 같으므로 다수인 소문자를 택한다.
- **provisional 사유 해소 (2차 심사)**: `tests/telemetry.rs:1189-1203`의 픽스처 순회 테스트가 새
  파일에 요구하는 것은 리터럴 `"isAsync": false`를 포함하지 않을 것 하나뿐임이 확인됐다.
  새 픽스처가 그것만 피하면 되므로 resolved로 올린다.
- **주의**: 이 스펙 파일 자체가 초안에서 `\u001b`를 쓰려다 **실제 ESC 바이트(0x1b)를 문서에 박는**
  사고를 냈다(2차 심사에서 "코드 스팬이 비어 있다"로 관측됨 — ESC는 보이지 않는다).
  픽스처를 만들 때 같은 실수가 나면 테스트가 조용히 다른 것을 검증하게 된다.

## Constraints
- `term-core`는 이 작업과 무관하다. 변경은 `ai-monitor`(파싱)와 `app`(소비)에 국한된다.
  `ai-monitor`는 내부 크레이트 의존이 없고 `app`만 그것을 의존하므로 dependency-direction 유지.
- 외부 크레이트를 추가하지 않는다 (**D11** — 초안은 철회된 D8을 참조했다).
- `no unwrap()` outside tests.
- 테스트는 `tests/fixtures/*.jsonl` + `include_str!`만 쓴다 — 라이브 로그도, 시계도 읽지 않는다
  (`tests/telemetry.rs:4`의 명시적 관례).
- 판독값이 없는 세션의 동작은 **오늘과 바이트 동일**해야 한다 (D6).
- `cargo clippy --workspace --all-targets -- -D warnings` 통과.

## Known Gaps
- **G-a. config를 설정하면 이 기능은 영구히 발동하지 않는다** (D3의 필연적 귀결, Inversion
  Implication에서 확인). config와 판독값은 실무상 **상호 배타**다. 사용자가 우선순위를 그렇게
  정했으므로 결함이 아니지만, 나중에 "왜 판독이 안 먹지"로 오인되지 않도록 명시해 둔다.
- **G-b. 커버리지는 좁지만, 좁은 방식이 초안과 다르다.** 전체 42개 로그 중 `/context` 기록
  보유 **5개(12%)**이고, 반전된 D6에서는 그 **5개 전부** 표시가 달라진다. 나머지 37개는 오늘
  동작을 유지한다.
  - ⚠️ **초안 G-b의 "3개(7%)"와 "세션이 200k를 넘기면 대상에서 빠지므로 대상 집합이 줄어든다"는
    둘 다 거짓이다** — 반전 전 D6의 셈법이었다. Research가 그 문장들을 이미 거짓으로 선언했는데
    G-b만 갱신되지 않아 스펙이 자기모순 상태였다(3차 심사에서 발견). 승인 게이트에서 사용자가
    읽는 것이 이 절이므로 blocking이었다.
  - 좁은 것은 **대상 세션 수**(5/42)이지 세션당 효과가 아니다. 효과는 최대 29pp로 크다.
- **G-c. 반올림된 읽기다** (D7). `700k`는 정확히 700,000임이 이 세션에서 교차검증으로 일관했으나
  증명은 아니다. 최대 오차는 표기 단위의 절반(±500 토큰, 0.07%)으로 퍼센트 표시에서는 무해하다.
- **G-d. L2 provisional 잔여: 없음.** 마지막 1건(D10 진단 카운터)은 2026-09-11 사용자가
  채택을 확정해 resolved로 올라갔다.
- **G-e. 판독값은 스냅샷이지 라이브 게이지가 아니다.** `/context` 실행 시점에 얼어붙는다.
  D9가 모델 변경이라는 가장 위험한 사용-불가 사유는 잡지만, 그 외 사유로 윈도우가 바뀌는 경로가
  생기면(오늘은 없음) 감지하지 못한다.
- **G-f. D9는 high-water가 200k 미만일 때만 목표를 달성한다** (L2 심사에서 발견). 판독값을
  사용하지 않기로 판정한 뒤 떨어지는 `promoted_window`가 **monotone `context_high_water`**를 읽기 때문이다
  (`telemetry.rs:302`: *"monotone, never reset"*). 1M 시절 250k를 쓴 세션이 200k 모델로 바뀌면
  분모가 다시 1M이 되어 과소보고가 남는다. 이는 `promoted_window`의 기존 성질이며 이 스펙이
  만든 결함이 아니다 — 고치려면 high-water를 모델 전환 시 재설정해야 하는데, 그러면 `/compact`
  후 amnesiac 재파생을 막으려던 그 필드의 존재 이유와 충돌한다. 별도 과제.
- **G-g. `>100%`의 의미가 경로에 따라 달라진다** (D6의 귀결). 판독값 경로에서는 "auto-compact
  임계를 넘었다"(참인 진술), 테이블 경로에서는 "이 빌드가 아는 어떤 윈도우보다 크다"(추측이
  틀렸다는 신호)를 뜻한다. `dashboard.rs:462-472`의 주석이 후자만 설명하므로 갱신이 필요하다.
  타일에 두 경우를 구분해 표시하지는 **않는다** — 사용자가 표시 형태 변경을 non-goal로 정했다.
- **G-h. 200k 모델의 `/context` 샘플이 코퍼스에 0건이다** (L2 심사에서 발견). 42개 로그의
  `/context` 기록 5건이 전부 700k를 보고한다. 따라서 "200k 모델은 `/context`에서 무엇을
  보고하는가"는 **관측되지 않았다.** D6(판독값 종착)은 그 값이 무엇이든 그대로 쓰므로 정의상
  안전하지만, D7의 하한(1,000)과 파싱 문법이 그 미관측 표기에서도 성립하는지는 확인되지 않았다.
- **G-i. 판독과 계수가 모두 패널 타이틀(`Context Usage`)에 의존한다** (D1a). 그 타이틀이 바뀌면
  카운터가 침묵할 뿐 아니라 **기능 자체가 죽어** 오늘 동작으로 fallback한다. (3차의 G-i는
  "카운터가 침묵한다"고만 적어 위험을 절반만 기술했다 — 4차 심사에서 교정.)
  앵커 라벨 드리프트와는 **독립된 사건**이라 1차 게이트 오류(게이트 키 == 앵커 키)와 같은 자멸
  구조는 아니지만, 단일 의존이라는 점은 남는다. 두 카운터가 오래 0이면 게이트부터 의심할 것.
- **G-j. 카운터는 "무엇이" 실패했는지는 말하지만 "왜"는 말하지 않는다.** 3차 게이트가 잡음을
  0/16으로 없앴으므로, `context_window_parse_failures > 0`은 이제 **"`/context` 레코드가 있었고
  두 앵커 모두 읽지 못했다"**는 확정적이고 행동 가능한 진술이다 — 라이브 로그에서도 유효하다.
  남는 한계는 **원인 특정**뿐이다: 포맷 드리프트인지, 잘린 레코드인지, 하한 미달인지를 카운터가
  구별하지 못한다. (3차의 G-j는 2차 무게이트 시절 결론인 "라이브 진단용이 아니다"를 게이트 교체
  후 재유도하지 않고 옮겨 와 카운터를 과소평가했다 — 4차 심사에서 교정.)

## Requirements

경계 분해 기준: **`ai-monitor`(생산 — 로그를 판독해 `TelemetrySnapshot`에 싣는다) ↔
`app/dashboard.rs`(소비 — 스냅샷을 읽어 분모를 고른다)**. 생산 측 요구사항마다 소비 측
대응이 존재해야 한다.

### R0: `/context`를 실행한 세션에서 패널 퍼센트가 `/context`와 일치한다

#### R0.1: 판독값이 있는 세션의 퍼센트가 `/context`와 같다
- **Given**: 세션 로그에 `/context` 출력 레코드가 있고 그것이 윈도우 700,000을 보고하며, 최신 `context_used`가 217,737이다
- **When**: 패널이 그 탭의 CONTEXT 타일을 그린다
- **Then**: 타일이 `31%`를 표시한다 (오늘은 `22%`)

#### R0.2: 판독값이 없는 세션은 오늘과 바이트 동일하게 동작한다
- **Given**: 세션 로그에 `/context` 출력 레코드가 하나도 없다
- **When**: 패널이 CONTEXT 타일을 그린다
- **Then**: 이 변경 이전과 **동일한** 퍼센트가 나온다 (`MODEL_WINDOWS` 추측 → `promoted_window` 승격 경로가 그대로 돈다)

#### R0.3: Antigravity 세션은 영향을 받지 않는다
- **Given**: `agy` dialect 로그라 토큰 usage가 전혀 기록되지 않아 `context_used == 0`이다
- **When**: 패널이 CONTEXT 타일을 그린다
- **Then**: `context_pct`가 `None`을 반환해 타일이 `—`를 표시하며, 판독 경로는 실행되지 않는다

---

### R1: 판독 게이트 *(D1, D1a — 생산 측)*

#### R1.1: 게이트를 통과한 레코드에서만 윈도우를 판독한다
- **Given**: `type == "system"`, `subtype == "local_command"`, `content`에 `Context Usage`를 포함하는 레코드가 로그에 있다
- **When**: 파서가 그 줄을 접는다
- **Then**: 윈도우가 판독되어 `TelemetrySnapshot`의 판독 필드가 채워진다

#### R1.2: `assistant` 산문에 같은 문자열이 있어도 판독하지 않는다
- **Given**: `type == "assistant"`인 레코드의 본문에 `Auto-compact window: 999k tokens`라는 문자열이 인용되어 있다
- **When**: 파서가 그 줄을 접는다
- **Then**: 판독 필드가 `None`으로 유지되고 `999000`은 어디에도 저장되지 않는다

#### R1.3: `Context Usage`가 없는 `local_command`는 판독하지 않는다
- **Given**: `content`가 `<local-command-stdout>Kept model as `Opus 5 (1M context) (default)`</local-command-stdout>`인 `local_command` 레코드가 있다
- **When**: 파서가 그 줄을 접는다
- **Then**: 판독 필드가 `None`으로 유지된다 (`1M`이 1,000,000으로 읽히지 않는다)

#### R1.4: 게이트는 `strip_sgr` 이전에 원문에서 검사한다
- **Given**: 게이트를 통과하지 못하는 `local_command` 레코드가 있다
- **When**: 파서가 그 줄을 접는다
- **Then**: 그 레코드에 대해 `strip_sgr`이 호출되지 않는다

---

### R2: 앵커·문법·ANSI *(D2, D7, D11 — 생산 측)*

#### R2.1: 전용 라인에서 윈도우를 판독한다
- **Given**: 게이트를 통과한 `content`에 `\033[1mAuto-compact window: \033[22m\033[38;5;246m700k tokens\033[39m`가 있다
- **When**: 파서가 윈도우를 판독한다
- **Then**: `700000`이 판독된다 (라벨과 숫자 사이의 SGR 시퀀스가 방해하지 않는다)

#### R2.2: 전용 라인이 없으면 헤더 분모에서 판독한다
- **Given**: 게이트를 통과한 `content`에 `Auto-compact window:`는 없고 `74.7k/700k tokens (11%)`만 있다
- **When**: 파서가 윈도우를 판독한다
- **Then**: 분모 `700000`이 판독된다 (분자 `74.7k`나 퍼센트 `11`이 아니다)

#### R2.3: 헤더 문법은 카테고리 라인을 배제한다
- **Given**: 게이트를 통과한 `content`에 `System prompt: 4.7k tokens (0.7%)`와 `Autocompact buffer: 33k tokens (4.7%)`가 있다
- **When**: 헤더 앵커가 매치를 시도한다
- **Then**: 두 줄 모두 매치하지 않는다 (`/` 와 리터럴 ` tokens (` 요구 때문)

#### R2.4: 두 앵커가 다른 값을 내면 전용 라인을 채택한다
- **Given**: 전용 라인이 `700k`, 헤더 분모가 `1.0M`을 낸다
- **When**: 파서가 윈도우를 판독한다
- **Then**: `700000`이 저장되고 `context_window_anchor_conflicts`가 1 증가한다

#### R2.5: `k`/`M` 접미사와 소수점을 받는다
- **Given**: 값이 각각 `700k`, `74.7k`, `1.0M`, `700000`이다
- **When**: 파서가 각각을 숫자로 변환한다
- **Then**: `700000`, `74700`, `1000000`, `700000`이 된다

#### R2.6: 1,000 미만은 거부한다
- **Given**: 판독된 값이 `11`이다 (퍼센트를 잘못 집은 경우)
- **When**: 파서가 하한을 검사한다
- **Then**: 값이 버려지고 판독 필드가 `None`으로 유지되며 `context_window_parse_failures`가 1 증가한다

#### R2.7: ANSI 스트리퍼가 두 SGR 인코딩을 모두 제거한다
- **Given**: 입력에 truecolor `\033[38;2;153;153;153m`와 256색 `\033[38;5;246m`가 모두 들어 있다
- **When**: `strip_sgr`이 돈다
- **Then**: 두 시퀀스가 모두 제거되고 그 사이의 텍스트는 보존된다

---

### R3: 스냅샷 계약 *(D5, D12 — 경계 자체)*

#### R3.1: 같은 세션에 `/context`가 여러 번이면 마지막 것이 남는다
- **Given**: 한 세션 로그에 `/context` 레코드가 두 개 있고 각각 `200k`, `700k`를 보고한다
- **When**: 파서가 파일을 끝까지 접는다
- **Then**: 판독 필드가 `700000`을 담는다

#### R3.2: 윈도우와 모델이 한 구조체로 원자적으로 저장된다
- **Given**: 판독이 일어나는 시점의 `self.model`이 `Some("claude-opus-5")`다
- **When**: 파서가 판독 결과를 싣는다
- **Then**: `Some(ContextWindowReading { tokens: 700000, model_at_read: Some("claude-opus-5") })`가 한 번의 대입으로 저장된다 (두 값이 따로 갱신되는 지점이 없다)

---

### R4: 분모 우선순위 *(D3, D4, D6 — 소비 측)*

#### R4.1: config가 판독값을 이긴다
- **Given**: `[ai_monitor] context_window = 500000`이 설정돼 있고, 스냅샷의 판독값은 `700000`이다
- **When**: `context_pct`가 분모를 고른다
- **Then**: `500000`이 분모가 된다

#### R4.2: 판독값이 승격과 테이블을 이기고, 승격을 통과하지 않는다
- **Given**: config가 없고, 판독값이 `700000`, `context_high_water`가 `750000`이다
- **When**: `context_pct`가 분모를 고른다
- **Then**: 분모가 `700000`이다 (`promoted_window`가 호출되지 않아 `1000000`으로 올라가지 않는다)

#### R4.3: 모델을 몰라도 판독값이 있으면 퍼센트를 낸다
- **Given**: 스냅샷의 `model`이 `None`이고(`<synthetic>`만 있는 로그), 판독값이 `700000`, `context_used`가 `70000`이다
- **When**: `context_pct`가 호출된다
- **Then**: `Some(10)`을 반환한다 (오늘은 모델 미상이라 `None`을 반환한다)

#### R4.4: 판독값을 넘으면 `>100%`가 뜬다
- **Given**: 판독값이 `700000`이고 `context_used`가 `750000`이다
- **When**: 타일이 그려진다
- **Then**: 값이 `>100%`로 표시되고 게이지가 가득 찬다

---

### R5: 모델 짝 술어 *(D9 — 소비 측)*

#### R5.1: 모델이 그대로면 판독값을 쓴다
- **Given**: `model_at_read == Some("claude-opus-5")`이고 현재 `model == Some("claude-opus-5")`다
- **When**: `context_pct`가 판독값 사용 가능 여부를 판정한다
- **Then**: 사용 가능으로 판정되어 판독값이 분모가 된다

#### R5.2: 판독 시점에 모델이 없었으면 나중에 모델이 들어와도 쓴다
- **Given**: `model_at_read == None`이고 현재 `model == Some("claude-opus-5")`다
- **When**: `context_pct`가 판정한다
- **Then**: 사용 가능으로 판정된다 (`None`은 "다른 모델"이 아니라 "증거 없음")

#### R5.3: 모델이 바뀌면 판독값을 쓰지 않는다
- **Given**: `model_at_read == Some("claude-opus-5")`이고 현재 `model == Some("claude-haiku-4-5")`다
- **When**: `context_pct`가 판정한다
- **Then**: 사용 불가로 판정되어 D3의 다음 단계(`MODEL_WINDOWS` → `promoted_window`)로 떨어진다

#### R5.4: 모델이 되돌아오면 판독값이 다시 유효해진다
- **Given**: `model_at_read == Some("claude-opus-5")`이고, 현재 `model`이 `claude-haiku-4-5`를 거쳐 다시 `claude-opus-5`다
- **When**: `context_pct`가 판정한다
- **Then**: 사용 가능으로 판정된다 (술어이므로 저장값이 삭제되지 않았다)

---

### R6: 진단 카운터 *(D10 — 생산 측)*

#### R6.1: 두 앵커가 모두 실패하면 계수한다
- **Given**: 게이트를 통과한 레코드인데 전용 라인도 헤더도 값을 내지 못한다
- **When**: 파서가 그 레코드를 끝낸다
- **Then**: `context_window_parse_failures`가 정확히 1 증가하고 `context_window_anchor_conflicts`는 변하지 않는다

#### R6.2: 한 앵커만 실패하면 계수하지 않는다
- **Given**: 전용 라인이 없고 헤더만 `700k`를 성공적으로 낸다
- **When**: 파서가 그 레코드를 끝낸다
- **Then**: 두 카운터 모두 변하지 않고 판독 필드가 `700000`을 담는다

#### R6.3: 게이트를 통과하지 못한 레코드는 계수하지 않는다
- **Given**: `Context Usage`가 없는 `local_command` 레코드 16건이 로그에 있다
- **When**: 파서가 파일을 끝까지 접는다
- **Then**: 두 카운터 모두 `0`이다

---

### R7: 테스트 픽스처 *(D13)*

#### R7.1: 새 픽스처가 기존 스냅샷을 흔들지 않는다
- **Given**: `tests/fixtures/context_command.jsonl`이 추가된다
- **When**: `cargo test --workspace`가 돈다
- **Then**: `fixture_transcript_folds_to_expected_telemetry`의 `session.jsonl` 스냅샷이 변경 전과 동일하게 통과한다

#### R7.2: 새 픽스처가 픽스처 순회 테스트의 불변식을 지킨다
- **Given**: 새 픽스처 파일이 `tests/fixtures/`에 있다
- **When**: 픽스처 디렉터리 순회 테스트가 돈다
- **Then**: 통과한다 (리터럴 `"isAsync": false`를 포함하지 않는다)

## Tasks

분할 기준은 **파일 경계와 인터페이스 경계**다. `telemetry.rs`(생산)와 `dashboard.rs`(소비)는
서로 다른 파일이고 크레이트도 다르므로 병렬 가능하지만, `telemetry.rs` 안의 작업들은 같은 파일을
건드리므로 직렬이어야 한다 — 병렬 워커가 한 파일을 편집하면 충돌한다.

이 기능의 DAG가 좁은 것은 분할을 게을리해서가 아니라 **일이 실제로 사슬형**이기 때문이다:
원시 함수 -> 그것을 쓰는 판독 -> 판독 결과를 읽는 소비. 유일하게 실재하는 병렬은 T3 // T4다.

### T1: ANSI 스트리퍼와 값 파서 — 순수 함수 [infra]
- **Fulfills**: R2.5, R2.6, R2.7
- **Depends on**: (none)
- **Files**: `crates/ai-monitor/src/telemetry.rs` (private 자유 함수)
- `strip_sgr(&str) -> String` (일반 CSI-`m`), 값 문법 `<num>[.<num>][kKmM]` 파서, 하한 1,000 거부.
  소비자 없이 단위 테스트가 가능한 유일한 조각이라 먼저 떼어 낸다.

### T2: 게이트 · 두 앵커 · 스냅샷 필드 [vertical — 생산 측 완결]
- **Fulfills**: R1, R2.1, R2.2, R2.3, R2.4, R3
- **Depends on**: T1
- **Files**: `crates/ai-monitor/src/telemetry.rs`
- `type`/`subtype`/`Context Usage` 게이트(D1a, `strip_sgr` 전 원문 검사), 전용 라인 + 헤더 두 앵커,
  `ContextWindowReading` 구조체와 `TelemetrySnapshot` 필드(D12), last-wins(D5).

### T3: 진단 카운터 2개 [vertical — 생산 측]
- **Fulfills**: R6
- **Depends on**: T2
- **Files**: `crates/ai-monitor/src/telemetry.rs`
- `context_window_parse_failures`, `context_window_anchor_conflicts`. T2가 만든 판독 경로의
  실패 분기에 붙으므로 T2 없이는 붙일 자리가 없다.
- **T4와 병렬 가능** — 다른 파일이고, T4는 이 카운터를 읽지 않는다.

### T4: 분모 우선순위와 모델 짝 술어 [vertical — 소비 측]
- **Fulfills**: R4, R5
- **Depends on**: T2
- **Files**: `crates/app/src/dashboard.rs`
- `context_pct`의 분모 캐스케이드에 판독값 삽입(D3), 모델 요구 단락(D4), 종착값(D6),
  사용 가능 술어 4칸(D9). `>100%` 주석 갱신(G-g).
- **T3과 병렬 가능** — T2가 스냅샷 필드를 확정한 뒤부터 두 작업은 서로의 입력이 아니다.

### T5: 픽스처와 end-to-end 검증 [vertical]
- **Fulfills**: R0, R7
- **Depends on**: T3, T4
- **Files**: `crates/ai-monitor/tests/fixtures/context_command.jsonl`,
  `crates/ai-monitor/tests/telemetry.rs`, `crates/app/src/dashboard.rs` 테스트 모듈
- 새 픽스처(D13 — `\u001b` 이스케이프, 리터럴 `"isAsync": false` 회피), 기존 `session.jsonl`
  스냅샷 불변 확인, R0.1-R0.3의 end-to-end 단언.

### DAG

```
T1 --> T2 --+--> T3 --+
            |         +--> T5
            +--> T4 --+

T3 // T4 (병렬)
```

## External Dependencies

### Pre-work
- (none) — 외부 서비스도, 새 크레이트도, 계정도 필요하지 않다. 입력은 이미 디스크에 있는
  `~/.claude/projects/**/*.jsonl`뿐이다.

### Post-work
- **`cargo mutants -f crates/ai-monitor/src/telemetry.rs -j 4` 와
  `cargo mutants -f crates/app/src/dashboard.rs -j 4` 를 돌려 생존자 수를 기록한다.**
  CLAUDE.md가 커버리지를 테스트 개수가 아니라 생존자 수로 판정하고, 이 기능의 분기
  (게이트 · 술어 4칸 · 카운터 2조건)는 특히 생존자가 나오기 쉬운 모양이다.
  비율이 아니라 **절대 생존자 수**를 적을 것.
- `docs/architecture.md`의 갭 목록에 **G-i**(게이트가 패널 타이틀에 단일 의존)를 옮길지 판단한다.
  스펙 로컬로 둘 수도 있으나, 외부 도구의 출력 포맷에 의존한다는 점에서 G5의 A1/A2와 같은 종류다.
