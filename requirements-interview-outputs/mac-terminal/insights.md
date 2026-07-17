# Deep Interview Insights: macOS 커스텀 터미널 (Warp × iTerm)
> Date: 2026-07-16
> Rounds: 7
> Final Ambiguity Score: 0.14 ✅

## Core Problem
Warp의 tmux 스크롤 불가와 iTerm2의 이미지 붙여넣기 부재에 지쳐, Rust로 밑바닥부터 만드는 macOS 전용 GPU 터미널을 데일리 드라이버로 삼고 원하는 기능을 계속 확장하려 함.

## Key Insights & Decisions
- **스택: Rust + 순정 wgpu/Metal GPU 스택** (GUI 크레이트 미사용). Warp도 Rust + 자체 GPU 프레임워크(Metal)라는 사실을 확인하고 이 길을 선택 — 셀 하나하나 수동 제어의 자유를 위해 "레이아웃/위젯 지옥"을 감수.
- **ai-monitor는 Rust로 재작성.** 단순 TUI 실행이 아니라 네이티브 통합 — 터미널이 세션 로그(`~/.claude/projects/**/*.jsonl`, Gemini CLI 로그)를 직접 읽어 자체 UI로 렌더.
- **이미지 붙여넣기는 (c) 둘 다**: 화면 인라인 표시(iTerm2/Kitty 프로토콜급) + 클립보드 이미지를 파일로 저장해 경로를 외부 AI CLI(claude/gemini)에 붙여넣기. Warp의 이미지 첨부는 내장 AI 전용이라 외부 CLI엔 안 맞다는 점이 근거.
- **ai-monitor UI = 단축키 토글 오버레이**, 감시 대상은 현재 활성 탭 세션만.

## Defined Requirements (v1)
- 순정 wgpu 셀 그리드에 실제 zsh 출력 렌더
- 스크롤 (tmux 세션에서도 동작 — Warp의 원 페인포인트 해결)
- 탭 형식 새 창/세션 영역
- 이미지 붙여넣기 — 인라인 표시 + 파일 경로 전달 (둘 다)
- ai-monitor 네이티브 통합 — 토글 오버레이, 활성 세션 감시
- 폰트·컬러테마·ligature 커스터마이즈 (설정) — v1 포함
- 성능은 숫자 기준으로 판정 (예시 후보: 120fps 유지 / 입력→화면 반영 8ms 이하 / 10만 줄 출력 스크롤 시 프레임 드롭 없음)

## Non-Goals (v1에서 안 함)
- SSH·원격 세션 (로컬 셸만)
- bash/fish (zsh만)
- Windows/Linux 이식 (macOS 전용)

## Success Criteria
하루 종일 iTerm/Warp 대신 이 터미널로 실제 작업을 하고, 성능 숫자 기준을 만족하며, 이미지 붙여넣기·ai-monitor 통합이 실사용 가능한 상태.

## Identified Risks & Failure Modes
- **범위 초과 위험**: 순정 GPU 스택은 "글자 하나 제대로 그리기"부터 큰 산인데 v1에 모든 기능(이미지·ai-monitor·설정)을 다 넣기로 함 → 중간 좌초 가능성. 마일스톤 쪼개기가 생존 관건.
- **ai-monitor 재작성 비용**: TS/Node 자산을 Rust로 옮기는 작업량이 별도 프로젝트급.
- **1인 개발 + 가장 어려운 렌더링 경로** 조합의 지속성.

## Open Questions & Unknowns
- 성능 목표의 정확한 숫자 미확정 (120fps/8ms/10만 줄은 예시 — 확정 필요).
- (d) "남김"의 정확한 의미 확인 필요: 폰트·테마·ligature 커스터마이즈를 v1 포함으로 해석했으나, "설정은 나중으로 미룸"을 의도했다면 정정 필요.
- 이미지 인라인 표시에 어떤 프로토콜을 채택할지 (iTerm2 vs Kitty vs 자체).
- v1 내부 마일스톤 순서(무엇부터 만들지)는 계획 단계 몫.

## Clarity Assessment
Ambiguity Score: 0.14
- Goal Clarity: 0.90 (40%)
- Constraint Clarity: 0.85 (30%)
- Success Criteria: 0.82 (30%)

Maturity: Solid — 목표·스택·범위 경계·성공 기준이 모두 구체적으로 못박혀 계획 단계로 넘어갈 수 있음.
