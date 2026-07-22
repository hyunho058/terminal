# Spec: left-sidebar-waiting-area

## Meta
- **Created**: 2026-07-21
- **Type**: dev
- **Status**: approved
- **Source**: requirements-interview-outputs/left-sidebar-awareness/insights.md
- **Approved by**: user
- **Approved at**: 2026-07-21

## Goal
좌측 사이드바를 "상황 인지판"으로 만들기 위해 최상단에 **waiting 영역**을 추가한다.
여러 세션 중 지금 주목이 필요한 세션(멈춘 에이전트 / 백그라운드 명령 완료 / 에러)을
그 영역에 모아 보여주고, 해당 세션을 클릭/이동하면 확인 처리되어 사라진다.

## Non-goals
- 사이드바를 탭 스위처/런처로 재정의하는 것 (인지판 역할에 집중)
- 확인 없는 자동 소멸(타임아웃으로 사라지기)
- zsh 외 다른 셸 지원, 원격/SSH 세션 (프로젝트 제약: macos-only, zsh-local-only)
- 경과 타이머·에러 문구 조각 등 waiting 한 행의 부가 정보 (v1에서 버리기로 결정)
- 신호 종류의 시각 인코딩 최종 확정 이외의 사이드바 전면 리디자인

## Confirmed Goal
기존 시간순 세션 목록과 **물리적으로 분리된** 별도 waiting 영역을 사이드바 최상단에
pinned로 둔다. 세 신호 소스(에이전트 멈춤 = `~/.claude/projects/**/*.jsonl` 파일
이벤트 감시; 백그라운드 완료·에러 = 셸 통합 OSC 133/exit-code)가 세션을 waiting
영역으로 밀어 올린다. waiting 행은 `[신호 종류] 세션 이름`으로, 기다린 시간순 정렬.
해당 세션으로 이동/클릭하면 항목이 제거된다(확인 기반).

**완료 기준(측정 가능):**
- 멈춘 에이전트 확인용 세션 훑기 빈도: 하루 ~20회 → ~3회
- 멈춘 에이전트는 발생 후 ≤ 5초 내 waiting 영역에 등장
- 에러 판정: exit code ∉ {0, 1, 130} 인 경우만 waiting 으로 (grep 무매치·Ctrl-C 제외)

## Research

### 사이드바 렌더링 / 레이아웃 (renderer)
- 사이드바 draw 레이어 `TabBar`/`DrawLayer::record`: `crates/renderer/src/sidebar_row.rs:748,815-1004`. 패널 배경+우측 보더 `sidebar_row.rs:872-889`, 카드 루프 `sidebar_row.rs:893-988`.
- 순수 지오메트리(단일 소스) `SidebarLayout`: `crates/renderer/src/sidebar_layout.rs:199-266` (`row_stride`,`row_rect`,`row_at`). 히트테스트 `hit_test`→`SidebarRegion::Card/Chrome/Panel`: `sidebar_layout.rs:316-344`.
- **최상단 헤더 밴드**: `SidebarChromeLayer`(z=30) `crates/renderer/src/sidebar_header.rs:66`; 헤더 높이 `header_h(scale)` `sidebar_layout.rs:122-126`, 모델에 `list_top`으로 발행 `crates/app/src/terminal_app.rs:519,544-545`. 카드 리스트는 그 아래에서 시작 → **waiting 영역을 끼울 seam**. 헤더 안에 빈 `search_slot_rect` 예약 영역 존재 `sidebar_layout.rs:176-185`.
- 기존 per-tab 인디케이터: `RowMark { None, Agent(✳), Ready(◇) }` `sidebar_row.rs:52-61`, 매핑 `crates/app/src/sidebar_model.rs:76-83`; `AgentSkin` 카드 스킨 `sidebar_theme.rs:192-268`; hover/selected `CardState` `sidebar_row.rs:404-441`. **waiting 영역/카운트 배지는 없음.**

### 탭/세션 모델 (app)
- `Session`(탭 1개 = PTY+Terminal+reader): `crates/app/src/session.rs:23-31`. 안정 id `TabId(u64)` `session.rs:19-20`.
- `Tabs { sessions: Vec<Session>, active, next_id }`: `crates/app/src/tabs.rs:17-24`. **정렬 = open 순서(시간순), 재정렬/sort 없음** `tabs.rs:80-86`. 렌더러 모델 `TabBarModel { titles: Vec<SidebarRow>, active }` `sidebar_row.rs:95-102`, 타이틀 = cwd basename `tabs.rs:140-174`.

