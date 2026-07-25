# Deep Interview Insights: 탭 UX — 세션 정체성(tab identity)
> Date: 2026-07-24
> Rounds: 3
> Final Ambiguity Score: 0.55

## Core Problem
멀티 세션 터미널에서 각 탭이 "지금 무슨 작업을 하는 세션인지"를 직관적으로 식별하기
어렵다 — '주목이 필요한가'(waiting area가 이미 해결)가 아니라 **세션의 정체**가 문제다.

## Key Insights & Decisions
- 이 문제는 "주목 라우팅"과는 별개 축이다. waiting area는 "어떤 탭을 봐야 하나"를 풀지만,
  "그 탭이 뭐였지?"는 못 푼다.
- 세션을 구분하는 결정적 단서의 우선순위: ① AI 에이전트에게 시킨 작업(무엇을 하는 세션인가),
  ② 작업 폴더/레포명.
- 탭 라벨은 처음 띄운 작업(고정 앵커)이 아니라 "현재 활동"으로 실시간 갱신되길 원한다.
  → 최신성을 안정성보다 우선.

## Defined Requirements
- 각 탭에 세션의 현재 AI 작업을 반영하는 라벨을 지속 갱신해 표시.
- 보조 정체 신호로 레포/폴더명을 노출.
- 기존 waiting area(주목 라우팅)와 역할을 분리해 유지.

## Identified Risks & Failure Modes
- 최신성 ↔ 안정성 트레이드오프: 라벨이 볼 때마다 바뀌면 "고정 앵커"가 없어 오히려 재혼란 가능.
- 소스 불명확: "현재 작업"을 무엇에서 유도할지(에이전트 jsonl 로그 vs foreground 프로세스)
  미정 — 잘못 뽑으면 노이즈.
- 압축 손실: 좁은 탭 폭에 긴 프롬프트를 몇 글자로 줄이면 정보가 뭉개질 수 있음.
- 스케일: 탭이 10·20개로 늘면 라벨만으로 부족 — 색/그룹/검색 등 대안 경로 필요(이번엔 미탐색).

## Open Questions & Unknowns
- 성공 판정 기준 미정: "직관적으로 구분 가능"을 어떻게 측정? (예: N개 탭에서 목표 탭을 X초 내 찾기)
- "현재 작업" 소스: ai-monitor가 쓰는 `~/.claude/projects/**/*.jsonl` 로그를 재사용? 아니면 활성 프로세스?
- 자동 유도 vs 수동 별명의 관계 (자동 우선이되 수동 override 허용?)
- 갱신 주기/디바운스: 매 이벤트마다? 몇 초 간격?
- 탭 다수 시 색상·그룹핑·검색 등 대안 경로가 실제로 필요한지.

## Clarity Assessment
Ambiguity Score: 0.55 ❌
- Goal Clarity: 0.6 (40%)
- Constraint Clarity: 0.4 (30%)
- Success Criteria: 0.3 (30%)

Maturity: Exploratory — "무엇을 보여줄지"는 또렷해졌으나, 성공 기준·현재-작업 소스·
압축/갱신 정책 등 핵심 결정이 아직 열려 있음.
