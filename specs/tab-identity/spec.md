# Spec: tab-identity

## Meta
- **Created**: 2026-07-24
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-24

## Goal
mac-terminal에서 여러 세션을 동시에 돌릴 때, 각 탭이 "지금 무슨 작업을 하는
세션인지"를 직관적으로 식별할 수 있도록 탭 UX(세션 정체성)를 개선한다.

## Non-goals
- 주목 라우팅(어떤 탭을 봐야 하는가) 재구현 — 기존 waiting area가 담당.
- 탭 다수 시 색상·그룹핑·검색 등 대안 경로 — 이번 스코프 밖(미탐색으로 남김).
- 수동 별명(사용자가 직접 붙이는 탭 이름)을 1차 수단으로 삼기 — 자동 유도가 우선.

## Confirmed Goal
각 탭이 세션의 "현재 AI 작업"을 반영하는 짧은 라벨을 실시간으로 갱신해 보여주고,
보조 정체 신호로 레포/폴더명을 함께 노출한다. 여러 탭이 떠 있을 때 각 탭 라벨만으로
그 세션의 현재 작업을 (수동 이름 붙이기 없이) 식별할 수 있으면 완료다.

## Research

### 탭/세션 모델 (app이 소유)
- 탭 제목은 현재 `cwd().file_name()`에서 **실시간 파생**, 없으면 `"Tab {n}"` (`crates/app/src/tabs.rs:162`, `titles()` `:140`).
- 제목 갱신은 탭 열기/닫기/전환 시에만 발생 — **프레임마다가 아님** (`crates/app/src/terminal_app.rs:521` `sync_tab_model()`).
- `Session`에 미사용 명시적 override 훅 존재: `title()`/`set_title()` (`crates/app/src/session.rs:136`,`:143`, `#[allow(dead_code)]`).
- `Session::cwd()`는 자식 pid → `proc_pidinfo`로 라이브 cwd 해석 (`crates/app/src/session.rs:129`).

### 에이전트 감지 + 메타데이터 (off-thread 워커)
- `TabMeta { branch, agent }` / `Agent { kind, state }` (`crates/app/src/tabmeta/mod.rs:40`,`:48`).
- `AgentState { Busy, Waiting, Idle, Unknown }` — Done 상태 없음 (`tabmeta/mod.rs:60`).
- `AgentKind { Claude, Antigravity, Codex, Gemini, Aider }` (`tabmeta/mod.rs:79`).
- pid 기반 감지: `agent_of_pid()` (`tabmeta/agent.rs:182`), 워커 틱마다 파생 (`tabmeta/worker.rs:81`).
- 워커: `TICK=500ms`, 백그라운드 탭 스태거(`BG_INTERVAL=4`), 로그 경로 재해석 `RESOLVE_TTL≈10s`, `(mtime,len)` 메모이즈 → 변화 없으면 stat 1회 (`tabmeta/worker.rs:27`, `tabmeta/activity.rs:28`,`:170`).
- 변화 시에만 generation 증가 후 UI wake: `publish_if_changed` (`tabmeta/worker.rs:221`).

### "현재 AI 작업" 텍스트 소스 — 이미 존재하나 버려짐
- `ai-monitor` 크레이트가 `~/.claude/projects/**/*.jsonl`을 읽음. cwd → `encode_cwd` → `<root>/<encoded>/` 최신 jsonl (`crates/ai-monitor/src/lib.rs:29`,`:45`).
- **작업 텍스트 추출이 이미 구현됨**: `event_summary()`가 `message.content[].text`를 뽑고(`ai-monitor/src/lib.rs:95`), `one_line()`이 공백 정리 후 **200자 캡** (`lib.rs:117`).
- 그러나 per-tab 경로(`tabmeta`)의 tail 분류기는 JSONL을 읽고 **role/LastMessage만 남기고 작업 텍스트를 버림** (`ai-monitor/src/activity.rs:90`, `tail_state` `:54`). ← 여기서 텍스트를 살리면 "현재 작업" 라벨의 소스가 됨.
- 툴 호출 이름/인자는 현재 미추출(`type=="tool_use"`만, 이름 없음).
- 로그↔탭 매핑은 오직 cwd 기반. **같은 cwd의 두 탭은 로그를 공유** — 세션 단위 구분 없음 (`tabmeta/mod.rs:103` 주석).