### 에이전트 감지 (ai-monitor + app/tabmeta)
- 로그 위치/파싱: `crates/ai-monitor/src/lib.rs:37-72` (`~/.claude/projects/<encoded-cwd>/*.jsonl`), 8KiB tail `crates/ai-monitor/src/activity.rs:30-51`.
- **사이드바 경로는 폴링**(파일 감시 아님): `crates/app/src/tabmeta/worker.rs` — `TICK=500ms`(active), 백그라운드 ~2s stagger `worker.rs:27-37`. 상태 규칙 `state_from` `crates/app/src/tabmeta/activity.rs:37-46`.
- 상태 enum `AgentState { Busy, Idle, Unknown }` `crates/app/src/tabmeta/mod.rs:60-64`. **명시적 `Waiting`/`Done` 변형 없음** — "입력 대기"는 현재 `role=="user"` 절로 `Busy`에 흡수(`activity.rs:42-43`), 종료 에이전트는 `agent: None`.
- notify 기반 `LogWatcher` 이미 존재(`ai-monitor/src/lib.rs:144-218`)하나 Cmd+Shift+M 오버레이 전용. `notify` 의존성은 `common`/`ai-monitor` Cargo.toml에 이미 있음.
- 사이드바 데이터 플로: **GridSnapshot 아님** — 앱 소유 `SidebarModel`(rows+active) + pull 기반 `MetaSource`/`TabMetaHandle`(branch+agent), generation-gated `sidebar_row.rs:85-90`, `crates/app/src/sidebar_model.rs:22-57`. 재드로우는 `OverlayPending`/`TabMetaPump` `mod.rs:235-265`.

### vte 파서 / PTY (term-core) — 셸 통합 착지점
- vte `Perform` 구현 `crates/term-core/src/parser.rs:242`. **`print`/`execute`/`esc_dispatch`/`csi_dispatch`만 구현**, `osc_dispatch` 미구현 → **OSC 133 현재 무시됨**. OSC 133 = `parser.rs:242`에 `fn osc_dispatch` 추가.
- zsh spawn: `SHELL="/bin/zsh"` `crates/term-core/src/pty.rs:19`, 바 `/bin/zsh`(인자 없음, rc 주입 없음) `pty.rs:112-120` = **ZDOTDIR/env 주입 choke point**. reader 스레드/session당 → `UserEvent::PtyOutput(TabId)` `crates/app/src/terminal_app.rs:691` → `Session::drain`→`terminal.advance` `session.rs:82-88`.
- **세션당 PTY+파서+grid 독립**, `TabId`로 keying `session.rs:23`, `tabs.rs:17-24`.

### 빌드/테스트/린트
- `cargo build` / `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` (`CLAUDE.md:36-37`). insta 스냅샷 `crates/term-core/tests/grid_snapshot.rs` (GPU-free vte→grid 테스트 exemplar).

## Decisions

### D1: waiting 영역 = 최상단 별도 pinned 존, 3신호 표시
- **Status**: assumed (from insights)
- **Rationale**: 사이드바 정체성이 "상황 인지판"이고 핵심은 주목 필요 세션을 놓치지 않는 것. 기존 시간순 카드 리스트를 재정렬하는 대신(스캔 습관을 못 꺾음) 물리적으로 분리된 최상단 밴드에 모은다.

