# Spec: atlas-swap-bind-invalidation

## Meta
- **Created**: 2026-08-04
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-08-04

## Goal
사이드바 탭 카드의 제목/서브라인 글자가 전혀 다른 글리프로 렌더링되는 버그를 고친다.
`Renderer::apply_config`가 아틀라스를 통째로 교체하는데, 사이드바 텍스트 레이어의
바인드 그룹 캐시 키가 그 교체를 감지하지 못해 죽은 텍스처를 계속 샘플링한다.
overlay/preedit 레이어도 같은 결함을 공유하므로 함께 고친다.

## Non-goals
- 그리드(본문) 렌더 경로 변경 — 이미 올바르게 재생성된다 (`lib.rs:656`, `lib.rs:677`)
- `apply_config`가 **언제** 호출되는지 바꾸는 것 (config 워처 / 에코 억제 로직은 그대로)
- 아틀라스 교체 자체를 없애는 최적화 (예: 폰트 크기가 안 바뀌면 재생성 생략) — 별도 과제
- 사이드바 시각 디자인·레이아웃 변경

## Confirmed Goal
`Atlas`에 인스턴스 정체성(instance identity)을 부여하고, 아틀라스 텍스처를 샘플링하는
**모든** 텍스트 레이어 — `sidebar_row`, `overlay_layer`, `preedit_layer` — 가 그 정체성을
바인드 그룹 무효화 조건에 포함하도록 만든다. 라이브 config 리로드가 아틀라스를 교체해도
세 레이어 모두 올바른 글리프를 그려야 하며, 현재 코드에서 실패하는 헤드리스 회귀 테스트로
이를 고정한다.

## Research

### 결함의 기계적 경로
- `Renderer::apply_config`는 `Atlas::new`로 **새 GPU 텍스처를 통째로 만들어** `self.atlas`에 대입한다 (`crates/renderer/src/lib.rs:654`). 그리드용 `glyph-bg` 바인드 그룹만 함께 재생성된다 (`lib.rs:656-673`).
- `Atlas::new`는 `generation: 0`으로 초기화한다 (`crates/renderer/src/atlas.rs:149`). `generation`은 아틀라스 **내부** eviction 카운터이며 (`atlas.rs:333`, `atlas.rs:354`에서만 증가), 인스턴스 정체성이 아니다.
- 사이드바 텍스트 파이프라인의 캐시 키는 `(format, atlas.generation(), cell)` (`crates/renderer/src/sidebar_row.rs:904-909`, 필드 타입 `sidebar_row.rs:820`). 교체 직후 세 필드가 모두 불변이다:
  - `format` — 서피스 포맷은 런타임에 안 바뀜
  - `generation` — `0 → 0` (새 인스턴스라서)
  - `cell` — `cell_size_px`는 `font.size_pt * scale_factor`만 사용 (`lib.rs:1375-1378`)
- 따라서 바인드 그룹이 재생성되지 않고 **옛 텍스처 뷰**를 계속 참조한다. 반면 UV는 **새** 아틀라스 기준으로 계산된다 (`sidebar_row.rs:1002` `push_glyphs`) → 셰이더가 엉뚱한 위치를 샘플링 → 글자마다 다른 글리프.
- 정정(핸드오프 대비): 옛 텍스처는 바인드 그룹이 refcount를 잡고 있어 **해제되지 않는다**. use-after-free가 아니라 **stale contents 샘플링**이다 — 크래시·validation error 없이 세션 내내 조용히 깨진 상태가 유지되는 관측 사실과 일치한다.

### 아틀라스 소비자 전수 조사 (4곳)
| 소비자 | 바인드 그룹 재생성 조건 | 상태 |
|---|---|---|
| 그리드 (`lib.rs:422` 초기 / `lib.rs:666` 재생성) | `apply_config`에서 명시적 재생성 + `last_snapshot = None`으로 재셰이핑 강제 (`lib.rs:677`) | **정상** |
| `sidebar_row` (`sidebar_row.rs:920-924`) | `(format, generation, cell)` 복합 키 (`sidebar_row.rs:904`) | **결함 — 교체 미감지** |
| `overlay_layer` (`overlay_layer.rs:227-231`) | `ensure_gpu`가 **format만** 검사 (`overlay_layer.rs:152-154`) | **결함 — 더 심각** |
| `preedit_layer` (`preedit_layer.rs:322-326`) | `ensure_gpu`가 **format만** 검사 (`preedit_layer.rs:243-245`) | **결함 — 더 심각** |

overlay/preedit는 서피스 포맷이 런타임에 불변이므로 **첫 프레임 이후 영원히 재생성되지 않는다.** 사이드바와 달리 아틀라스 정체성 검사가 아예 없어, 폰트 크기가 **바뀌는** 리로드에서도 깨진다.