### 사이드바 렌더링 (renderer, GridSnapshot 아님)
- `SidebarRow { tab_id, title }`; 카드 루프가 제목을 그림 `push_glyphs(&rows[i].title,…)` (`crates/renderer/src/sidebar_row.rs:44`,`:1079`).
- 2줄 카드: 1줄=선행 마크(에이전트/ready/waiting) + 제목, 2줄=git 브랜치 (`sidebar_row.rs:1057`,`:1094`). 폭 초과 시 `truncate_to_width()` 말줄임 (`:261`).
- 탭 상태는 `GridSnapshot`이 아니라 **별도 버스** `TabModelHandle::set()`로 전달 (`sidebar_row.rs:329`). 그리드 스냅샷은 터미널 격자만.
- per-tab 메타는 `MetaSource` pull 트레이트로 프레임마다 당김, generation 변화 시에만 갱신 (`sidebar_row.rs:114`,`:830`; app측 `SidebarMetaSource` `crates/app/src/sidebar_model.rs:22`).

### waiting area (별개 축 — 주목 라우팅)
- `WaitingKind { Error, Waiting, Completion }`, `WaitingState` (`crates/app/src/waiting.rs:40`,`:103`).
- 사이드바 상단 고정 밴드(≤5행 + "+N more") (`sidebar_row.rs:945`). rising-edge Waiting 감지 `feed_agent_states()` (`terminal_app.rs:587`).
- 정체(identity)와 직교 — 이번 스코프는 카드 라벨이지 이 밴드가 아님.

### 아키텍처 함의
- 이 기능은 전부 **app + renderer**에서 처리 가능 — `term-core`(GPU-free)는 손대지 않음 → dependency-direction 규칙 준수.
- 자연스러운 경로: tabmeta 워커의 per-tab 활동 파생에 "현재 작업" 짧은 요약 문자열을 추가(ai-monitor `event_summary`/`one_line` 재사용) → `TabMeta`/`Agent`에 실어 generation 스냅샷으로 발행 → 사이드바 카드에 렌더.
- 빌드/테스트: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

## Decisions

### D1: 라벨 내용 = 세션 Claude 로그의 "최근 사용자 프롬프트"
- **Status**: resolved
- **Rationale**: 기각의 1차 근거는 **인터뷰 증거** — 사용자는 세션을 "내가 시킨 작업"으로 식별한다(1순위 단서). 부차적으로, 최근 사용자 프롬프트는 한 작업 안에서는 안정적 앵커이면서 새 프롬프트가 오면 최신화되어 "현재 활동으로 갱신"과 "고정 앵커 부재로 인한 혼란" 사이의 긴장을 해소한다. 기각: (a) 최근 어시스턴트 요약 — 위 인터뷰 증거에 반함(사용자가 원한 신호가 아님)이 주된 이유이고, 이벤트마다 요동(flicker)은 부차적(요동 자체는 D10류 평활화로 완화 가능하므로 결정적 근거가 아님); (b) 현재 툴 호출 — 툴 이름이 현재 미추출(추가 파싱 필요)이고 너무 저수준·노이즈. 소스는 `ai-monitor`가 이미 읽는 JSONL의 user 턴 텍스트(`event_summary`, `lib.rs:95`)이며, 현재 `tabmeta` 경로가 버리는 값을 살린다.

### D2: 배치 = 카드 1줄(제목=cwd)을 "현재 작업"으로 대체, 레포/폴더는 2줄로 병합
- **Status**: resolved
- **Rationale**: 작업이 1순위 식별 단서이므로 가장 눈에 띄는 1줄을 차지한다. 카드는 2줄 유지(높이 불변). 기각: (a) 3번째 줄 추가 — 탭이 많을 때 세로 밀도·높이 증가; (b) cwd를 1줄에 유지 — 1순위 신호(작업)를 묻어버림. 레포/폴더명은 2순위 단서로 2줄(git 브랜치 줄)에 병합 표기.