### D2: waiting 정렬 = 트리거 이벤트 시각 오름차순(가장 오래 기다린 항목 최상단)
- **Status**: resolved (reviewer #6 반영)
- **Rationale**: 종류 고정 서열(에러>대기>완료) 정렬은 사용자가 거부 — 오래 기다린 것이 먼저 눈에 와야 함. 종류는 아이콘/색으로 *보여주기만*. **공통 정렬 기준(wait-start) 정의**: 완료/에러 = OSC 133 D(명령 종료) 이벤트의 벽시계 시각; 에이전트 대기 = 로그에서 `end_turn`이 관측된 시각(폴링 양자화 ≤5s 허용). 두 소스 모두 "트리거 이벤트 시각"으로 정규화해 오름차순 정렬.

### D3: waiting 한 행 = [신호 아이콘] + 세션 이름 (타이머·문구 조각 생략)
- **Status**: assumed (from insights)
- **Rationale**: 좁은 행 우선순위 1=신호 종류, 2=세션 이름. 경과 타이머·에러 문구 조각은 버려도 됨(사용자 확정). 클릭하면 세션에서 전체 맥락 확인.

### D4: "멈춘 에이전트" = 새 Waiting 상태(입력 대기만), Busy(작업중) 제외
- **Status**: resolved (round 1, A; feasibility spike 확정)
- **Rationale**: insights 핵심이 "내 입력을 기다리며 멈춘" 세션. 작업중(Busy)까지 올리면 노이즈. 현재 `AgentState { Busy, Idle, Unknown }`는 입력 대기를 Busy에 흡수(`activity.rs:42-43`)하므로 **`Waiting` 변형을 신설**하고 감지 규칙을 분리. **실현가능성 검증(실제 jsonl 스파이크)**: 판별 마커 존재 — 마지막 assistant 이벤트의 `stop_reason=="end_turn"`(턴을 사람에게 넘김)이 "내 입력 대기"를 정밀 식별, `tool_use`(작업중)·`user` tool_result(작업중)·human_prompt(사람이 방금 입력)와 구분됨. 8KiB tail로 파싱 가능. 정밀 규칙은 D16. 대안 기각: 기존 Busy 재사용(작업중 세션까지 떠 신뢰 저하), role==user 흡수 유지(tool_result와 실제 입력 혼동).

### D5: "완료" 신호 = ≥30초 실행된 포그라운드 명령의 종료, `&` 잡·짧은 명령 제외
- **Status**: resolved (round 1, B; 임계값 30초 조정 가능)
- **Rationale**: "자리를 비운 긴 명령이 끝남"이 진짜 니즈. 모든 명령 완료는 노이즈, 진짜 `&` 백그라운드 잡은 OSC 133 프롬프트 마커로 감지 불가. 포그라운드 명령의 start→end 시간차 ≥ 임계값일 때만 완료 알림. 대안 기각: 모든 명령(노이즈), `&` 잡만(감지 불가).

### D6: 시각 인코딩 = 신호별 색 + 앞머리 아이콘(⏸ 대기 / ✓ 완료 / ✕ 에러)
- **Status**: resolved (round 1, C; reviewer #5 반영 — "재사용" → "확장")
- **Rationale**: 좁은 행에서 색+아이콘이 가장 빠른 판별. 기존 `RowMark`/`Icon`/`sidebar_theme` 인프라(`sidebar_row.rs:52`, `sidebar_icons.rs`, `sidebar_theme.rs:46-48`)를 **확장**: 현재 `RowMark{None,Agent(✳),Ready(◇)}`·`Icon{Asterisk,Diamond,GitBranch}`에 waiting 3신호용 신규 변형(Pause/Check/Cross) 추가, `AgentState→RowMark` 매핑(`sidebar_model.rs:76-83`)에 `Waiting` arm 추가(비-`_` exhaustive match 갱신). 대안 기각: 색만(색맹 접근성·구분 약함), 텍스트 라벨(공간 차지).

### D7: "에러" = 방금 끝난 포그라운드 명령 exit code ∉ {0, 1, 130}
- **Status**: assumed (from insights)
- **Rationale**: exit≠0 그대로면 grep 무매치(1)·Ctrl-C(130)로 오탐 → waiting 신뢰 붕괴. {1,130} 무시목록으로 1차 방어, 나머지 non-zero만 에러로.

### D8: 감지 경로 이원화 — 에이전트=jsonl 폴링(기존), 완료/에러=OSC 133 셸 통합
- **Status**: assumed (L1 + insights)
- **Rationale**: 에이전트 대기는 기존 tabmeta 폴링(active 500ms / bg ~2s, `worker.rs:27-37`)이 이미 ≤5초 신선도 예산 충족 → notify 전환 불필요(있으면 나중에). 완료/에러는 PTY 바이트만으로 불가하므로 OSC 133 셸 통합을 v1에 포함.

### D9: 렌더 위치 = 헤더 밴드와 카드 리스트 사이 waiting 밴드 신설
- **Status**: assumed (L1)
- **Rationale**: `SidebarLayout`의 `list_top`(헤더 높이) 아래에서 카드가 시작(`sidebar_layout.rs:122-126`, `terminal_app.rs:519`). waiting 밴드를 헤더와 카드 리스트 사이에 삽입하고 `list_top`을 그만큼 밀어 seam 재사용.

### D10: OSC 133 파싱 = Performer::osc_dispatch 추가; 셸 통합 = ZDOTDIR 주입 zshrc
- **Status**: assumed (L1)
- **Rationale**: `parser.rs:242` Performer에 `fn osc_dispatch` 신설(현재 OSC 전부 무시). zsh 주입은 `pty.rs:112-120` choke point에서 `ZDOTDIR`를 우리 생성 디렉터리로 지정 → 그 `.zshrc`가 사용자 원본 rc를 source한 뒤 `preexec`/`precmd`에서 OSC 133(A/B/C/D + exit code) emit. 대안 기각: rc 직접 수정(사용자 파일 침범), `-c` 래핑(인터랙티브 셸 훼손).

### D11: waiting 표시 상한 = 최대 5개 + "+N more" 요약
- **Status**: resolved (round 2, D; 개수 조정 가능)
- **Rationale**: 세션 폭주 시 waiting 밴드가 사이드바를 잠식하면 인지판이 오히려 노이즈. 기다린 시간순 상위 5개만 표시하고 초과분은 요약 한 줄. 대안 기각: 무제한 스크롤(밴드 비대), 제한 없음(폭주 방치).

### D12: 클리어 = 해당 세션이 활성 탭이 되는 순간 제거(클릭/키보드 무관), 새 신호 시 재등장
- **Status**: resolved (round 2, E)
- **Rationale**: "확인" = 그 세션을 실제로 보러 갔다는 것. 입력 수단(클릭 vs ⌘N)과 무관하게 활성화가 확인 기준. 자동 소멸은 non-goal. 클리어 후 같은 세션이 새 신호를 내면 다시 올라옴(재발은 새 이벤트).

### D13: 셸 통합 실패/미설치 = 우아한 저하 (에이전트 신호 유지, 완료/에러만 비활성)
- **Status**: resolved (round 2, F)
- **Rationale**: OSC 133 주입이 사용자 zsh 설정과 충돌하거나 실패해도 앱은 정상 동작해야. 로그 기반 에이전트 대기 신호는 계속 뜨고, 완료/에러 신호만 조용히 빠짐(경고 배너 없음 — 배너 자체가 노이즈). 대안 기각: PTY 휴리스틱 추정(오탐으로 신뢰 붕괴), 필수화+경고(daily-driver에서 성가심).

### D14: 완료 × 에러 충돌 = 에러 우선 (단일 항목, ✕)
- **Status**: resolved (round 2, G)
- **Rationale**: 같은 명령이 ≥30초 실행 후 exit∉{0,1,130}이면 "완료"와 "에러" 둘 다 참. 더 긴급한 에러가 이겨 하나의 ✕ 항목으로. 두 줄 표시는 중복 노이즈.

### D15: 활성 탭 세션은 waiting 항목을 만들지 않음 (Inversion Probe)
- **Status**: assumed (D12의 귀결)
- **Rationale**: 이미 보고 있는 활성 세션에서 신호가 나면 waiting에 올릴 이유가 없음(즉시 확인 상태). 신호는 비활성 세션에 대해서만 waiting 항목 생성. RowMark 매핑(`sidebar_model.rs:76-83`)도 `Waiting` 반영하도록 갱신 필요.

### D16: "입력 대기(Waiting)" 판정 규칙 (feasibility spike 기반)
- **Status**: resolved (reviewer #1)
- **Rationale**: 비활성 세션의 최신 jsonl(8KiB tail)에서 마지막 assistant/user 이벤트 판정: **마지막 assistant 이벤트의 `stop_reason=="end_turn"` → Waiting**(에이전트가 턴을 사람에게 넘김). 마지막이 `stop_reason=="tool_use"`(도구 실행중) 또는 `user`+`toolUseResult`(tool_result) → **Busy**. 마지막이 human_prompt(`user`, `toolUseResult` 없음) → **Busy**(사람이 방금 입력, 에이전트가 응답 예정). 로그 없음/미해결 → **Unknown**. 기존 `state_from`(`activity.rs:37-46`)·`tail_state`(`ai-monitor/src/activity.rs:30-51`)를 role-only에서 (role + stop_reason + toolUseResult) 판독으로 확장.

### D17: 세션당 waiting 항목 1개, 전 신호 우선순위 = 에러 > 입력대기 > 완료
- **Status**: resolved (reviewer #2)
- **Rationale**: D3(한 행=아이콘+이름)·D11(최대 5)와 정합하려면 세션당 항목은 최대 1개. 한 세션에 여러 신호 동시 발생 시 우선순위 **에러 > 입력대기 > 완료**로 최상위 하나만 그 세션 행에 표시(아이콘/색은 이긴 신호). D14(에러>완료)의 3-신호 일반화. 정렬(D2)은 이 대표 신호의 트리거 시각 기준.

### D18: ZDOTDIR 주입은 사용자 설정을 clobber하지 않고 chain
- **Status**: resolved (reviewer #3b)
- **Rationale**: 주입 zshrc는 원래 `$ZDOTDIR`(없으면 `$HOME`)를 먼저 캡처→우리 디렉터리로 `ZDOTDIR` 설정→우리 `.zshrc`가 사용자의 `.zshenv`/`.zshrc`/`.zlogin`을 명시적으로 source한 뒤 그 이후에 OSC 133 훅(preexec/precmd)만 추가. oh-my-zsh·starship 등 기존 프롬프트와 공존(우리 훅은 add-only, 프롬프트 문자열 미변경). 실패 시 D13(우아한 저하).

### D19: 명령 실행시간은 셸이 측정해 OSC로 emit (앱 시계 비의존)
- **Status**: resolved (reviewer #3c)
- **Rationale**: ≥30초 임계(D5) 판정의 시계 문제(랩톱 슬립 중 wall-clock 점프 → 오탐)를 피하기 위해, preexec에서 `EPOCHREALTIME` 기록→precmd에서 차이를 계산해 **경과초를 OSC 133 payload에 실어 emit**. 앱은 이 값을 읽어 임계만 적용(앱측 monotonic 계산 불필요). 대안 기각: 앱측 `Instant` 차이(슬립 처리 플랫폼차·마커 유실 시 취약).

### D20: osc_dispatch는 미지/부분 OSC 133을 안전하게 무시 (no panic, no unwrap)
- **Status**: resolved (reviewer #3a; 프로젝트 no-unwrap 규약)
- **Rationale**: `Performer::osc_dispatch`는 인식 못 하는 OSC 종류/불완전 param을 조용히 드롭(파서 상태 오염·패닉 금지). exit code·경과초 파싱 실패 시 해당 신호만 스킵. 부분 시퀀스는 vte가 프레이밍 담당.

### D21: term-core는 raw OSC 133 이벤트만 노출, 세션 keying·정책은 app
- **Status**: resolved (reviewer #4; dependency-direction 제약)
- **Rationale**: OSC 133은 특정 세션의 PTY 바이트 스트림으로 도착 → 그 세션을 아는 것은 app(라우팅은 이미 `UserEvent::PtyOutput(TabId)` `terminal_app.rs:691`). term-core는 파싱한 이벤트(종류 A/B/C/D, exit code, 셸-emit 경과초)를 GridSnapshot 유사 타입 접근자(`lib.rs:124` `app_cursor_keys` 패턴)로 노출만; ≥30초 임계·에러 무시목록·waiting 조립은 app이 소유(term-core는 UI 정책·wgpu/winit 비의존 유지).

## Constraints
- **dependency-direction**: `term-core`는 wgpu/winit 의존 금지. OSC 133로 파싱한 명령 경계·exit code·프롬프트 상태는 term-core가 GridSnapshot 유사 계약(또는 별도 접근자, `lib.rs:124` 패턴)으로 노출하고, waiting 상태 조립은 `app`이 소유.
- **no-gui-crates**: waiting 밴드도 per-cell 수제 렌더(기존 SidebarLayout/Icon/theme 재사용). GUI 위젯 크레이트 금지.
- **macos-only / zsh-local-only**: `/bin/zsh` 로컬 세션만. `#[cfg(target_os)]` 분기·타 셸 지원 금지.
- **오탐 최소화가 최상위 품질 제약**: 무의미한 waiting 항목 하나가 신뢰를 깨고 수동 스캔으로 회귀시킴(= 실패). {1,130} 무시목록·활성세션 제외·에러우선이 모두 이 제약의 방어선.
- **Cargo.lock 직접 편집 금지**(PreToolUse hook) — cargo가 관리.

## Known Gaps
- **완료 임계값 30초는 잠정 기본값** — 실사용 후 config로 조정 가능(L3에서 config 노출 여부 결정).
- **사이드바 collapsed(숨김) 상태의 알림 표현** — 접힌 사이드바에서 waiting을 어떻게 알릴지(배지/펼침 힌트)는 v1 후속 과제.
- **OSC 133 zsh 훅 스크립트의 실제 코드 검증** — D18(chain 방식)·D19(경과초 emit)의 구체 스크립트가 oh-my-zsh/starship/p10k 실환경에서 프롬프트를 깨지 않는지는 구현 시 실측(주입 실패는 D13 우아한 저하로 흡수).
- **비-Claude 에이전트의 Waiting 감지** — D16은 Claude jsonl 스키마(`stop_reason`/`toolUseResult`) 기반. Codex/Gemini/Aider 등은 로그 소스가 달라 v1에서 Waiting=Unknown(기존 `activity.rs` 정책 유지).

## Requirements

### R0: 세션 훑기 없이 주목 필요 세션을 놓치지 않는다 (goal)
#### R0.1: 대기 전환이 제때 밴드에 뜬다
- **Given**: 비활성 세션에서 Claude가 턴을 넘김(end_turn)
- **When**: 최대 폴링 주기(bg ~2s, 상한 5s) 경과
- **Then**: 그 세션이 waiting 밴드에 등장 (≤5초)
#### R0.2: 한눈 인지로 개별 세션 열람이 불필요
- **Given**: 비활성 3세션 중 1세션이 입력 대기 상태
- **When**: 사용자가 사이드바를 흘깃 봄
- **Then**: 대기 세션이 아이콘+색으로 즉시 식별되어, 나머지 세션을 하나씩 열어볼 필요가 없음

### R1: Waiting 밴드 렌더 & 레이아웃 (D1,D2,D3,D6,D9,D11)
#### R1.1: 밴드는 헤더와 카드 리스트 사이에 위치
- **Given**: waiting 항목 ≥ 1
- **When**: 사이드바 렌더
- **Then**: 헤더 밴드 아래·카드 리스트 위에 waiting 밴드가 그려지고 `list_top`이 밴드 높이만큼 하향
#### R1.2: 빈 상태는 공간을 차지하지 않음
- **Given**: waiting 항목 0
- **When**: 렌더
- **Then**: waiting 밴드 높이 0, 카드 리스트는 기존 `list_top` 위치 유지
#### R1.3: 행은 아이콘+세션 이름만
- **Given**: 항목(신호=에러, 세션="app")
- **When**: 행 렌더
- **Then**: `[✕] app` 형태 — 경과 타이머·에러 문구 조각 미표시
#### R1.4: 신호별 아이콘/색 인코딩
- **Given**: 대기/완료/에러 세 신호
- **When**: 각 행 렌더
- **Then**: ⏸(대기)/✓(완료)/✕(에러) 아이콘 + 신호별 색 (RowMark/Icon/theme 확장, 신규 변형)
#### R1.5: 트리거 시각 오름차순 정렬
- **Given**: 항목 트리거 시각 t1<t2<t3
- **When**: 렌더
- **Then**: t1(가장 오래 기다림) 최상단, t3 최하단
#### R1.6: 최대 5개 + 요약
- **Given**: waiting 대상 세션 9개
- **When**: 렌더
- **Then**: 상위 5행 표시 + "+4 more" 요약 한 줄

### R2: 항목 생명주기 & 우선순위 (D12,D15,D17)
#### R2.1: 활성화 시 제거(확인)
- **Given**: 세션 S가 waiting에 있음
- **When**: 사용자가 S로 전환(클릭 또는 키보드 단축키)
- **Then**: S 항목이 waiting에서 제거
#### R2.2: 클리어 후 재발
- **Given**: S가 클리어됨
- **When**: S에서 새 신호(대기/완료/에러) 발생
- **Then**: S가 다시 waiting에 등장
#### R2.3: 활성 세션은 항목을 만들지 않음
- **Given**: 현재 활성 탭 A에서 신호 발생
- **When**: 감지
- **Then**: A는 waiting 항목 미생성
#### R2.4: 세션당 1항목, 우선순위 에러>대기>완료
- **Given**: 세션 S에 에러와 완료 동시 충족
- **When**: waiting 조립
- **Then**: S 행 1개만, 아이콘/색은 에러(최상위 신호), 정렬 기준도 그 신호의 트리거 시각

### R3: 에이전트 입력 대기 감지 (D4,D16,D8-agent)
#### R3.1: end_turn → Waiting
- **Given**: 비활성 세션 jsonl 마지막 assistant 이벤트 `stop_reason=="end_turn"`
- **When**: tabmeta 폴링 tick
- **Then**: `AgentState=Waiting`, 대기 신호 발생
#### R3.2: tool_use → Busy(대기 아님)
- **Given**: 마지막 assistant 이벤트 `stop_reason=="tool_use"`
- **When**: 폴링
- **Then**: Busy 판정, 대기 신호 미발생
#### R3.3: tool_result → Busy
- **Given**: 마지막 user 라인에 `toolUseResult` 존재
- **When**: 폴링
- **Then**: Busy, 대기 신호 미발생
#### R3.4: human_prompt → Busy
- **Given**: 마지막 user 라인에 `toolUseResult` 없음(사람이 방금 입력)
- **When**: 폴링
- **Then**: Busy(에이전트 응답 예정), 대기 신호 미발생
#### R3.5: 로그 없음 → Unknown
- **Given**: 세션 cwd로 해결되는 jsonl 없음(또는 비-Claude 에이전트)
- **When**: 폴링
- **Then**: Unknown, 대기 신호 미발생
#### R3.6: 신선도 ≤5초
- **Given**: 비활성 세션이 end_turn으로 전환
- **When**: bg 폴링 주기(~2s, 상한 5s) 내
- **Then**: Waiting 상태가 밴드에 반영

### R4: 셸 통합 주입 & OSC 133 파싱 (D10,D18,D20,D21) — zsh↔term-core↔app 경계
#### R4.1: (producer) zsh 주입
- **Given**: 세션 spawn
- **When**: `Pty::spawn`
- **Then**: `ZDOTDIR`가 우리 생성 디렉터리로 설정되고 그 `.zshrc`가 OSC 133 훅을 등록
#### R4.2: (producer) 사용자 rc chain
- **Given**: 사용자가 원래 `$ZDOTDIR`(없으면 `$HOME`)에 `.zshenv`/`.zshrc` 보유
- **When**: 주입 zshrc 로드
- **Then**: 원래 rc를 먼저 source한 뒤 훅을 add-only로 추가(프롬프트 문자열 미변경, oh-my-zsh/starship 공존)
#### R4.3: (producer) 명령 마커 emit
- **Given**: 포그라운드 명령 실행
- **When**: preexec/precmd
- **Then**: OSC 133 마커 + exit code + 셸 측정 경과초를 PTY로 emit
#### R4.4: (parser) osc_dispatch 파싱
- **Given**: OSC 133 시퀀스가 PTY 스트림에 도착
- **When**: `Performer::osc_dispatch` (신설, `parser.rs:242`)
- **Then**: 종류/exit/경과초를 파싱해 term-core 이벤트로 보관
#### R4.5: (parser) 견고성
- **Given**: 미지 OSC 종류 또는 불완전 param
- **When**: `osc_dispatch`
- **Then**: 패닉/unwrap 없이 조용히 드롭, 파서 상태 불변
#### R4.6: (consumer) 세션 keying
- **Given**: OSC 이벤트가 세션 S의 PTY에서 발생
- **When**: app 처리(`UserEvent::PtyOutput(TabId)` 경로)
- **Then**: 이벤트가 `TabId(S)`로 태깅되어 waiting 조립에 라우팅
#### R4.7: (contract) term-core는 raw 이벤트만 노출
- **Given**: 파싱된 OSC 133 이벤트
- **When**: app이 term-core에서 조회
- **Then**: GridSnapshot 유사 accessor(`app_cursor_keys` 패턴 `lib.rs:124`)로 raw 이벤트만 노출; 임계/무시목록/조립은 app 소유, term-core는 wgpu/winit·UI 정책 비의존

### R5: 완료 신호 (D5,D19)
#### R5.1: ≥30초 포그라운드 완료
- **Given**: 포그라운드 명령이 40초 실행 후 exit 0, 세션 비활성
- **When**: precmd emit + app 처리
- **Then**: 완료(✓) 신호 발생
#### R5.2: 짧은 명령 무시
- **Given**: 명령이 0.1초 실행 후 exit 0
- **When**: 처리
- **Then**: 완료 신호 미발생(< 30초 임계)
#### R5.3: `&` 백그라운드 잡 무시
- **Given**: `&`로 백그라운드 실행된 잡 완료(프롬프트 경계 마커 없음)
- **When**: 처리
- **Then**: 완료 신호 미발생
#### R5.4: 경과초는 셸이 측정
- **Given**: 명령 실행 중 랩톱 슬립 발생
- **When**: precmd
- **Then**: 경과초는 셸 `EPOCHREALTIME` 차이로 계산되어 OSC payload에 실림(앱측 시계 비의존)

### R6: 에러 신호 (D7,D14)
#### R6.1: 에러 발생
- **Given**: 포그라운드 명령 exit code 101, 세션 비활성
- **When**: 처리
- **Then**: 에러(✕) 신호 발생
#### R6.2: 무시목록 {1,130}
- **Given**: 명령 exit code 1(grep 무매치) 또는 130(Ctrl-C)
- **When**: 처리
- **Then**: 에러 신호 미발생
#### R6.3: 에러>완료 억제
- **Given**: 명령이 40초 실행 후 exit 101(완료·에러 동시 충족)
- **When**: 처리
- **Then**: 에러(✕) 신호만, 완료 신호 억제 (R2.4 우선순위)

### R7: 우아한 저하 (D13)
#### R7.1: 통합 실패 시 부분 동작
- **Given**: OSC 133 주입 실패/미동작(사용자 zsh 충돌)
- **When**: 세션 사용
- **Then**: 완료/에러 신호는 미발생하되 앱은 정상, 에이전트 대기 신호(로그 기반)는 계속 동작
#### R7.2: 경고 없음
- **Given**: 셸 통합 미동작
- **When**: 렌더
- **Then**: 경고 배너/토스트 미표시(조용한 저하)

## Tasks

### T1: OSC 133 셸 통합 파이프라인 (emit + parse + accessor) [infra]
- **Fulfills**: R4.1, R4.2, R4.3, R4.4, R4.5, R4.6, R4.7
- **Depends on**: (none) ← T2와 병렬
- **Touches (예상)**: `crates/term-core/src/parser.rs`(osc_dispatch 신설), `crates/term-core/src/lib.rs`(accessor), `crates/term-core/src/pty.rs`(ZDOTDIR 주입), 신규 zsh 통합 스크립트 asset, `crates/app/src/session.rs`(파싱 이벤트를 TabId로 라우팅). OSC 133 wire 계약의 생산자(zsh)·소비자(파서)를 한 태스크에 두어 인터페이스를 내부에서 완결. 테스트: insta 파서 테스트(OSC 133 바이트→이벤트), `pty_smoke` 계열 실-zsh 스모크.

### T2: tabmeta 입력-대기(Waiting) 감지 [infra]
- **Fulfills**: R3.1, R3.2, R3.3, R3.4, R3.5, R3.6
- **Depends on**: (none) ← T1과 병렬 (파일 비중첩: tabmeta/ai-monitor vs term-core/pty)
- **Touches (예상)**: `crates/app/src/tabmeta/mod.rs`(`AgentState`에 `Waiting` 추가), `crates/app/src/tabmeta/activity.rs`(`state_from` 확장), `crates/ai-monitor/src/activity.rs`(`tail_state`를 role-only → role+stop_reason+toolUseResult). 테스트: jsonl 픽스처 단위 테스트(end_turn/tool_use/tool_result/human_prompt/none → Waiting/Busy/Unknown).

### T3: app waiting-state 조립 모델 [core]
- **Fulfills**: R2.1, R2.2, R2.3, R2.4, R5.1, R5.2, R5.3, R5.4, R6.1, R6.2, R6.3, R7.1, R7.2
- **Depends on**: T1, T2 (두 신호 소스 모두 소비)
- **Touches (예상)**: 신규 `crates/app/src/waiting.rs`(항목 집합·임계 30초·무시목록{1,130}·우선순위 에러>대기>완료·세션당 1항목·활성 제외·활성화 시 클리어·재발), `crates/app/src/sidebar_model.rs`(Waiting arm 매핑), `crates/app/src/terminal_app.rs`(신호 소스 배선·탭 전환 시 클리어 훅). 셸 통합 부재 시 완료/에러만 비활성(R7). 테스트: 단위 테스트(우선순위·임계·무시목록·클리어/재발·활성 제외).

### T4: 사이드바 waiting 밴드 렌더 + 입력 배선 [vertical]
- **Fulfills**: R0.1, R0.2, R1.1, R1.2, R1.3, R1.4, R1.5, R1.6, R2.1
- **Depends on**: T3
- **Touches (예상)**: `crates/renderer/src/sidebar_row.rs`·`sidebar_layout.rs`(헤더~카드 사이 waiting 밴드, `list_top` 하향, 최대 5+"+N more", 시각순), `crates/renderer/src/sidebar_icons.rs`·`sidebar_theme.rs`(⏸/✓/✕ 아이콘+색), `crates/renderer/src/sidebar_row.rs`(`RowMark` 확장), `crates/app/src/input.rs`(밴드 hit-test→해당 세션 Switch), `crates/app/src/terminal_app.rs`(모델 push·밴드 입력). 홀리스틱 검증은 execute 파이프라인이 담당(별도 final-verify 태스크 없음).

## External Dependencies

### Pre-work
- (none) — 전부 리포 내 작업. 단, 주입 zsh 스크립트는 `zmodload zsh/datetime`(EPOCHREALTIME)·OSC 133을 사용하므로 macOS 기본 zsh(5.9)에서 동작 확인 필요(코드 내 검증).

### Post-work
- `/run-term`으로 밴드 렌더·실시간 갱신 육안 확인.
- 파서/그리드 스냅샷 변경분은 `/insta-review`로 검토·수락.
- 성공 지표(세션 훑기 20→3회/일)는 실사용 관찰 항목 — 릴리스 후 일주일 자가 점검.