### 트리거 경로
- tab-resize(PR #23)가 사이드바 폭을 config 파일에 저장 → 워처가 그 쓰기를 되돌려줌 → `settings::apply` → `apply_config`.
- 에코 억제는 **전체 config가 완전히 동일할 때만** 걸린다 (`crates/app/src/terminal_app.rs:226-238`, `last_applied_cfg` 전체 비교). 폭 변경은 진짜 변경이므로 통과 → 사이드바를 드래그하면 사이드바 글자가 깨진다.
- 주석이 이 경로를 이미 인지하고 있다: *"the atomic rename IS a change event, so the watcher hands our own write straight back, and `settings::apply` would rebuild the whole glyph atlas"* (`terminal_app.rs:226`).

### 테스트 인프라 (기존 관례)
- 크레이트 루트에 `#[cfg(test)] pub(crate) fn headless() -> Option<(Device, Queue)>` (`lib.rs:1415-1425`) — 어댑터 없으면 `None`으로 **skip**하는 것이 이 크레이트의 관례. 형제 모듈 테스트에서 쓰라고 루트에 hoist되어 있음 (D10).
- `preedit_layer.rs:631`, `lib.rs` 테스트 모듈이 각자 로컬 `headless()`도 갖고 있음 (중복).
- 테스트에서 `FrameCtx`를 직접 조립하는 선례 존재: `lib.rs:2712`, `lib.rs:2781`, `preedit_layer.rs:799`.
- 검증 명령: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` (CLAUDE.md).

### 관련 제약
- `.claude/rules/dependency-direction.md` — `Atlas`는 `renderer` 크레이트 내부. `term-core` 무관, 이 수정은 경계를 건드리지 않음.
- `docs/architecture.md` G1 — 유휴 CPU 부채(매 프레임 재셰이핑)는 이미 해결됨. 바인드 그룹 재생성이 **매 프레임** 일어나면 그 회귀가 된다 → 무효화는 정확히 교체 시점에만.

### L2 인터뷰 중 확인된 추가 사실
- **인스턴스는 매 프레임 재생성된다.** `record`가 `text.clear()` 후 `push_glyphs`로 다시 채운다 (`sidebar_row.rs:1155`, `1225` 이하). 따라서 UV는 항상 현재 아틀라스 기준으로 신선하며, **stale한 것은 바인드 그룹뿐이다.** → 수정 범위가 바인드 그룹 무효화로 좁혀진다.
- **아틀라스는 내부적으로 텍스처를 재생성하지 않는다.** `create_texture`는 `Atlas::new` 안 단 한 곳뿐 (`atlas.rs:119`) — **D1 이후에는 전용 텍스처 팩토리 안 한 곳**. eviction(`atlas.rs:337` `pack`의 overflow 분기, `atlas.rs:328` `evict_for_test`)은 같은 텍스처를 재사용하며 `generation`만 올린다. `create_view`/`create_sampler`도 같은 출생 지점뿐이다 (`atlas.rs:133-139`).
  → `epoch`(텍스처 정체성)과 `generation`(한 텍스처 내 UV 유효성)은 서로 직교하지만, **바인드 그룹 키에 필요한 것은 `epoch`뿐이다.** `generation`이 표상하는 UV 무효화는 바인드 그룹이 아니라 **인스턴스 재발행**이 복구한다 (D2/D5). ⚠️ 이 줄의 결론은 L2 심사에서 교정되었다 — 초안은 "둘 다 키에 필요"였다.

## L2 Checkpoints

복잡도 분류: **Medium (2 signals), brownfield** — L2-reviewer가 최초 분류(Simple, 1 signal)를 반박했고 수용했다.
- **Data schema design**: D2는 합성 키의 필드별 정당화 + `Atlas`의 새 정체성 필드 + `epoch ⊥ generation` 직교성 계약까지 다룬다. "튜플 모양 변경"으로 축소할 수 없다.
- **3+ independent subsystems**: GPU 리소스를 각자 소유하는 세 모듈에 **서로 다른 두 가지 처방**이 적용된다 — overlay/preedit는 구조 분리(D3), sidebar는 키 확장(D4). 비균질 처방 자체가 신호다.
- 나머지 4개 신호(외부 리소스 로딩 / 상태 기계 / 비동기 / 다중 액터)는 없음.

가중치(brownfield): Core 30% / Scope 20% / Error 25% / Data 15% / Impl 10%.

| Dimension | R | P | U | Score |
|---|---|---|---|---|
| Core Behavior | 4 | 0 | 0 | 1.00 |
| Scope Boundaries | 3 | 0 | 0 | 1.00 |
| Error/Edge | 5 | 0 | 0 | 1.00 |
| Data Model | 4 | 0 | 0 | 1.00 |
| Implementation | 4 | 0 | 0 | 1.00 |

Data Model 체크포인트(2차 심사에서 3→4로 증설 — 키 스키마를 통째로 재작성한 차원이 검토 전 3개로 남아 있던 것은 부실이었다):
- [x] 인스턴스 정체성 신호의 출처와 **스탬프 지점** (D1)
- [x] 키 튜플의 필드 구성과 각 필드의 무효화 사유 (D2)
- [x] `epoch` 접근자 계약 (D1)
- [x] **`format` 필드의 지위** — 필수인가 중복 방어인가 (D2, 2차 심사에서 신설)

**Composite: 1.00** (threshold 0.80, per-dimension floor 0.60) · Unknown/Unknowns: 0
Inversion Probe 통과 · Unresolved Sweep 잔여 없음.

**L2-reviewer 판정: NEEDS_FIX → 반영 완료.** 사실 주장 6건은 전부 소스와 일치했으나, 결정 4건이 틀렸다. 반영 내역:
| 지적 | 반영 |
|---|---|
| 복잡도 과소 분류 | Simple → **Medium (2 signals)** (위) |
| `generation`·`cell`은 바인드 그룹 정체성에 **무영향** — 리빌드해도 바이트 동일 | **D2 전면 재작성** — 키를 `(format, epoch)`로 축소 |
| D1의 `epoch`은 텍스처가 아니라 **구조체** 정체성 — D7이 스스로 배격한 "불변식이 아니라 우연"에 D1만 면제 | **D1 전면 재작성** — 스탬프 지점을 텍스처 생성 팩토리로 이동 |
| D3은 파생 불가 — `Gpu`에 `text_bgl` 필드가 없어 나중에 바인드 그룹을 만들 수 없음 | **D3에 구조 변경 3항 명시** |
| D5가 `atlas.rs:61-67`의 문서화된 intra-frame eviction 갭과 충돌 | **D5 범위 한정** + Known Gaps 기록 |
| Error/Edge 1.00은 비현실적 (`Atlas::new` 실패 경로 미정의) | **D8 신설** + Error/Edge 체크포인트 3→5 |
| D6 테스트가 공허하게 통과할 수 있음 (early-return) / epoch 값 단정은 flaky | **D6에 계약 4항 명시** |

**2차 심사 판정: NEEDS_FIX → 반영 완료.** 1차 수정의 실질은 전부 소스로 재확인되어 유지되었고(특히 "`generation` 제거가 안전한가" — 바인드 그룹 내용물 `{uniform, view, sampler}`의 교체 경로를 전수 조사한 결과 `epoch` 불변인데 리빌드가 필요한 경로는 **없음**), 남은 5건을 교정했다:
| 지적 | 반영 |
|---|---|
| Research 67행이 D2와 모순 ("둘 다 키에 필요"가 살아남음) | Research 결론 **재작성** — 상위 섹션이 폐기된 지침을 사실처럼 제시하던 문제 |
| D2의 `format` 정당화가 **틀림** — 구조체 통째 교체가 이미 키를 `None`으로 리셋하므로 그 위험은 발생 불가. D3은 같은 필드를 "이중 안전망"이라 부르고 있어 **자기모순** | D2 **정직하게 재기술** + 1-필드 대안 병기. `generation`·`cell`을 걷어낸 것과 **같은 종류의 오류**였다 |
| D6 계약 1이 `PreeditLayer::record`의 가장 엄격한 조기 반환과 `reset_shared_model()` 관례를 누락 | 계약 1을 레이어별로 분해 + 1b 신설 |
| D3이 `text_bind`의 최종 형태를 안 정함 (`Option`인가 아닌가) | **규칙 4 신설** — 비-`Option` 유지·재대입 방식 (no-`unwrap()` 규칙 충돌 회피) |
| Data Model만 R=3으로 남아 1.00이 비현실적 — 키 스키마를 통째로 재작성한 차원인데 | 체크포인트 3→4 (`format` 지위) |
| D1 기각 목록이 "카운터를 아예 안 두는" 대안을 다루지 않음 | 기각 사유 추가 (역방향 결합) — steelman 자체는 **PASS** |

## Decisions

### D1: `epoch: u64`는 **텍스처가 태어나는 지점**에서 찍는다 — `Atlas::new`가 아니라 텍스처 팩토리에서
- **Status**: resolved
- **Rationale**: 교체를 감지하려면 **텍스처 정체성**이 필요하다. 기존 `generation`은 아틀라스 *내부* eviction 카운터이고 `Atlas::new`가 항상 0으로 리셋하므로(`atlas.rs:149`) 그 역할을 못 한다. 프로세스 전역 `AtomicU64`의 단조 증가 값은 서로 다른 텍스처가 절대 같은 값을 갖지 않음을 보장한다.
- **스탬프 지점이 핵심이다** (L2-reviewer steelman에서 교정): `epoch`을 `Atlas::new` 본문에서 찍으면, 그 정확성은 *"`create_texture`가 `Atlas::new` 안 한 곳뿐"*(`atlas.rs:119`)이라는 **오늘의 코드 사실**에만 의존한다. 그건 불변식이 아니다. 누군가 `Atlas::resize()`나 `rebuild_at_ppem()`을 추가해 **구조체는 그대로 둔 채 내부 텍스처만 교체**하는 순간 — Known Gap #2가 스스로 제안하는 최적화가 정확히 그 모양이다 — `epoch`은 얼어붙고 동일한 버그가 재발한다. 그것도 `epoch`이라는 이름 때문에 아무도 의심하지 않는 필드 뒤에서.
  - 이 스펙은 D7에서 그리드를 두고 *"불변식이 아니라 우연"*이라 판정했다. **D1만 그 기준에서 면제될 수 없다.**
  - 채택: 텍스처·뷰·샘플러 생성을 단일 private 팩토리(예: `fn new_atlas_texture(device) -> (Texture, TextureView, Sampler, u64)`)로 모으고 **거기서** 카운터를 뽑는다. "새 텍스처 ⟹ 새 epoch"이 주석이 아니라 **구성상(by construction)** 참이 된다. 비용은 리팩터 몇 줄.
  - 기각: **`Atlas::new`에서 스탬프** — 위 이유. 굳이 택한다면 Known Gaps에 D7과 동형의 경고를 반드시 남겨야 하는데, 팩토리 방식이 공짜라 그럴 이유가 없다.
  - 기각: **"옛 generation + 1을 새 아틀라스에 이어받기"** — 버그의 원인인 *정체성과 eviction 카운터의 혼동*을 그대로 유지한다.
  - 기각: **`Renderer`별 카운터** — 단일 윈도우 앱이라 실익이 없고 `Atlas`가 소유자를 알아야 해 결합이 는다.
  - 기각: **wgpu `global_id()` 비교** — 핸들 id 재사용 가능성이 있고 안정성을 보장하는 공개 계약이 아니다.
  - 기각: **카운터를 아예 두지 않고, `Atlas`가 소비자별 바인드 그룹 캐시를 소유해 텍스처 교체 시 함께 버린다.** 가장 강력한 대안이다 — `epoch`도 키도 전역 카운터도 사라지고 **무효화를 잊는 것이 구조적으로 불가능해진다.** 이 스펙이 존재하는 이유가 정확히 "무효화를 잊었다"이므로 정면으로 겨냥한 설계이고, 덤으로 G-a(그리드)까지 공짜로 닫는다. 그럼에도 기각하는 이유: `Atlas`가 소비자들의 `BindGroupLayout`을 알아야 해 **역방향 결합**이 생기고(`renderer` 내부 계층이 뒤집힌다), 그 맵을 키잉하려면 `global_id()`(위에서 기각)나 호출자 제공 토큰이 필요해 결국 "카운터 없음"이 성립하지 않는다. **이 기각은 이 스펙 범위에서의 판단이며, 교체 사이트가 둘 이상으로 늘어나면 재검토 대상이다** (G-a와 함께).
- `Atlas`는 형제 접근자 `generation()`과 **동일한 가시성**의 `pub fn epoch(&self) -> u64`를 갖는다 (R1.4의 정정 참조 — 초안은 `generation()`을 `pub(crate)`로 잘못 적었다).

### D2: 바인드 그룹 캐시 키는 `(format, epoch)` **2-필드**다
- **Status**: resolved
- **Rationale**: 키는 **바인드 그룹의 실제 내용**을 반영해야 한다. 바인드 그룹이 담는 것은 `{uniform 버퍼, atlas.view(), atlas.sampler()}` 세 개뿐이다(`sidebar_row.rs:913-926`). 따라서:
  - **`format` — 구조적으로는 중복이며, 미래 방어용으로만 남긴다.** 초안은 "포맷 변경이 `uniform` 버퍼를 갈아치우므로 필요하다"고 적었으나 **그 위험은 발생할 수 없다**: 포맷이 바뀌면 `ensure`가 `SidebarTextPipeline::new`로 구조체를 통째로 교체하고(`sidebar_row.rs:888-894`), 새 구조체는 `bind: None, bind_key: None`으로 시작한다(`sidebar_row.rs:878-879`). 즉 키는 이미 `None`이라 `format` 비교가 발화할 일이 없다. overlay/preedit도 D3 규칙 2가 적용되면 동일하다(`ensure_gpu`가 `self.gpu = Some(Gpu {…})`로 통째 교체).
    → 그래도 남기는 이유는 단 하나: **캐시가 format-gated 구조체 밖으로 나가는 미래 리팩터에 대한 방어.** 비용은 프레임당 enum 비교 하나. 이것이 "필요"가 아니라 "이중 안전망"임을 D3 규칙 2와 일관되게 기술한다.
    → 대안 **(i) `format`도 제거해 키를 `Option<u64>` 1-필드로** 축소하는 것도 똑같이 타당하다. D2 자신의 기준("키는 바인드 그룹의 실제 내용을 반영해야 한다")을 끝까지 밀면 이쪽이다. **채택하지 않은 이유는 취향이 아니라 비대칭 위험**: `epoch`을 빼면 버그가 나지만 `format`을 남겨도 손해가 없다. 승인 시 뒤집어도 무방한 결정.
  - **`epoch` — 필요하다.** 텍스처·뷰·샘플러 교체를 잡는 유일한 신호(D1).
  - **`generation` — 제거한다.** eviction은 **같은 텍스처를 재사용**하고 `generation`만 올린다(`atlas.rs:337`의 overflow 분기). 뷰도 샘플러도 그대로이므로 재생성해봐야 **바이트 동일한 바인드 그룹**이 나온다. UV 무효화는 바인드 그룹이 아니라 **인스턴스 재발행**으로 복구되며 그건 이미 매 프레임 일어난다(D5). `generation`을 키에 두는 것은 범주 오류이고, L3가 검증 불가능한 요구사항을 쓰게 만든다.
  - **`cell` — 제거한다.** `TextUniforms`는 `{viewport, _pad}`뿐이라(`sidebar_row.rs:741-744`) **`cell`은 바인드 그룹에 아예 들어가지 않는다.** 게다가 원래 정당화였던 "`Atlas::new` 실패 경로"는 성립하지 않는다 — 그 경로에서 바인드 그룹은 **이미 올바르다**(살아 있는 옛 아틀라스를 가리킴). 덤으로 `cell_size_px`는 반올림하므로(`lib.rs:1375-1378`) 실제 `size_pt` 변화가 `cell`을 안 움직일 수도 있어 안전망 구실도 못 한다.
  - **Constraints와의 정합**: `generation`·`cell`을 남기면 *교체가 아닌* 시점(eviction마다, 반올림된 cell 변화마다)에 리빌드가 돌아 "재생성은 정확히 교체 시점에만"이라는 이 스펙 자신의 제약을 위반한다. 2-필드 키는 그 제약과 일치한다.
  - 부수 효과: `sidebar_row`의 `bind_key` 타입이 `Option<(TextureFormat, u64, [u32; 2])>`(`sidebar_row.rs:820`)에서 `Option<(TextureFormat, u64)>`로 **좁아진다**. 기존 필드를 늘리는 게 아니라 잘못된 필드를 걷어내는 방향임에 유의.

### D3: overlay/preedit는 아틀라스 바인드 그룹을 format-gated `ensure_gpu`에서 **분리**한다
- **Status**: resolved
- **Rationale**: 두 레이어는 파이프라인·셰이더 모듈·유니폼 버퍼·쿼드·바인드 그룹을 `ensure_gpu` 하나에서 만들고 **서피스 포맷만** 검사한다(`overlay_layer.rs:152`, `preedit_layer.rs:243`). 포맷은 런타임 불변이라 첫 프레임 이후 영원히 재생성되지 않는다. 그런데 아틀라스 교체가 무효화하는 것은 **바인드 그룹뿐**이며 파이프라인·셰이더는 멀쩡하다.
  - 채택: `ensure_gpu`는 format-gated 그대로 두고, 아틀라스 바인드 그룹만 D2 키로 별도 캐시(`ensure_text_bind`)로 뺀다. 교체당 비용이 최소이고, 세 레이어가 **동일한 무효화 관용구**를 갖게 된다.
  - 기각: **`ensure_gpu` 게이트를 넓히기** — diff는 가장 작지만 config 리로드마다 파이프라인과 셰이더 모듈까지 통째로 재생성한다. 사이드바 드래그 한 번이 그 비용을 반복해서 문다.
  - 기각: **공유 `AtlasBindCache` 타입 추출** — 가장 통일적이지만 지금 동작하는 `sidebar_row` 코드까지 건드려 회귀 표면을 넓힌다. 세 곳이 같은 키 모양을 쓰는 것으로 충분하다.
- **필수 구조 변경 3항** (L2-reviewer 지적 — 이것 없이는 D3이 파생 불가):
  1. **`Gpu`에 `text_bgl: wgpu::BindGroupLayout` 필드를 추가한다.** 현재 `text_bgl`은 **스택 지역 변수**로 만들어져 `build_pipeline`에 `&text_bgl`로 넘겨진 뒤 그대로 버려진다(`overlay_layer.rs:195`, `preedit_layer.rs:290`). `struct Gpu`(`overlay_layer.rs:123-131`, `preedit_layer.rs:212-220`)에는 `bgl` 필드가 없다 — `SidebarTextPipeline`이 `bgl`을 보관하는 것(`sidebar_row.rs:816`)과 대조적이다. BGL을 보관하지 않으면 **나중에 바인드 그룹을 만드는 것 자체가 불가능하다.**
  2. **`text_bind`와 `bind_key`는 `Gpu` 안에 둔다** (레이어 본체가 아니라). 레이어에 두면 포맷 변경으로 `Gpu`가 통째로 재생성될 때 캐시된 바인드 그룹이 **교체된 `uniform` 버퍼**를 가리킨 채 남는다. `Gpu` 안에 두면 그 위험이 구조적으로 사라진다 (D2의 `format` 필드는 이때 이중 안전망).
  3. **`record`에서 `ensure_gpu` → `ensure_text_bind` 순서를 지킨다.** 후자는 전자가 만든 `text_bgl`에 의존한다. (preedit는 `ensure_gpu` 호출 지점이 이미 조기 반환 뒤에 있다 — `preedit_layer.rs:481`.)
  4. **`text_bind`는 `Option`으로 바꾸지 않는다.** 현재 `text_bind: wgpu::BindGroup`은 비-`Option` 필드로 draw 시점에 무조건 소비된다(`overlay_layer.rs:130`→`361`, `preedit_layer.rs:219`→`540`). `ensure_gpu`가 지금처럼 **즉시 생성**하고, `ensure_text_bind`는 키가 어긋날 때 그 필드를 **재대입**한다. `Option`으로 바꾸면 draw 지점에 `unwrap()`/`expect()`가 생겨 CLAUDE.md의 no-`unwrap()` 규칙과 충돌하고, 호출부 두 곳을 불필요하게 건드린다.

### D4: `sidebar_row`는 구조를 유지하고 키만 **교정**한다
- **Status**: resolved
- **Rationale**: `SidebarTextPipeline`은 이미 올바른 *모양*(프레임당 키 비교 후 조건부 재생성, `sidebar_row.rs:904-928`)이다. 결함은 구조가 아니라 **키에 들어간 필드**뿐이다. D2에 따라 `bind_key`를 `Option<(TextureFormat, u64)>`로 바꾸고 `generation`·`cell`을 걷어내고 `epoch`을 넣는다. 호출부 구조는 그대로 — 최소 diff가 최소 회귀다.

### D5: 무효화의 작용 범위는 **바인드 그룹뿐**이다 — 인스턴스/셰이핑 캐시는 손대지 않는다
- **Status**: resolved
- **Rationale**: 세 레이어 모두 `record`마다 인스턴스를 처음부터 다시 만든다(`sidebar_row.rs:1155`의 `text.clear()` 이후 `push_glyphs`). UV는 매 프레임 현재 아틀라스 기준으로 재계산되므로 별도 무효화 장치가 필요 없다. 그리드의 `last_snapshot = None`에 해당하는 강제 재셰이핑을 레이어에 도입할 이유가 없다.
- **범위 한정 (L2-reviewer 지적):** 이 주장은 **프레임 경계에서만** 참이다. `atlas.rs:61-67`이 이미 문서화한 대로, 한 프레임 안에서 `pack`이 eviction을 일으키면 **그 프레임에 이미 발행된 인스턴스의 UV가 무효화된다** — 그리고 `draw`는 `record`가 글리프를 전부 push한 *뒤에* `ctx.atlas`를 읽으므로, 실제로 깨진 프레임이 오히려 키를 "정상"으로 확정하게 된다. 이는 **기존 Known Gap이며 이 스펙이 고치지 않는다**(2048² 아틀라스가 확률을 실용 범위 밖으로 밀어낸 상태). D5는 "intra-frame eviction이 없는 한"으로 읽어야 한다. Known Gaps에 기록.

### D6: 회귀 테스트는 레이어 파일 안의 `#[cfg(test)]` GPU 테스트로, **양방향**을 고정한다
- **Status**: resolved
- **Rationale**: 크레이트 루트의 `pub(crate) fn headless()`(`lib.rs:1415`)와 테스트에서 `FrameCtx`를 조립하는 선례(`lib.rs:2712`, `preedit_layer.rs:799`)가 이미 있다. 같은 파일 안에 두면 private `bind_key`를 프로덕션 표면 추가 없이 직접 관찰할 수 있다.
  - **양방향 고정**: ① 아틀라스가 그대로면 N 프레임 record 후에도 키 불변(재생성 0회) ② 아틀라스 교체 후 첫 record에서 키 변경(재생성 1회). ①이 G1 유휴-CPU 회귀를, ②가 본 버그를 막는다.
  - 기각: **순수 키 함수 단위 테스트만** — GPU 없이 빠르지만 "레이어가 실제로 재생성하는가"를 증명하지 못한다. 키는 맞는데 호출부가 안 쓰는 회귀를 놓친다.
  - 어댑터 없는 환경에서는 `headless()`가 `None`을 주며 테스트가 skip한다 — 크레이트의 기존 관례.
- **테스트 계약 4항** (L2-reviewer 지적 — 이것 없이는 테스트가 공허하게 통과한다):
  1. **세 레이어 각각의 조기 반환을 통과시킬 것** — 모델을 안 채우면 바인드 그룹이 애초에 안 만들어져 어떤 단정이든 **공허하게 통과**한다:
     - `SidebarTextPipeline::draw` — 빈 텍스트에서 반환 (`sidebar_row.rs:899-901`). 추가로 `TabBar::record`는 `left_inset ≤ 0`이나 뷰포트 0에서 반환 (`sidebar_row.rs:1099-1102`).
     - `OverlayLayer::record` — `!visible`에서 반환 (`overlay_layer.rs:301`).
     - `PreeditLayer::record` — **가장 엄격하다.** `preedit_geom`이 `None`이면 반환하는데(`preedit_layer.rs:468-480`), 그 판정이 `snap.at_bottom`·`cursor.col/row`·`cols/rows`·`ctx.cell`·양쪽 inset에 모두 걸린다. 기본 크기 스냅샷으로는 조용히 빠져나간다 — 기존 선례(`preedit_layer.rs:769-774`)가 10×2 그리드를 쓰는 이유가 이것이다.
  1b. **사이드바 테스트는 `reset_shared_model()`을 먼저 호출할 것** (`sidebar_row.rs:619`). 모델이 `thread_local!` 싱글턴이라(`sidebar_row.rs:499-501`) 직렬 실행되는 테스트 간에 `layout`/`MetaSource`가 샌다. 기존 사이드바 테스트 전부가 이 관례를 따른다(1568, 1579, 1586, 1615, 1648, 1747, 1758).
  2. **`epoch`은 부등호만 단정할 것.** 프로세스 전역 `AtomicU64`는 `cargo test`의 병렬 스레드가 공유하므로 `assert_eq!(atlas.epoch(), 0)` 류는 flaky하다. `assert_ne!(a.epoch(), b.epoch())`만 결정적이다.
  3. **①의 단정 대상은 "바인드 그룹 재생성 0회"로 한정할 것** — "GPU 작업 0회"가 아니다. overlay/preedit는 이미 프레임마다 인스턴스 버퍼를 `create_buffer_init`으로 새로 만든다(`overlay_layer.rs:323-336`, `preedit_layer.rs:503-510`). 그건 이 스펙의 범위 밖이며, 단정을 넓게 쓰면 누군가 그것까지 고치려다 범위를 터뜨린다.
  4. **관찰 지점은 private 상태** — 프로덕션 표면을 넓히지 않는다. 그래서 테스트가 같은 파일 안에 있어야 한다.
  5. ⚠️ **초안 정정 (Gate 3 심사에서 발견)**: 초안은 관찰 지점을 `bind_key` **하나로** 못박았는데, 그러면 계약 3항("재생성 0회")을 **표현할 수 없다** — 키 동등성은 매 프레임 재생성해도 그대로 성립하기 때문이다. 실제로 초안대로 작성된 idle 테스트 3개는 short-circuit을 통째로 지워도 전부 통과했다(변이 검증으로 확인). 두 조항은 **동시에 만족 불가능**했다.
     → 정정: 관찰 지점에 `#[cfg(test)] rebuilds: u32` 카운터를 **추가로** 허용한다. `cfg(test)`이므로 프로덕션 표면은 그대로이고(계약 4항의 취지 유지), 계약 3항이 비로소 검증 가능해진다. idle 테스트는 `rebuilds == 1`을 단정한다.

### D8: `Atlas::new`가 `None`을 반환하는 경로는 **명시적으로 "옛 아틀라스 유지, 바인드 그룹 불변"** 으로 정의한다
- **Status**: resolved
- **Rationale**: `apply_config`는 `self.cell_size`·`self.ppem`·`self.ligatures`를 **먼저** 갱신하고(`lib.rs:641-643`) 그 뒤 fallible한 `Atlas::new`를 호출한다(`lib.rs:654`). 할당이 실패하면 아틀라스는 **옛 ppem으로 래스터화된 상태로 남는데** `last_snapshot = None`(`lib.rs:677`)이 새 셀 기하로 재셰이핑을 강제한다 → 글리프가 잘못된 크기로 그려진다.
  - 이 스펙에서의 **정의된 동작**: `epoch`이 안 바뀌므로 세 레이어는 바인드 그룹을 유지한다. 이는 **올바르다** — 텍스처가 살아 있고 뷰도 유효하기 때문이다. 크기 불일치는 아틀라스 할당 실패라는 **별개의 선행 결함**이며 이 스펙은 그것을 고치지 않는다.
  - 요구되는 것은 코드 변경이 아니라 **이 경로가 우연이 아니라 의도임을 기록**하는 것이다. D2가 `cell`을 키에서 뺀 근거가 정확히 여기에 걸려 있으므로, 명시하지 않으면 다음 사람이 `cell`을 "안전망"이라며 되돌린다.
  - Known Gaps에 선행 결함(ppem/cell 불일치)을 기록한다.

### D7: 그리드 경로(`lib.rs:422`/`666`)는 이 수정에서 건드리지 않는다
- **Status**: resolved
- **Rationale**: 그리드는 오늘 올바르게 동작한다 — `apply_config`가 바인드 그룹을 손수 재생성하고 `last_snapshot = None`으로 재셰이핑까지 강제한다. 다만 그것은 **불변식이 아니라 우연**이다(교체 사이트가 하나뿐이라서 성립). 수정 후 네 소비자 중 셋은 자기방어하고 하나는 안 하는 비대칭이 남는다 → Known Gaps에 명시적으로 기록한다.
  - 기각: **그리드도 함께 키 기반으로 전환** — 유일하게 동작 중인 경로를 건드려 diff와 회귀 표면을 넓힌다. 두 번째 교체 사이트가 실제로 생길 때 하는 것이 옳다.
  - 기각: **침묵** — 다음에 교체 사이트를 추가하는 사람에게 아무 단서도 남지 않는다. 같은 버그가 그리드에서 재발한다.

## Constraints
- **바인드 그룹 재생성은 정확히 텍스처 교체 시점에만** 일어나야 한다. 매 프레임 재생성은 `docs/architecture.md` G1이 이미 갚은 유휴-CPU 부채의 회귀다.
  - 범위 주의: 이 제약은 **바인드 그룹에만** 적용된다. overlay/preedit가 이미 프레임마다 인스턴스 버퍼를 새로 만드는 것(`overlay_layer.rs:323-336`, `preedit_layer.rs:503-510`)은 기존 동작이며 이 스펙의 범위 밖이다 — 건드리지 말 것.
- `apply_config`의 **호출 빈도·에코 억제 로직을 바꾸지 않는다** (`terminal_app.rs:226-238`). 이 스펙은 교체가 일어난다는 전제 위에서 소비자 쪽만 고친다.
- **의존성 방향 불변** (`.claude/rules/dependency-direction.md`): 변경은 `renderer` 크레이트 내부에 머문다. `term-core`는 무관.
- **GUI 크레이트 추가 금지** (`.claude/rules/no-gui-crates.md`) — 새 의존성 없이 기존 wgpu 표면만 사용한다.
- 어댑터/폰트가 없는 환경에서 테스트는 **fail이 아니라 skip** (크레이트 기존 관례, `lib.rs:1411-1414` 주석).
- 검증 게이트: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.
- `no unwrap()` outside tests (CLAUDE.md).

## Known Gaps
- **G-a. 그리드 바인드 그룹은 키 기반 자기방어가 없다** (D7). `lib.rs:654`의 `apply_config`가 유일한 아틀라스 교체 사이트라는 사실에만 의존해 올바르다. 두 번째 교체 사이트가 추가되면 그리드에서 동일 버그가 재발한다 — 그때 D2 키를 그리드에도 적용할 것.
- **G-b. `apply_config`는 폰트 크기가 안 바뀌어도 아틀라스를 항상 재생성한다.** 사이드바 드래그마다 2048² 텍스처 재할당 + 전체 재래스터화를 문다. 이 스펙은 **정합성 결함만 고치고 낭비는 남긴다** — 별도 최적화 과제(교체 필요 여부를 ppem/폰트셋 변화로 판정). ⚠️ 그 최적화가 만약 *구조체를 유지한 채 내부 텍스처만 교체*하는 모양(`Atlas::resize()`)이 된다면 **D1의 팩토리 스탬프 지점을 반드시 경유해야 한다**. 안 그러면 이 버그가 그대로 재발한다.
- **G-c. Intra-frame eviction은 여전히 미해결이다** (D5 범위 한정, `atlas.rs:61-67`이 이미 문서화). 한 프레임 안에서 `pack`이 eviction을 일으키면 이미 발행된 인스턴스의 UV가 무효화되고, `draw`가 `record` 뒤에 `ctx.atlas`를 읽는 순서 때문에 **깨진 프레임이 키를 정상으로 확정한다.** 2048² 아틀라스가 확률을 실용 범위 밖으로 밀어낸 상태에 의존 중. 이 스펙은 손대지 않는다.
- **G-d. `Atlas::new` 실패 시 ppem/cell 불일치** (D8). `apply_config`가 `cell_size`·`ppem`을 fallible한 `Atlas::new`보다 **먼저** 갱신하므로(`lib.rs:641-643` vs `654`), 할당 실패 시 옛 ppem 아틀라스를 새 셀 기하로 재셰이핑한다 → 잘못된 크기의 글리프. 바인드 그룹 관점에서는 올바른 동작이라 이 스펙은 그대로 두지만, **선행 결함으로 기록**한다.
- **G-e. L2 checkpoint 잔여**: 없음 (Unresolved Sweep 결과 unresolved/provisional 0건).
- **G-f. 테스트는 프록시를 관찰할 뿐, 바인드 그룹 자체를 관찰하지 못한다** (Gate 3 2차 심사에서 발견, 변이 검증으로 확인).
  테스트가 볼 수 있는 것은 `bind_key`와 `#[cfg(test)] rebuilds` 카운터뿐이고, 둘 다 **실제 `wgpu::BindGroup` 객체가 교체됐는지**를 직접 말해주지 않는다. 실측한 결과:
  - ✅ 잡히는 변이: 키가 교체를 감지 못함(`epoch`→`generation` 되돌리기) · 매 프레임 재생성(short-circuit 삭제) · `ensure_text_bind` 호출 누락 — 셋 다 세 레이어 전부에서 테스트 실패로 잡힌다.
  - ❌ 안 잡히는 변이: `gpu.text_bind = build_text_bind(...)` **한 줄만** 지우고 키 갱신과 카운터 증가는 남기는 경우. 이 변이는 프로덕션 버그와 동일한 상태(옛 텍스처를 가리키는 바인드 그룹 + 새 아틀라스 기준 UV)를 만드는데도 전부 통과한다.
  - 원인은 근본적이다: 배정문 **옆**에 있는 어떤 프록시든 변이가 분리할 수 있다. `wgpu::BindGroup`은 안정적인 동일성 비교 수단을 노출하지 않는다.
  - 이걸 진짜로 닫는 유일한 방법은 **픽셀 리드백 테스트** — 아틀라스 A로 텍스트를 오프스크린에 그리고, B로 교체해 다시 그린 뒤, 출력이 같은 글리프인지 비교. 크레이트에 이미 오프스크린 타깃 선례가 있으므로 불가능하지 않지만 **이 스펙의 범위를 넘는 별도 과제**다.
  - 현 상태의 정직한 요약: 구현은 세 지점 모두에서 `text_bind`를 올바르게 재대입한다(코드 리뷰로 확인). 테스트는 **현실적인 회귀는 잡고, 인위적인 한 줄 삭제는 못 잡는다.**

## Requirements

경계(boundary) 분해 기준: **`Atlas`(정체성 생산자) ↔ 각 텍스트 레이어(소비자)**. 생산 측 요구사항마다 소비 측 대응 요구사항이 존재해야 한다.

### R0: 라이브 config 리로드가 아틀라스를 교체해도 세 텍스트 레이어의 글자가 깨지지 않는다

#### R0.1: 사이드바 폭 드래그 후에도 카드 텍스트가 올바른 글리프를 유지한다
- **Given**: 터미널이 실행 중이고 사이드바에 탭 카드가 하나 이상 렌더링돼 있다
- **When**: 사용자가 사이드바 가장자리를 드래그해 폭이 config에 저장되고, 워처가 그 쓰기를 되돌려주어 `settings::apply` → `Renderer::apply_config`가 돌아 아틀라스가 교체된다 (폰트 크기는 불변)
- **Then**: 카드의 제목·서브라인이 드래그 이전과 **동일한 글리프**로 계속 렌더링된다

#### R0.2: 동일 조건에서 ai-monitor 오버레이 텍스트가 깨지지 않는다
- **Given**: ai-monitor 오버레이가 표시 중(`visible == true`)이고 로그 라인이 하나 이상 있다
- **When**: 위와 같은 아틀라스 교체가 일어난다
- **Then**: 오버레이 로그 텍스트가 올바른 글리프로 계속 렌더링된다

#### R0.3: 동일 조건에서 IME preedit 텍스트가 깨지지 않는다
- **Given**: 한글 조합 중이라 preedit 문자열이 표시되고 있다
- **When**: 위와 같은 아틀라스 교체가 일어난다
- **Then**: preedit 문자열이 올바른 글리프로 계속 렌더링된다

#### R0.4: 본문 그리드는 회귀하지 않는다
- **Given**: 그리드에 텍스트가 렌더링돼 있다
- **When**: 위와 같은 아틀라스 교체가 일어난다
- **Then**: 그리드 렌더링은 수정 전과 동일하다 (D7 — 이 스펙은 그리드 경로를 변경하지 않는다)

---

### R1: `Atlas`가 텍스처 정체성 `epoch`을 **출생 지점에서** 발급한다 *(D1 — 생산 측)*

#### R1.1: 텍스처·뷰·샘플러 생성이 단일 팩토리로 모인다
- **Given**: 현재 `create_texture`/`create_view`/`create_sampler`가 `Atlas::new` 본문에 인라인돼 있다 (`atlas.rs:119`, `133-139`)
- **When**: 이 세 호출을 하나의 private 팩토리 함수로 추출한다
- **Then**: `renderer` 크레이트 전체에서 아틀라스 텍스처를 만드는 지점이 **정확히 그 함수 하나**가 되고, `Atlas::new`는 그것을 호출할 뿐이다

#### R1.2: 팩토리가 전역 카운터에서 `epoch`을 뽑아 함께 반환한다
- **Given**: 프로세스 전역 `static NEXT_ATLAS_EPOCH: AtomicU64`가 존재한다
- **When**: R1.1의 팩토리가 호출된다
- **Then**: 팩토리는 새 텍스처·뷰·샘플러와 함께 `fetch_add`로 얻은 **그 호출에 고유한** `epoch` 값을 반환하고, `Atlas`가 그것을 필드로 보관한다

#### R1.3: 서로 다른 두 `Atlas`는 절대 같은 `epoch`을 갖지 않는다
- **Given**: 동일한 device·fontset·ppem
- **When**: `Atlas::new`를 두 번 호출해 `a`, `b`를 만든다
- **Then**: `a.epoch() != b.epoch()` (값 자체는 단정하지 않는다 — 전역 카운터는 병렬 테스트가 공유하므로 부등호만 결정적이다)

#### R1.4: `epoch` 접근자가 `generation()`과 동형으로 노출된다
- **Given**: `Atlas`가 이미 `generation(&self) -> u64` 접근자를 갖는다
- **When**: 소비 측 레이어가 정체성을 읽어야 한다
- **Then**: 형제 접근자와 **동일한 가시성**으로 `fn epoch(&self) -> u64`를 노출한다
- ⚠️ **초안 정정 (Gate 3 심사에서 발견)**: 초안은 `generation()`이 `pub(crate)`(`atlas.rs:161`)라고 적고 `pub(crate) fn epoch`을 요구했다. **둘 다 사실이 아니다** — 실제 코드는 `pub fn generation`이다. "동형으로"와 "`pub(crate)`"가 서로 모순이었고, 구현은 동형 쪽(=`pub`)을 택했다. 이 요구사항의 취지는 가시성 수준 자체가 아니라 **형제 접근자와의 일관성**이므로, 구현(`pub fn epoch`)이 옳고 초안 문구가 틀렸다. 소비자는 전부 크레이트 내부다.

#### R1.5: eviction은 `epoch`을 바꾸지 않는다
- **Given**: 아틀라스 `a`의 `epoch`을 기록해 둔다
- **When**: `a.evict_for_test()`를 호출해 eviction을 강제한다 (`atlas.rs:328`)
- **Then**: `a.epoch()`은 **불변**이고 `a.generation()`만 증가한다 — 두 값의 직교성이 고정된다

---

### R2: `sidebar_row`의 바인드 키가 `(format, epoch)`로 교정된다 *(D2, D4 — 소비 측)*

#### R2.1: 키에서 `generation`과 `cell`이 제거된다
- **Given**: `bind_key: Option<(TextureFormat, u64, [u32; 2])>`이고 키가 `(format, generation, cell)`이다 (`sidebar_row.rs:820`, `904-908`)
- **When**: D2를 적용한다
- **Then**: `bind_key: Option<(TextureFormat, u64)>`이고 키가 `(ctx.format, ctx.atlas.epoch())`이다 — 필드가 **줄어든다**

#### R2.2: 아틀라스 교체가 사이드바 바인드 그룹 재생성을 유발한다
- **Given**: 사이드바가 비어 있지 않은 텍스트를 한 번 이상 record 해 `bind_key`가 `Some(k1)`이다
- **When**: `ctx.atlas`가 새 `Atlas` 인스턴스로 바뀐 뒤 다시 record 한다
- **Then**: `bind_key != Some(k1)`이고 바인드 그룹이 새 아틀라스의 뷰·샘플러를 가리킨다

#### R2.3: 아틀라스가 그대로면 재생성이 일어나지 않는다
- **Given**: 사이드바가 한 번 record 되어 `bind_key == Some(k1)`이다
- **When**: 동일 아틀라스로 N(≥3) 프레임을 더 record 한다
- **Then**: `bind_key`가 N 프레임 내내 `Some(k1)`으로 불변이다 — 바인드 그룹 재생성 0회 (G1 유휴-CPU 제약)

#### R2.4: eviction만으로는 재생성되지 않는다
- **Given**: 사이드바가 record 되어 `bind_key == Some(k1)`이다
- **When**: 같은 아틀라스에 `evict_for_test()`로 eviction을 일으킨 뒤 다시 record 한다
- **Then**: `bind_key == Some(k1)`으로 불변 — eviction은 같은 텍스처를 재사용하므로 재생성이 바이트 동일한 결과만 낳는다 (D2)

---

### R3: overlay/preedit가 아틀라스 바인드 그룹을 `ensure_gpu`에서 분리한다 *(D3 — 소비 측)*

#### R3.1: `Gpu`가 `text_bgl`을 보관한다
- **Given**: `text_bgl`이 스택 지역 변수로 만들어져 `build_pipeline`에 넘겨진 뒤 버려진다 (`overlay_layer.rs:195`, `preedit_layer.rs:290`)
- **When**: D3 규칙 1을 적용한다
- **Then**: `struct Gpu`에 `text_bgl: wgpu::BindGroupLayout` 필드가 생기고 이후 프레임에서 바인드 그룹을 만들 수 있다

#### R3.2: `bind_key`가 `Gpu` 안에 산다
- **Given**: `ensure_gpu`가 포맷 불일치 시 `self.gpu = Some(Gpu {…})`로 구조체를 통째로 교체한다 (`overlay_layer.rs:248-256`)
- **When**: `bind_key`를 `Gpu`의 필드로 둔다
- **Then**: 포맷 변경 시 `bind_key`가 새 `Gpu`와 함께 `None`으로 리셋되어, 캐시된 바인드 그룹이 교체된 `uniform` 버퍼를 가리키는 상태가 **구조적으로 불가능**해진다

#### R3.3: `text_bind`는 비-`Option`으로 남고 재대입된다
- **Given**: `text_bind: wgpu::BindGroup`이 draw 시점에 무조건 소비된다 (`overlay_layer.rs:361`, `preedit_layer.rs:540`)
- **When**: `ensure_text_bind`가 키 불일치를 감지한다
- **Then**: 필드를 **재대입**할 뿐 `Option`으로 바꾸지 않는다 — draw 지점에 `unwrap()`/`expect()`가 생기지 않는다 (CLAUDE.md no-`unwrap()`)

#### R3.4: `record`가 `ensure_gpu` 다음에 `ensure_text_bind`를 호출한다
- **Given**: `ensure_text_bind`는 `ensure_gpu`가 만든 `text_bgl`에 의존한다
- **When**: 레이어가 프레임을 record 한다
- **Then**: 호출 순서가 `ensure_gpu` → `ensure_text_bind` → draw이고, preedit는 조기 반환 뒤인 기존 `ensure_gpu` 지점(`preedit_layer.rs:481`)을 그대로 쓴다

#### R3.5: 두 레이어 모두 교체 시 재생성, 무변경 시 유지
- **Given**: 오버레이가 `visible`이고 preedit가 조합 중이라 각각 한 번 이상 record 됐다
- **When**: 아틀라스가 교체된 뒤 / 교체 없이 다시 record 된다
- **Then**: 교체 시에는 `bind_key`가 바뀌고, 무변경 시에는 N 프레임 불변이다 (R2.2/R2.3과 동일한 계약)

---

### R4: 무효화의 작용 범위가 바인드 그룹으로 한정된다 *(D5, D8)*

#### R4.1: 인스턴스·셰이핑 캐시에 무효화 장치를 추가하지 않는다
- **Given**: 세 레이어가 `record`마다 인스턴스를 새로 만든다 (`sidebar_row.rs:1155`의 `text.clear()` 이후 `push_glyphs`)
- **When**: 이 스펙을 구현한다
- **Then**: 그리드의 `last_snapshot = None`(`lib.rs:677`)에 해당하는 강제 재셰이핑 장치가 레이어에 **추가되지 않는다**

#### R4.2: 기존의 프레임당 인스턴스 버퍼 생성은 건드리지 않는다
- **Given**: overlay/preedit가 이미 매 프레임 `create_buffer_init`으로 인스턴스 버퍼를 만든다 (`overlay_layer.rs:323-336`, `preedit_layer.rs:503-510`)
- **When**: 이 스펙을 구현한다
- **Then**: 그 동작은 그대로 남는다 — 이 스펙의 "재생성 0회" 제약은 **바인드 그룹에만** 적용된다

#### R4.3: 아틀라스 할당 실패 시 바인드 그룹을 유지한다
- **Given**: `apply_config`에서 `Atlas::new`가 `None`을 반환해 옛 아틀라스가 유지된다 (`lib.rs:654`)
- **When**: 세 레이어가 다음 프레임을 record 한다
- **Then**: `epoch`이 불변이므로 바인드 그룹이 유지되고, 이는 **올바르다** — 텍스처가 살아 있고 뷰도 유효하다 (D8). 이 경로의 ppem/cell 불일치는 별개 선행 결함으로 G-d에 기록돼 있으며 이 스펙은 고치지 않는다

---

### R5: 회귀 테스트가 양방향을 고정한다 *(D6)*

#### R5.1: 세 레이어 각각에 in-file GPU 테스트가 존재한다
- **Given**: 크레이트 루트에 `pub(crate) fn headless()`가 있다 (`lib.rs:1415`)
- **When**: 테스트를 작성한다
- **Then**: `sidebar_row.rs`·`overlay_layer.rs`·`preedit_layer.rs` 각각의 `#[cfg(test)] mod tests` 안에 테스트가 있고, private `bind_key`를 직접 관찰하며, 프로덕션 표면을 넓히지 않는다

#### R5.2: 수정 전 코드에서 실패한다
- **Given**: `epoch` 도입 전의 현재 코드
- **When**: R2.2/R3.5의 "교체 시 재생성" 테스트를 돌린다
- **Then**: `bind_key`가 그대로라 **실패한다** — 테스트가 실제 버그를 잡는다는 증거

#### R5.3: 조기 반환을 통과하도록 모델을 채운다
- **Given**: 세 레이어에 각기 다른 조기 반환이 있다 — 사이드바 빈 텍스트(`sidebar_row.rs:899-901`)와 `left_inset ≤ 0`(`1099-1102`), 오버레이 `!visible`(`overlay_layer.rs:301`), preedit `preedit_geom → None`(`preedit_layer.rs:468-480`)
- **When**: 테스트가 레이어를 record 한다
- **Then**: 각 레이어의 모델·`FrameCtx`가 조기 반환을 통과하도록 채워져 있고, preedit는 선례(`preedit_layer.rs:769-774`)처럼 충분한 크기의 그리드 스냅샷을 쓴다 — 공허한 통과가 불가능하다

#### R5.4: 사이드바 테스트가 싱글턴을 리셋한다
- **Given**: 사이드바 모델이 `thread_local!` 싱글턴이다 (`sidebar_row.rs:499-501`)
- **When**: 사이드바 테스트가 시작된다
- **Then**: 기존 테스트들의 관례대로 `reset_shared_model()`(`sidebar_row.rs:619`)을 먼저 호출한다

#### R5.5: 어댑터·폰트가 없으면 skip 한다
- **Given**: GPU 어댑터나 시스템 폰트가 없는 환경
- **When**: 테스트를 돌린다
- **Then**: `headless()`가 `None`을 주면 테스트는 **fail이 아니라 조용히 반환**한다 (크레이트 기존 관례)

---

### R6: 범위 밖 경로가 변경되지 않고, 그 사실이 기록된다 *(D7)*

#### R6.1: 그리드 바인드 그룹 코드가 그대로다
- **Given**: `lib.rs:422`(초기)와 `lib.rs:656-673`(`apply_config` 재생성)의 그리드 경로
- **When**: 이 스펙을 구현한다
- **Then**: 두 지점의 동작이 변경되지 않는다

#### R6.2: 비대칭이 코드에 기록된다
- **Given**: 수정 후 네 아틀라스 소비자 중 셋은 키로 자기방어하고 그리드만 안 한다
- **When**: 구현이 끝난다
- **Then**: `apply_config`의 그리드 바인드 그룹 재생성 지점에, 다음 아틀라스 교체 사이트를 추가하는 사람이 읽을 주석이 남는다 (G-a와 대응)

#### R6.3: 팩토리 우회 금지가 기록된다
- **Given**: G-b의 최적화(불필요한 아틀라스 재생성 제거)가 `Atlas::resize()` 모양이 될 수 있다
- **When**: R1.1의 팩토리를 만든다
- **Then**: 팩토리에 "아틀라스 텍스처를 만드는 유일한 지점 — 우회하면 `epoch`이 얼어붙어 stale 바인드 그룹 버그가 재발한다"는 주석이 달린다

---

### 커버리지 자기점검

| Decision | 커버하는 Requirement |
|---|---|
| D1 (팩토리 스탬프 `epoch`) | R1.1–R1.5 |
| D2 (키 = `(format, epoch)`) | R2.1–R2.4 |
| D3 (overlay/preedit 분리 + 구조 4항) | R3.1–R3.5 |
| D4 (sidebar 키 교정) | R2.1, R2.2 |
| D5 (바인드 그룹 한정) | R4.1, R4.2 |
| D6 (양방향 테스트 + 계약 5항) | R5.1–R5.5 |
| D7 (그리드 불변 + 기록) | R0.4, R6.1–R6.3 |
| D8 (`Atlas::new` 실패 경로) | R4.3 |

- **고아 결정 없음** — D1–D8 전부 최소 하나의 요구사항으로 추적된다.
- **경계 점검** — 생산 측(R1: `Atlas`의 `epoch` 발급)에 대해 소비 측 대응이 세 레이어 모두 존재한다(R2 사이드바, R3 overlay/preedit). 소비 측만 있고 생산 측이 없는 요구사항은 없다.
- **GWT 완전성** — 모든 하위 요구사항이 Given/When/Then 세 필드를 갖는다.
- **양방향 대칭** — "교체 시 재생성"(R2.2/R3.5)마다 대응하는 "무변경 시 유지"(R2.3/R2.4/R3.5)가 있다.

## Tasks

파일 경계가 곧 병렬 경계다. T1이 정체성을 발급하는 **생산 측 인프라**이고, T2–T4는 서로 다른 파일에 사는 **소비 측 슬라이스**라 T1 이후 완전 병렬이다. 각 소비 측 태스크는 "수정 + 그 레이어의 회귀 테스트"를 함께 담아 인터페이스를 태스크 내부에서 닫는다.

### T1: `Atlas`에 텍스처 팩토리와 `epoch`을 도입한다 [infra]
- **Fulfills**: R1.1, R1.2, R1.3, R1.4, R1.5, R6.3
- **Depends on**: (none)
- **Files**: `crates/renderer/src/atlas.rs`
- 텍스처·뷰·샘플러 생성(`atlas.rs:119`, `133-139`)을 단일 private 팩토리로 추출하고 전역 `AtomicU64`에서 `epoch`을 뽑아 함께 반환. `epoch` 필드 + `pub fn epoch()` 접근자 추가 (형제 `generation()`과 동일 가시성 — R1.4 정정). 팩토리에 우회 금지 주석(R6.3). `atlas.rs` 안에 `epoch` 고유성(R1.3)과 eviction 무영향(R1.5) 테스트 추가 — 둘 다 GPU 불필요하거나 기존 `headless()` 관례를 따름.
- ⚠️ `Atlas::new`의 공개 시그니처를 바꾸지 말 것 — 바뀌면 `lib.rs` 호출부(`654`, 초기화 지점)까지 번져 T5와 파일이 겹친다.

### T2: `sidebar_row`의 바인드 키를 `(format, epoch)`로 교정한다 [vertical]
- **Fulfills**: R0.1, R2.1, R2.2, R2.3, R2.4, R4.1, R5.1, R5.2, R5.3, R5.4, R5.5
- **Depends on**: T1
- **Files**: `crates/renderer/src/sidebar_row.rs`
- `bind_key`를 `Option<(TextureFormat, u64)>`로 좁히고(`sidebar_row.rs:820`) 키를 `(ctx.format, ctx.atlas.epoch())`로 교체(`904-908`). 같은 파일 `mod tests`에 교체→재생성 / 무변경→유지 / eviction→유지 3종 테스트. `reset_shared_model()` 선행 호출, 빈 텍스트·`left_inset ≤ 0` 조기 반환 통과.
- ← T3, T4와 **병렬**

### T3: `overlay_layer`의 아틀라스 바인드 그룹을 분리한다 [vertical]
- **Fulfills**: R0.2, R3.1, R3.2, R3.3, R3.4, R3.5, R4.2, R5.1, R5.2, R5.3, R5.5
- **Depends on**: T1
- **Files**: `crates/renderer/src/overlay_layer.rs`
- `struct Gpu`에 `text_bgl` 필드 추가(`123-131`), `text_bgl`을 지역 변수에서 필드로 승격(`195`), `bind_key`를 `Gpu` 안에 배치, `ensure_text_bind`를 `ensure_gpu` 뒤에 호출. `text_bind`는 비-`Option` 유지·재대입. 프레임당 `create_buffer_init`(`323-336`)은 **건드리지 않는다**. `visible == true` + 로그 라인 채운 상태로 테스트.
- ← T2, T4와 **병렬**

### T4: `preedit_layer`의 아틀라스 바인드 그룹을 분리한다 [vertical]
- **Fulfills**: R0.3, R3.1, R3.2, R3.3, R3.4, R3.5, R4.2, R5.1, R5.2, R5.3, R5.5
- **Depends on**: T1
- **Files**: `crates/renderer/src/preedit_layer.rs`
- T3와 동형의 변경(`212-220`, `290`, `243-245`, `481`). 테스트는 `preedit_geom`이 `None`을 반환하지 않도록 선례(`769-774`)처럼 충분한 크기의 그리드 스냅샷과 조합 중 상태를 갖춰야 한다 — 세 레이어 중 조기 반환 조건이 가장 엄격하다.
- ← T2, T3와 **병렬**

### T5: 그리드 경로의 비대칭을 주석으로 기록한다 [infra]
- **Fulfills**: R0.4, R4.3, R6.1, R6.2
- **Depends on**: T1
- **Files**: `crates/renderer/src/lib.rs` (주석만)
- `apply_config`의 그리드 바인드 그룹 재생성 지점(`lib.rs:656-673`)에, 세 레이어는 `epoch` 키로 자기방어하지만 그리드는 "`apply_config`가 유일한 교체 사이트"라는 사실에만 의존한다는 주석을 남긴다(G-a). 아울러 `Atlas::new`가 `None`일 때 바인드 그룹 유지가 **의도된 동작**임을 기록한다(R4.3/G-d). **동작 변경 없음 — 주석 전용 태스크다.**
- ← T2, T3, T4와 **병렬** (유일하게 `lib.rs`를 건드리는 태스크)

### DAG

```
T1 (atlas.rs) [infra]
 ├─→ T2 (sidebar_row.rs)  ┐
 ├─→ T3 (overlay_layer.rs)│ 4-way 병렬 — 파일 교집합 없음
 ├─→ T4 (preedit_layer.rs)│
 └─→ T5 (lib.rs, 주석)    ┘
```

**병렬성 검증** (세 조건 모두 충족):
1. **파일 중복 없음** — T2/T3/T4/T5가 각각 `sidebar_row.rs`/`overlay_layer.rs`/`preedit_layer.rs`/`lib.rs` 단독. T1은 `atlas.rs` 단독이며 `Atlas::new` 시그니처를 보존하므로 `lib.rs`로 번지지 않는다.
2. **인터페이스 의존 없음** — T2–T5는 모두 T1이 노출한 `epoch()` 하나만 소비하고 서로의 출력을 쓰지 않는다.
3. **모델 의존 없음** — 공유 상태 없음. 각 레이어가 자기 바인드 그룹만 소유한다.

순환 없음. 최종 검증 태스크를 두지 않는 것은 실행 파이프라인이 홀리스틱 검증을 담당하기 때문이다.

## External Dependencies

### Pre-work
- (none) — 워크트리 `terminal-fix-sidebar-atlas-swap`이 `fix/sidebar-atlas-swap` 브랜치로 이미 준비돼 있고, 새 크레이트 의존성이 없다.

### Post-work
- **`HANDOFF-sidebar-atlas-swap.md`를 정리한다** — 리포지토리 루트에 untracked 상태로 남아 있고 gitignore 대상이 아니다. 이 스펙이 그 내용을 모두 흡수했으므로 커밋 전에 삭제한다.
- **눈으로 확인**: `/run-term` → 사이드바 가장자리를 드래그 → 카드 글자가 유지되는지. ai-monitor 오버레이를 켠 채로도, 한글 조합 중에도 같은 확인.
- **게이트**: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.
- G-a/G-b/G-c/G-d는 **의도적으로 남긴 후속 과제** — 필요하면 별도 `/specify`로 스코프할 것.