### D3: 유휴/폴백 = 에이전트 유휴 시 마지막 작업 라벨 유지, 순수 셸만 cwd
- **Status**: resolved
- **Rationale**: 정체성은 작업이 끝난 뒤에도 유지돼야 한다 — "어느 탭이 X를 했지?"를 훑는 바로 그 순간 cwd로 되돌아가면 정체성이 지워진다. `AgentState::Idle`이어도 마지막으로 확정된 사용자 프롬프트 라벨을 잔상으로 유지. 에이전트가 아예 감지되지 않는 순수 셸 탭만 1줄=cwd(현행 동작). 기각: (a) 유휴 즉시 cwd — 완료 직후 정체성 소실; (b) dim 처리 — 렌더링 복잡도 대비 이득 적음, 향후 재고.

### D4: 같은 cwd 두 세션 = v1 한계 수용, known gap로 문서화
- **Status**: resolved
- **Rationale**: 세션 단위 정밀 구분은 PTY↔Claude sessionId 상관을 요구(각 탭의 세션을 특정 jsonl에 매핑)하는 더 큰 변경이다. 같은 폴더에서 두 에이전트를 동시에 돌리는 경우는 드물고 v1은 출시를 우선한다. **정확한 실패 모드**: 로그 해석이 encoded-cwd 디렉터리에서 newest-mtime jsonl을 고르므로(`lib.rs:56`), 두 세션이 번갈아 append하면 두 탭 모두 "가장 최근 활동한 세션"의 프롬프트를 따라가 라벨이 세션 사이를 **오간다**(정적 공유보다 나쁨) — 이 점을 known gap에 정직히 명시. 기각: (a) sessionId 정밀 매핑 — 작업량 큼, 연기; (b) 조용히 스코프 밖 — 최소한 문서화가 낫다.

### D5: 갱신 주기 = 기존 tabmeta 워커 재사용 (신규 폴링/스레드 없음)
- **Status**: assumed
- **Rationale**: `tabmeta/worker.rs`의 틱(`TICK=500ms`, 백그라운드 스태거, `(mtime,len)` 메모이즈)이 이미 per-tab 로그를 감시한다. 라벨 파생을 이 경로에 얹으면 idle 시 추가 read 없음(변화 없으면 stat 1회), 변화 시에만 generation 증가 후 UI wake. 새 폴링 스레드 추가는 성능·복잡도상 기각(L1: `worker.rs:27`,`:221`).

### D6: 라벨 저장 = TabMeta의 신규 필드 + 마지막 값 캐싱
- **Status**: assumed
- **Rationale**: 라벨 문자열을 `TabMeta`(또는 `Agent`)에 `task: Option<String>` 형태로 실어 generation 스냅샷으로 렌더러에 전달(`MetaSource` pull 재사용). D3(유휴 잔상)를 위해 마지막으로 확정된 라벨을 per-tab 메모에 캐싱 — 8KiB tail에 사용자 프롬프트가 더 없어도 잔상 유지. 별도 버스/스키마 신설은 불필요(기존 `TabMetaSnapshot` 확장).

### D7: 비-Claude 에이전트 = 작업 라벨 없음, cwd 폴백 (에이전트 아이콘은 유지)
- **Status**: assumed
- **Rationale**: L1: Claude만 JSONL 로그 소스를 가진다. Codex/Gemini/Aider/Antigravity는 작업 텍스트 소스가 없으므로 1줄=cwd 폴백(순수 셸과 동일), 단 기존 에이전트 감지에 의한 아이콘/skin은 그대로. 소스 없는 라벨을 지어내지 않는다.

### D8: 절단/정규화 = one_line + 카드 폭 truncate, tool-result 제외한 user 턴 선택
- **Status**: assumed
- **Rationale**: 소스 텍스트는 `event_summary`로 뽑고 `one_line`(200자 캡, 공백 정리) 후 카드 1줄 폭에 `truncate_to_width`로 말줄임(L1: `lib.rs:95`,`:117`; `sidebar_row.rs:261`). "최근 사용자 프롬프트"는 `classify_user`로 tool-result 턴을 제외한 실제 사람 프롬프트를 고른다(L1: `activity.rs:140`). **추출 실패 처리**: JSONL 파손/torn-tail, 빈·공백뿐인 `event_summary`, 텍스트 없는 user 턴(이미지/붙여넣기·tool-result만)은 "실질 프롬프트 없음"으로 취급 → D10 폴백(직전 실질 프롬프트) → 없으면 D6 캐시 라벨 → 그것도 없으면 D3 cwd 폴백.

### D9: 2줄 레이아웃 = 레포/폴더명 + git 브랜치 병합 표기
- **Status**: assumed
- **Rationale**: 1줄이 작업으로 대체되므로 2순위 신호인 레포/폴더명을 2줄(기존 브랜치 줄)에 병합. 폭 초과 시 브랜치보다 레포명 우선 노출(정체성 우선). 정확한 아이콘/구분자 배치는 렌더 디테일로 구현 시 확정.

### D10: 모호 프롬프트 = 짧은/연속 프롬프트는 건너뛰고 직전 "실질" 프롬프트로 폴백
- **Status**: resolved
- **Rationale**: (Inversion Probe에서 도출) 최근 사용자 프롬프트가 "계속"·"y"·"네"·"고쳐줘"처럼 임계 길이 미만이거나 연속-지시 패턴이면, tail을 거슬러 마지막으로 의미 있던(임계 이상) 사용자 프롬프트를 라벨로 채택한다. 여러 탭이 "계속"으로 몰려 정체성이 붕괴하는 실패 모드를 방지. **판정 형태(정의됨)**: v1은 (i) 길이 임계(초기값 <8자)와 (ii) 연속어 사전(예: 계속/continue/y/네/응/다시/go) 두 가지만 사용 — 그 외 패턴 감지(반복·유사도)는 미포함. 임계값·사전 내용은 구현 중 튜닝. 기각: (a) 그대로 표시 — 정체성 붕괴; (b) 항상 레포/브랜치로 보강 — 작업 신호가 약화됨(보강은 폭 여유 시 부차적).

## Constraints
- `term-core`는 GPU-free 유지 — 모든 변경은 app + renderer에 한정 (dependency-direction 규칙).
- waiting area 로직·동작 불변 — 정체성과 직교한 별개 축.
- 카드는 2줄 유지 — 높이 불변.
- 신규 폴링 스레드/비동기 런타임 금지 — 기존 tabmeta 워커 + generation 스냅샷 재사용 (D5).
- idle(로그 무변화) 시 추가 파일 read 없음 — `(mtime,len)` 메모이즈 유지, stat 1회.
- 프로젝트 규칙 준수: no-gui-crates · macos-only · zsh-local-only.

## Known Gaps
- 같은 cwd에서 두 Claude 세션 → 두 탭이 newest-mtime 로그를 공유해 라벨이 "가장 최근 활동한 세션" 프롬프트로 **오감**(정적 공유보다 나쁨) (D4, v1 미해결 — sessionId 정밀 매핑은 향후).
- 비-Claude 에이전트(Codex/Gemini/Aider/Antigravity)는 작업 라벨 없음 — cwd 폴백 (D7, 로그 소스 부재).
- 툴 호출 이름 기반 "지금 하는 동작" 라벨 미구현 (D1 기각지 — 향후 파싱 확장 여지).
- 성공의 정량 기준(예: N개 탭에서 목표 탭을 X초 내 식별) 미확정 — 현재는 정성 목표(라벨만으로 구분 가능)로만 판정.
- 모호-프롬프트 임계값·연속어 사전 초기값은 구현 중 튜닝 필요 (D10).

## Requirements

### R0: 각 탭 카드가 세션의 현재 작업을 반영하는 라이브 정체성 라벨을 표시한다

#### R0.1: 여러 활성 세션이 서로 다른 라벨로 구분됨
- **Given**: 서로 다른 작업을 하는 두 Claude 세션 탭이 열려 있다
- **When**: 사용자가 사이드바를 본다
- **Then**: 각 카드 1줄에 해당 세션의 사용자 프롬프트에서 파생된 서로 다른 라벨이 보인다

### R1: 라벨 내용 파생 (D1, D8, D10)

#### R1.1: 최근 사용자 프롬프트를 라벨로 파생 (producer)
- **Given**: 세션 로그의 최신 사용자 프롬프트가 "auth.rs 리팩터링해줘"
- **When**: tabmeta 워커가 해당 탭 로그를 파싱
- **Then**: 그 탭의 `task`가 "auth.rs 리팩터링해줘"(one_line·정규화 적용)로 설정됨

#### R1.2: tool-result user 턴 제외 (producer)
- **Given**: 로그 tail의 최신 user 턴이 `toolUseResult`(사람 프롬프트 아님)
- **When**: 워커가 "최근 사용자 프롬프트"를 선택
- **Then**: tool-result 턴은 건너뛰고 마지막 실제 사람 프롬프트를 라벨로 사용

#### R1.3: 짧은/연속 프롬프트 폴백 (producer, D10)
- **Given**: 최신 사용자 프롬프트가 "계속"(<8자 또는 연속어 사전에 해당)
- **When**: 워커가 라벨을 파생
- **Then**: tail을 거슬러 직전의 실질(≥임계) 프롬프트를 라벨로 채택

#### R1.4: 추출 실패 폴백 체인 (producer, D8)
- **Given**: JSONL 파손·torn-tail 또는 빈/공백뿐인 텍스트로 실질 프롬프트를 못 뽑음
- **When**: 워커가 라벨을 파생
- **Then**: 캐시된 마지막 라벨 → 없으면 cwd 순으로 폴백(패닉·빈 카드 없음)

#### R1.5: 라벨을 카드 폭에 맞춰 말줄임 (consumer)
- **Given**: `task`가 카드 1줄 폭보다 긴 문자열
- **When**: 렌더러가 카드를 그림
- **Then**: `truncate_to_width`로 …말줄임해 1줄에 그림

### R2: 카드 레이아웃 재배치 (D2, D9)

#### R2.1: 1줄이 작업 라벨을 표시 (consumer)
- **Given**: 에이전트가 감지된 탭의 `task`가 존재
- **When**: 렌더러가 카드 1줄을 그림
- **Then**: cwd 대신 작업 라벨이 1줄(굵게)에 표시됨

#### R2.2: 2줄이 레포/폴더명 + git 브랜치를 병합 표시 (consumer)
- **Given**: 탭의 레포/폴더명과 git 브랜치가 존재
- **When**: 렌더러가 카드 2줄을 그림
- **Then**: 레포/폴더명과 브랜치가 2줄에 병합 표기, 폭 초과 시 레포명을 우선 노출

#### R2.3: 카드 높이 불변 (consumer)
- **Given**: 작업 라벨 기능이 적용된 카드
- **When**: 렌더러가 카드를 그림
- **Then**: 카드는 2줄 유지 — 스트라이드/높이가 기존과 동일

### R3: 유휴·폴백 동작 (D3, D7)

#### R3.1: 유휴 시 마지막 작업 유지 (consumer)
- **Given**: 에이전트가 작업을 마쳐 `AgentState::Idle`
- **When**: 렌더러가 카드를 그림
- **Then**: cwd로 되돌리지 않고 마지막 확정 라벨을 그대로 유지

#### R3.2: 순수 셸 탭은 cwd (consumer)
- **Given**: 에이전트가 감지되지 않는 탭
- **When**: 렌더러가 카드 1줄을 그림
- **Then**: 1줄=cwd 파일명(현행 동작), 작업 라벨 없음

#### R3.3: 비-Claude 에이전트는 cwd 폴백 + 아이콘 유지 (producer+consumer)
- **Given**: Codex/Gemini 등 로그 소스 없는 에이전트 탭
- **When**: 렌더러가 카드를 그림
- **Then**: 1줄=cwd, 작업 라벨 없음, 단 에이전트 아이콘/skin은 표시

### R4: 갱신 파이프라인 (D5, D6)

#### R4.1: 기존 워커 틱에서 라벨 파생 (producer)
- **Given**: tabmeta 워커가 500ms 틱으로 로그를 감시
- **When**: 로그 파일의 `(mtime,len)`이 변함
- **Then**: 라벨을 재파생하고 변화가 있을 때만 generation을 올려 UI를 wake

#### R4.2: 무변화 시 추가 read 없음 (producer)
- **Given**: 로그 파일이 지난 틱 이후 변하지 않음
- **When**: 워커 틱이 돎
- **Then**: stat 1회만 수행, 파일 본문 read·재파생 없음(idle 비용 유지)

#### R4.3: TabMeta에 task 필드 + 마지막 값 캐싱 (producer)
- **Given**: 마지막으로 확정된 라벨이 존재하고, 8KiB tail에 더는 사용자 프롬프트가 없음
- **When**: 워커가 라벨을 파생
- **Then**: per-tab 캐시가 그 라벨을 유지해 잔상이 사라지지 않음

#### R4.4: 스냅샷으로 렌더러에 전달 (boundary)
- **Given**: 새 라벨이 확정돼 generation이 오름
- **When**: 렌더러가 `MetaSource`로 per-tab 메타를 당김
- **Then**: 카드가 새 라벨을 반영해 다시 그려짐(GridSnapshot이 아닌 TabModel/메타 버스 경유)

#### R4.5: term-core 불변 (constraint)
- **Given**: 라벨 기능 전체
- **When**: 워크스페이스가 컴파일됨
- **Then**: 변경은 app+renderer에만 존재하고 term-core는 라벨 로직·wgpu/winit를 참조하지 않음

### R5: 같은 cwd 한계 (D4)

#### R5.1: 같은 cwd 두 세션은 newest-mtime 로그를 공유 (known limitation)
- **Given**: 같은 폴더에서 두 Claude 세션 탭이 동시에 실행
- **When**: 두 세션이 번갈아 로그에 append
- **Then**: 두 탭 모두 가장 최근 활동한 세션의 프롬프트 라벨을 표시(v1 알려진 한계, 패닉·크래시 없음)

## Tasks

### T1: 메타데이터 계약에 task 라벨 필드 플럼빙 [infra]
- **Fulfills**: R4.4, R4.5
- **Depends on**: (none)
- 범위: `crates/app/src/tabmeta/mod.rs`의 `TabMeta`(또는 `Agent`)에 `task: Option<String>` 추가, `TabMetaSnapshot` 확장, `crates/renderer/src/sidebar_row.rs`의 `RowMeta`에 필드 추가, `crates/app/src/sidebar_model.rs`의 `MetaSource::fill()`이 값을 전달. 이 단계에선 값은 항상 `None`(파생·렌더 없음). term-core는 손대지 않음.

### T2: tabmeta 워커의 라벨 파생 (producer) [internal service]
- **Fulfills**: R1.1, R1.2, R1.3, R1.4, R4.1, R4.2, R4.3, R5.1
- **Depends on**: T1
- 범위: `crates/ai-monitor/src`에서 user 턴 텍스트를 노출(현재 버려지는 값 — `activity.rs`/`lib.rs`), `crates/app/src/tabmeta/activity.rs`·`worker.rs`가 최근 실질 사용자 프롬프트를 뽑아 `one_line` 정규화 후 `TabMeta.task`에 설정. tool-result 제외(`classify_user`), 짧은/연속 프롬프트 폴백(D10 사전+길이), 추출 실패 폴백 체인, per-tab 마지막 라벨 캐싱. 기존 틱·`(mtime,len)` 메모이즈·generation 발행에 편승(신규 스레드 없음). 유닛 테스트: JSONL 픽스처 → 기대 라벨.

### T3: 사이드바 카드에 라벨 렌더 (consumer) [vertical]
- **Fulfills**: R0.1, R1.5, R2.1, R2.2, R2.3, R3.1, R3.2, R3.3
- **Depends on**: T1, T2
- 범위: `crates/renderer/src/sidebar_row.rs` 카드 그리기 — 1줄=`task`(있으면) 또는 cwd 폴백, 2줄=레포/폴더명+브랜치 병합(폭 초과 시 레포 우선), `truncate_to_width` 적용, 카드 2줄·스트라이드·높이 불변. 유휴 시 마지막 라벨 유지(값이 남아 있으므로 자연 유지), 순수 셸/비-Claude는 cwd. R0.1(두 탭 서로 다른 라벨)은 T2+T3 통합으로 관찰되며 execute 파이프라인의 holistic 검증에서 확인.

## External Dependencies

### Pre-work
- (none) — 전부 기존 인프라(ai-monitor 로그 리더, tabmeta 워커, MetaSource 버스) 재사용.

### Post-work
- `/run-term`로 실제 렌더 육안 확인(여러 탭에서 라벨 구분·유휴 잔상·폭 절단).
- 같은 cwd 한계(R5.1/D4)를 사용자 문서/릴리스 노트에 known gap로 명시(선택).
