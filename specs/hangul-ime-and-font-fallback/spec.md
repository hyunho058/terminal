# Spec: hangul-ime-and-font-fallback

## Meta
- **Created**: 2026-07-19
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-19

## Goal
한글 입력이 깨지는 두 결함을 고친다. 터미널에 한글을 타이핑하면 (a) 조합된 음절이
만들어지고 (b) 실제 한글 글리프로 렌더된다.

- **결함① 렌더 — 폰트 폴백 부재 (`renderer`)**: `FONT_CANDIDATES`는 Menlo/SFNSMono뿐
  (`crates/renderer/src/lib.rs:33-36`), `build_glyph_instances`는 단일 `FontRef`로만
  셰이핑(`lib.rs:831`) → 한글은 glyph id 0(.notdef); `get_or_insert_styled`는 0 가드 없이
  .notdef 박스를 그림(`atlas.rs:136-150`); `glyph_id`는 단일 charmap만 조회(`glyph.rs:57-62`).
- **결함② 입력 — IME 미연결 (`app`)**: 창을 `default_attributes`로만 생성하고
  `set_ime_allowed(true)` 없음(`terminal_app.rs:120-123`); `window_event`에
  `WindowEvent::Ime` arm 없음(`terminal_app.rs:342-404`); `key_event.text`만 PTY로 보냄
  (`terminal_app.rs:393-404`, `input.rs:65-73`) → 조합 불가, 낱자만 감.

## Non-goals
- 컬러 이모지 렌더링 (atlas는 R8Unorm 알파 전용 — 별도 feature).
- 일본어/중국어 전용 폰트 선정·튜닝 (폴백 *메커니즘*은 일반화하되 시드 폰트는 한글 커버리지 기준).
- 폰트 패밀리 설정 UI / config에서의 폴백 체인 커스터마이즈.
- `term-core`의 **그리드/VT 파서/스크롤백** 변경 — 한글 width=2 처리는 `grid.rs:7`의
  `unicode-width`로 이미 동작하고 테스트도 있다(`grid.rs:486-539`).
  ※ **예외**: `term-core/src/pty.rs`의 환경변수 주입은 범위 **안**이다 — 인터뷰의 인버전
  프로브가 결함③(로케일 미설정)을 찾아내 D15로 채택했다. 이 한 파일 외의 `term-core`
  변경은 이 스펙의 범위가 아니다. (D9의 "`term-core` 무변경"은 *preedit 렌더 경로*에
  한정된 서술이다.)
- 폰트 family→path 시스템 해석(CoreText 질의). 경로 상수 후보 목록 방식을 유지.
- SSH/원격, 다른 셸, 크로스플랫폼 (프로젝트 규칙 상 영구 non-goal).

## Confirmed Goal
`renderer`에 문자별 폰트 폴백 체인을 도입하고(프라이머리 Menlo + CJK 폴백
AppleSDGothicNeo; `GlyphKey`에 폰트 축 추가, `Atlas`가 폰트당 rasterizer 보유,
클러스터별로 charmap이 커버하는 첫 폰트로 셰이핑/래스터), `app`에 macOS IME를
연결한다(`set_ime_allowed(true)`, `WindowEvent::Ime` Commit→PTY write /
Preedit→커서 위치 인라인 렌더, 조합중 `KeyboardInput.text` 중복 전송 게이트).

**Done when**
1. `한글`을 타이핑하면 조합된 음절 바이트가 활성 PTY에 도달하고 자모가 중복 전송되지 않는다.
2. 조합중 문자열이 커서 위치에 인라인으로 보인다.
3. 한글 셀이 .notdef 박스가 아닌 실제 한글 글리프로 그려진다.
4. `cargo build` / `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` 전부 green.
5. 결함①②를 제목에 명시한 PR이 열려 있다.

## Research

### 결함① — 렌더 경로 (renderer)
- 폰트 후보는 2개, 둘 다 라틴 모노스페이스. 한글 커버리지 없음 (`crates/renderer/src/lib.rs:33-36`).
- `load_font()`는 첫 성공 경로 하나만 반환 → `Renderer.font: Arc<Vec<u8>>` 단일 필드 (`lib.rs:1001-1008`, `lib.rs:161`).
- `Atlas`는 rasterizer **1개**만 보유 (`atlas.rs:55`), `Atlas::new(device, font, index, ppem)` 시그니처 (`atlas.rs:69-74`).
- `GlyphKey { id, bold, italic }` — 폰트 축 없음 (`atlas.rs:16-21`). 폰트가 여럿이면 glyph id가 충돌한다.
- `get_or_insert_styled`는 `key.id == 0` 가드가 없어 .notdef 아웃라인을 그대로 래스터/팩 (`atlas.rs:136-150`).
- `GlyphRasterizer::glyph_id`는 단일 face의 charmap만 조회 (`glyph.rs:57-62`); `rasterize()`만 `glyph_id == 0`을 걸러냄 (`glyph.rs:66-72`) — 셰이핑 경로는 `rasterize_glyph_styled`를 직접 호출하므로 이 가드를 타지 않음.
- `build_glyph_instances`: 행 전체를 `String`으로 모아 `FontRef::from_index(font_data, 0)` **한 폰트**로 셰이핑 (`lib.rs:822-895`). 클러스터 pen은 `col * cw`로 **리셋**되므로(`lib.rs:882`) 셀 간 위치는 열 기준 — 폴백 폰트의 프로포셔널 advance는 클러스터 내부에서만 영향.
- 연속(continuation) 셀은 스킵하고 wide char는 2셀 간격으로 배치 (`lib.rs:849-862`) — 한글(width 2)은 그리드 레이아웃상 이미 정상.
- 다른 `DrawLayer`도 아틀라스로 텍스트를 그린다: `tab_bar.rs:408-409`, `overlay_layer.rs:264-265` — 둘 다 `atlas.glyph_id(c)` + `get_or_insert(gid)` (프라이머리 전용). 탭 제목은 cwd basename (`app/src/tabs.rs:138-150`)이라 **한글 디렉터리명이면 여기도 .notdef**.
- `apply_config`는 ppem 변경 시 `Atlas::new(.., self.font.clone(), 0, ..)`로 아틀라스를 재생성 (`lib.rs:535`) — 폰트 세트로 바뀌면 여기도 함께 수정 필요.
- 아틀라스는 1024² R8Unorm 알파 텍스처 + shelf packer + 가득 차면 전체 evict (`atlas.rs:47`, `182-200`). 한글은 글리프 수가 많아 evict 빈도가 올라갈 수 있음.
- 시스템 폰트 확인: `/System/Library/Fonts/AppleSDGothicNeo.ttc` 존재. `Hiragino Sans GB.ttc`도 존재(중국어).
- 렌더러 크레이트에 이미 `thiserror` 기반 `RendererError`가 있고 `NoFont(String)` variant 보유 (`lib.rs:38-52`).

### 결함② — 입력 경로 (app)
- 창 생성: `Window::default_attributes()` + title/size만. `set_ime_allowed` 호출 없음 (`terminal_app.rs:120-123`).
- `window_event` match에 `WindowEvent::Ime` arm 없음 → 기본 `_ => {}`로 버려짐 (`terminal_app.rs:346-434`).
- `KeyboardInput` arm이 `key_event.text`를 그대로 `KeyInput.text`에 담아 `on_key` → `encode_key` → PTY write (`terminal_app.rs:393-404`, `input.rs:65-73`).
- **winit 0.30.13 macOS 동작 확인** (`~/.cargo/registry/.../winit-0.30.13/src/platform_impl/macos/view.rs`):
  - `key_down`은 `ime_allowed`가 **true일 때만** `interpretKeyEvents`를 호출한다 (view.rs:466-468). 지금은 false라 IME가 아예 개입하지 못하고 낱자 text만 나온다 — 진단과 일치.
  - `had_ime_input`이면 `KeyboardInput`을 **큐에 넣지 않는다** (view.rs:479-496). 즉 `set_ime_allowed(true)`만 켜도 조합중/커밋 키의 중복 `KeyboardInput`은 winit이 이미 억제한다.
  - `do_command_by_selector`는 `ImeState::Committed`면 **early-return** — 주석에 "we'll end up sending it twice with some IMEs like Korean one"라고 명시 (view.rs:420-435). 한글 커밋용 Enter도 winit이 삼킨다.
  - 단, `Preedit` 상태에서의 `doCommandBySelector`(방향키 등)는 `forward_key_to_app=true`로 **KeyboardInput을 통과시킨다** — 앱의 게이트가 지나치게 넓으면 조합중 방향키를 잘못 삼킬 위험.
  - ASCII 입력은 marked text가 없어 `Commit`이 발생하지 않고 `KeyboardInput`으로 그대로 온다 (view.rs:411-414) → ASCII 경로는 무변경.
  - `Ime::Preedit(String::new(), None)`이 조합 취소/클리어의 합성 신호 (event.rs:748, view.rs:345).
- 확장 seam이 이미 있다: `InputRouter`/`OverlayHost` + `KeyInput { key, text, mods }` (`seam.rs:26-89`). 단 IME는 키가 아니라 별도 `WindowEvent`라 seam으로는 표현되지 않음.
- `DrawLayer` + 공유 모델 핸들 패턴이 확립돼 있다: `TabBar::new() -> (TabBar, TabModelHandle)`, `TabModelHandle(Rc<RefCell<TabBarModel>>)` (`tab_bar.rs:42-44, 224-232`). preedit 레이어가 그대로 복제할 형태.
- `CursorLayer`는 매 프레임 재기록되며 커서 위치를 `ctx.snapshot`의 `GridSnapshot::cursor`에서 읽는다 (`cursor_layer.rs:1-31`) — preedit 레이어도 커서 위치를 같은 경로로 얻을 수 있어 `term-core` 변경 불필요.
- 레이어 z-order: 글리프 z=0, `CURSOR_Z = 5`, 탭바 z=10 (`cursor_layer.rs:18`).

### 프로젝트 규약 / 검증
- 워크스페이스 4크레이트 + `ai-monitor`. 의존 방향 `common ← term-core ← renderer ← app` 강제 (`.claude/rules/dependency-direction.md`).
- `renderer`는 winit 무의존, `app`만 winit 0.30.13 보유 (`crates/renderer/Cargo.toml`, `crates/app/Cargo.toml`).
- 한글 width=2는 `term-core`가 `unicode-width`로 이미 처리 (`grid.rs:7`, `grid.rs:118-120`, 테스트 `grid.rs:486-516`).
- 시스템 폰트 의존 테스트의 기존 관행: `#[ignore = "depends on a system font path; run with --ignored"]` (`glyph.rs:131`).
- 검증 커맨드: `cargo build`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` (CLAUDE.md).
- 테스트 밖 `unwrap()` 금지인데 `glyph.rs:59`와 `glyph.rs:90`에 `unwrap()`이 이미 있음 (`new`에서 검증했다는 주석 근거) — 폰트 세트로 확장할 때 이 패턴을 늘리지 않도록 주의.

## Decisions

> **Complexity: Complex (5 signals)** — L2-reviewer 지적을 받아 Medium에서 재분류.
> ① 외부 리소스 로딩(55MB 시스템 폰트) ② 3+ 서브시스템(아틀라스/GPU · swash 셰이핑 ·
> winit IME · PTY) ③ 상태 머신(IME Disabled→Ground→Preedit→Committed)
> ④ **멀티 액터** — macOS IME는 우리가 읽을 수 없는 marked-text 상태를 들고 있고(D11),
> zsh/ZLE는 자기 폭 산술을 돌린다(D15). 이번 스펙의 실패 대부분이 이 두 경계에서 나왔다.
> ⑤ **데이터 스키마** — `GlyphKey`의 face 네임스페이스 축(D7) + `FontSet`/`FontFace` 신규 타입.
>
> 인터뷰 도중 인버전 프로브가 **세 번째 결함**(D15, 로케일)을 찾아내 범위가 세 번째
> 크레이트(`term-core`)까지 넓어졌고, L2-reviewer가 11건을 지적해 D1·D4·D5·D6·D7·D10·D11·D15를
> 개정하고 D16·D17·D18을 추가했다.

### D1: 폰트 세트 = 순서 있는 N-폰트 폴백 체인, **mmap 로드** (`renderer` 소유)
- **Status**: resolved *(L2-reviewer 지적 #10/#11 반영 개정)*
- `FONT_CANDIDATES`(프라이머리) 옆에 `FALLBACK_CANDIDATES`(한글)를 두고 로드 결과를
  `FontSet { faces: Vec<FontFace> }`(index 0 = 프라이머리)로 모은다.
  `FontFace { data: FontData, index: usize }`, `type FontData = Arc<memmap2::Mmap>`.
- 한글 후보는 **2개**: `/System/Library/Fonts/AppleSDGothicNeo.ttc`,
  `/System/Library/Fonts/Supplemental/AppleGothic.ttf`.
- **Rationale**: "한글 폰트 하나 추가"를 특수 분기로 박으면 다음 폴백에서 다시 뜯어야 한다.
  체인은 순서 있는 `Vec` 하나로 표현되고 선택 로직이 폰트 개수와 무관해진다.
  **mmap인 이유(steelman 반영)**: `AppleSDGothicNeo.ttc`는 실측 **55,373,848 bytes** —
  Menlo(2,156,036)의 26배다. 현행 `load_font`의 blocking `std::fs::read`(`lib.rs:1001-1008`)를
  그대로 쓰면 기동 경로에 55MB 동기 read가 붙고 55MB가 상주하며, **한글을 한 번도 안 치는
  사용자도 같은 값을 낸다** — `input→screen ≤ 8ms` 목표를 든 프로젝트에서 정당화되지 않는다.
  mmap은 실제로 만지는 페이지만 물리 메모리로 올라오므로 CJK 폰트의 정석 해법이고,
  swash `FontRef::from_index(&[u8], _)`는 어떤 슬라이스든 받으므로 zero-copy로 붙는다.
  `memmap2`는 GUI/위젯 툴킷이 아니라 `no-gui-crates` 규칙에 저촉되지 않는다.
  후보를 2개 둔 것은 Apple이 폰트를 `Supplemental/`로 옮긴 전례가 있어서다(#6 관련).
  거부한 대안: (a) 프라이머리/폴백 2개 필드 하드코딩 — 확장 불가, (b) CoreText
  (`CTFontCopyDefaultCascadeListForLanguages`)로 시스템 캐스케이드 질의 — **가장 올바르고
  OS 업데이트 내성이 있지만**(`macos-only` 규칙이 `objc2`를 명시 허용하므로 규칙 문제도
  아니다) 이번 PR의 범위를 크게 불린다. 경로 안정성은 후보 2개 + D3 프로브로 완화하고,
  CoreText 캐스케이드는 Known Gaps의 후속 후보로 남긴다.
- **`unsafe` 도입 명시(재검 B5)**: `Mmap::map`은 `unsafe fn`이고, 현재
  `crates/renderer/src`와 `crates/term-core/src`에는 `unsafe`가 **0건**이다 — 이 결정이
  워크스페이스 최초의 `unsafe` 블록을 들인다. 딱 한 곳(`load_font`)으로 격리하고
  `// SAFETY:` 주석에 "시스템 폰트 파일은 실행 중 교체되지 않는다고 가정; 교체 시 SIGBUS"를
  적는다(Known Gaps와 연결). 타입 측면은 문제없음: `Arc<Mmap>`은 `[u8]`로 deref되고
  `FontRef::from_index(&[u8], _)`는 임의 슬라이스를 받으며, `GlyphRasterizer`는 이미 호출마다
  `FontRef`를 재구성하므로(`glyph.rs:58,90`) 자기참조 수명 문제가 없다. `Mmap: Send + Sync`.

### D2: 문자별 폰트 선택 = 프라이머리 우선, charmap 커버리지 기반 첫 매치
- **Status**: resolved
- 순수 함수 `resolve_face(covers: impl Fn(usize, char) -> bool, n_faces: usize, ch: char) -> Option<usize>`:
  face 0부터 순회해 `covers(i, ch)`인 첫 인덱스를 반환, 전부 실패하면 `None`.
  `covers`는 실사용에선 `charmap().map(ch) != 0`.
- **Rationale**: 프라이머리(Menlo)에 있으면 라틴 렌더는 픽셀 단위로 무변경 — 회귀 위험이 0에
  가깝다. `covers`를 주입 가능한 클로저로 뺐기 때문에 시스템 폰트 없이 스텁으로 headless
  단위 테스트가 된다. 거부한 대안: 유니코드 블록 범위 테이블(한글 U+AC00..) — 폰트가 실제로
  커버하는지와 무관해 거짓 양성/음성이 난다.

### D3: TTC face 인덱스는 프로브 문자로 결정
- **Status**: assumed
- `.ttc` 로드 시 face 0..N을 순회하며 프로브 문자(폴백은 `'가'`, 프라이머리는 `'A'`)를
  charmap이 커버하는 첫 face를 채택. 어느 face도 커버 못하면 그 후보는 실패로 간주.
- **Rationale**: AppleSDGothicNeo.ttc는 여러 웨이트를 담은 컬렉션이라 "index 0이 한글을
  커버한다"는 가정을 코드에 굳히면 OS 업데이트에 깨진다. 프로브는 런타임 사실을 확인한다.
- **부수 효과(중요)**: `/System/Library/Fonts/Supplemental/NotoSansGothic-Regular.ttf`는
  이름과 달리 **고대 고딕 문자(U+10330)** 폰트이고 6,152 bytes다 — 한국어가 아니다.
  "Gothic"이라는 이름만 보고 후보에 넣는 실수를 프로브가 자동으로 걸러낸다. 후보 목록에
  넣지 않는다.

### D4: 폴백 로드 실패는 경고 후 정상 기동 (degraded 모드)
- **Status**: resolved *(L2-reviewer 지적 #6 반영 개정)*
- 폴백 후보가 하나도 안 열리면 `tracing::warn!`만 남기고 프라이머리 단독 `FontSet`으로 계속.
  이 상태를 **degraded**로 명시적으로 표현한다: `FontSet::is_degraded() -> bool`
  (= `faces.len() == 1`, 즉 폴백 face가 0개).
  `RendererError::NoFont`는 **프라이머리가 전부 실패할 때만** 반환(기존 동작 유지).
- **Rationale**: 폰트 하나 때문에 터미널이 안 뜨는 건 결함보다 나쁜 회귀다. 한글은 깨져
  보여도 셸은 살아 있다. 거부한 대안: 폴백도 hard error — 기동 불가 리스크가 더 크다.
  `is_degraded()`를 **상태로 승격**한 것은 D5와의 조합이 무성 실패를 만들기 때문이다(D5 참조).

### D5: 미커버 문자는 그리지 않는다 — 단 degraded 모드에서는 .notdef를 남긴다
- **Status**: resolved *(L2-reviewer 지적 #6 반영 개정)*
- **degraded 판정은 `Atlas`가 소유한다**(재검 B4): `Atlas::is_degraded() == (rasterizers.len() == 1)`.
  `Atlas::new`가 `&FontSet`을 받으므로(D7) 아틀라스가 이 사실을 안다. 레이어는 `ctx.atlas`만
  들고 있어 `FontSet::is_degraded()`에는 닿지 못하므로, 판정을 아틀라스에 두지 않으면
  `tab_bar`/`overlay_layer`에서 같은 무성 실패가 재발한다.
- 정상 모드(폴백 face ≥ 1): `Atlas::resolve(ch)`가 `None`을 반환하고, 호출자는 글리프
  인스턴스를 emit하지 않는다. 아틀라스의 `key.id == 0` 가드는 **`!is_degraded()`일 때만**
  작동한다.
- **degraded 모드**: `Atlas::resolve(ch)`는 미커버 문자에 대해
  `Some(GlyphKey { face: 0, id: 0, .. })`를 반환하고 id-0 가드가 비활성이므로 프라이머리
  .notdef가 그려진다 — 오늘의 동작을 유지한다. 이 규칙이 그리드·탭바·오버레이·preedit
  **네 경로 모두**에 동일하게 적용된다.
- **Rationale**: 이모지 한 줄이 빈 사각형 벽으로 도배되는 게 안 보이는 것보다 나쁘다.
  **그러나** D4(폴백 실패 시 degrade) × D5(미커버는 안 그림)를 무조건 합치면, Apple이
  두 후보 경로를 모두 옮기는 순간 **한글이 완전히 투명해진다** — 오늘의 .notdef 박스보다
  나쁜 상태이면서 신호는 사용자가 절대 안 보는 `tracing::warn!` 하나뿐이다. degraded에서만
  .notdef를 남기면 "폰트가 없다"가 눈에 보이고, 정상 경로의 이점(이모지 빈 박스 없음)은
  그대로 지킨다. 거부한 대안: (a) 항상 .notdef — 현 결함의 시각적 증상 그 자체,
  (b) U+FFFD 대체 — 프라이머리가 �를 커버한다는 보장이 없고 "실제 문자"와 구분이 안 된다,
  (c) 리스크 수용 — 무성 실패는 이 프로젝트에서 가장 비싼 실패 형태다.
- **주의**: 기존 테스트 `lib.rs:1190-1193`이 `atlas.get_or_insert(&queue, 0).is_some()`
  (=.notdef가 아틀라스에 들어간다)를 단언한다 → 정상/degraded 두 모드로 갈라 다시 쓴다.

### D6: 폴백 글리프는 변형 없이 셀 범위 안에서 가로 중앙 정렬
- **Status**: resolved *(L2-reviewer 지적 #4 반영 — 산식 구체화)*
- 폴백 face로 래스터한 글리프는 프라이머리와 동일 ppem, 스케일/변형 없음.
- **span 산출**: 리드 셀 `col`의 span은 `snapshot.cell(col+1, row)`가 continuation이면 2,
  아니면 1 — `lib.rs:849`가 이미 쓰는 것과 **같은 술어**를 재사용한다(새 정보원 없음).
- **오프셋**: 클러스터의 **총 advance**(`cluster.glyphs`의 `g.advance` 합)를 `adv`라 할 때
  `offset = ((span as f32 * cw - adv) * 0.5).max(0.0)`, `pen = col * cw + offset`.
  `max(0.0)`이므로 글리프가 span보다 넓어도 **이전 셀로 번지지 않는다**(오른쪽으로만 넘침).
  프라이머리 face(index 0)로 해석된 클러스터는 `offset = 0` — 라틴 배치 산식 무변경(제약).
- **Rationale**: Menlo 2셀 폭은 `1.2 × ppem`, SD Gothic 한글은 대략 `1.0 × ppem`이라 이미
  들어간다 — 스케일링은 불필요하면서 힌팅된 획 굵기를 라틴과 어긋나게 만든다. 총 advance를
  쓰는 이유는 배치 루프가 클러스터 단위로 `pen`을 리셋하고(`lib.rs:882`) 그 안에서
  `pen += g.advance`로 전진하기 때문(`lib.rs:899`) — 첫 글리프 advance만 쓰면 다중 글리프
  클러스터에서 중심이 어긋난다. 거부한 대안: (a) 셀 폭에 맞춘 재래스터 — 폴백만 다른 ppem이
  되어 획 굵기 불일치, (b) 클립 — 글자가 잘리면 가독성이 무너진다. 중앙 정렬은 좌측 정렬보다
  CJK 문서 관례에 맞고 구현이 한 줄이다.

### D7: `GlyphKey`에 face 축을 추가하고 `Atlas`가 face당 rasterizer를 보유
- **Status**: resolved
- `GlyphKey { face: u16, id: u16, bold: bool, italic: bool }`. `Atlas`는
  `rasterizers: Vec<GlyphRasterizer>`를 갖고 `Atlas::new(device, &FontSet, ppem)`으로 만든다.
  기존 무-face 진입점(`glyph_id`, `get_or_insert`)은 face 0(프라이머리) 기준으로 유지.
- **`GlyphRasterizer`는 `FontData`(= `Arc<Mmap>`)를 받는다** — 현행
  `GlyphRasterizer::new(data: Vec<u8>, …)`(`glyph.rs:46`)의 **값 전달**과
  `atlas.rs:75`의 `(*font).clone()`을 함께 고친다.
- **Rationale**: glyph id는 폰트별 네임스페이스라 face 축이 없으면 Menlo의 gid 42와 SD
  Gothic의 gid 42가 같은 캐시 슬롯을 두고 충돌해 엉뚱한 글리프가 그려진다.
  값 전달을 고치는 이유: 현행 시그니처를 유지한 채 face를 늘리면 `Atlas::new`가 face마다
  바이트를 **딥카피**한다 — 55MB짜리 한글 face에서는 `apply_config`가 config 워처에 의해
  ppem을 바꿀 때마다(`lib.rs:535`) 55MB 복사가 반복된다. 거부한 대안: face별 아틀라스
  텍스처 분리 — 바인드 그룹/드로우콜이 face 수만큼 늘어난다.

### D8: 조합중 게이트는 `Key::Character`/`Key::Dead`에만 적용
- **Status**: resolved
- `app`에 `composing: bool` 상태를 두고 `Ime::Preedit(s, _)`에서 `!s.is_empty()`로 set,
  `Ime::Preedit("", None)`/`Ime::Commit`/`Ime::Disabled`에서 clear. 순수 함수
  `fn suppress_key(composing: bool, key: &Key) -> bool`은 `composing && matches!(key,
  Key::Character(_) | Key::Dead(_))`일 때만 true.
- **Rationale**: winit 0.30.13 macOS가 이미 조합중/커밋 `KeyboardInput`을 억제하고(view.rs:479-496)
  한글 IME 이중 전송을 명시적으로 처리한다(view.rs:420-435). 하지만 `Preedit` 중
  `doCommandBySelector`는 `forward_key_to_app=true`로 방향키를 **일부러 통과**시키므로,
  "조합중 모든 키 드롭"은 조합중 커서 이동을 죽인다. 문자키로 좁히면 winit 회귀에 대한
  방어선은 유지하면서 Named 키 경로는 손대지 않는다. 거부한 대안: (a) 게이트 없음 — winit
  버전 업그레이드 시 회귀 감지 불가, (b) 전체 드롭 — 위 방향키 버그.

### D9: IME는 `app`, preedit 렌더는 `renderer`의 `DrawLayer` — `term-core` 무변경
- **Status**: resolved
- `app`이 `WindowEvent::Ime`를 받아 preedit 문자열을 `Rc<RefCell<..>>` 핸들로
  `renderer::preedit_layer`에 밀어넣고, 레이어는 커서 위치를 `ctx.snapshot.cursor`에서 읽어
  글리프 z=0 위 / 커서 z=5 아래에 그린다. `PreeditLayer::new() -> (PreeditLayer, PreeditHandle)`.
- **Rationale**: `tab_bar.rs:42-44,224-232`의 `TabModelHandle` 패턴을 그대로 복제 — 이미
  검증된 이 프로젝트의 레이어 모델 전달 방식이다. 조합중 텍스트를 `term-core` 그리드에
  쓰면 셸이 모르는 문자가 스크롤백/스냅샷에 섞이고 `dependency-direction` 규칙(엔진은
  GPU/윈도우 무의존, 그리고 그리드는 PTY가 쓴 것만 담는다)을 흐린다. 거부한 대안:
  (a) 그리드에 직접 write 후 커밋 시 지우기 — 스크롤백 오염 + 스냅샷 캐시(D7/idle-CPU) 무효화,
  (b) macOS 후보창 위치만 맞추기 — L0에서 사용자가 인라인 렌더를 선택.

### D10: 폴백은 아틀라스 API 한 곳에 두고, 레이어 텍스트 경로를 **폭 인식**으로 고친다
- **Status**: resolved *(L2-reviewer 지적 #5 반영 — "1줄 수정" 가정이 틀렸음)*
- `Atlas`에 `resolve(ch) -> Option<GlyphKey>`(face 해석 + glyph id, 커버 실패 시 `None`)를
  추가하고, `tab_bar.rs:408-409`와 `overlay_layer.rs:264-265`가 이걸 쓰게 바꾼다.
- **추가로 필수**: 두 레이어의 전진은 char당 `pen += cw` 고정이고(`tab_bar.rs:417`,
  `overlay_layer.rs:273`) `truncate`는 **char 수**를 센다(`tab_bar.rs:584-586`).
  한글은 2셀이므로 이대로면 글리프가 겹치고 14셀 사이드바(`width_px`, `tab_bar.rs:77-79`)를
  넘친다 — 정확히 Research에 적은 "한글 cwd basename" 케이스가 깨진다. 따라서:
  `pen += ch.width().unwrap_or(1) as f32 * cw`로 바꾸고, `truncate`를 **열 예산** 기준
  (`fn truncate_cols(s: &str, max_cols: usize) -> String`)으로 교체한다.
- **의존 선언(재검 B6)**: 루트 `[workspace.dependencies]`에는 `common`/`term-core`/`renderer`만
  있고 `unicode-width`는 `crates/term-core/Cargo.toml:16`에 직접 선언돼 있다. 워크스페이스
  공유로 바꾸려면 루트와 term-core를 **둘 다** 고쳐야 하므로, 이번엔 범위를 줄여
  `crates/renderer/Cargo.toml`에 `unicode-width = "0.2.0"`을 **term-core와 동일 버전으로 핀**해
  추가한다(cargo가 같은 semver 트리로 통일하므로 폭 규칙은 한 갈래로 유지된다).
- **Rationale**: 폴백 지식이 `build_glyph_instances`에만 있으면 "한글 디렉터리에서 연 탭의
  제목이 .notdef 박스"라는 같은 결함이 다른 이름으로 남는다. 진입점을 아틀라스에 두면 이후
  추가될 레이어도 자동으로 옳은 경로를 탄다. 다만 폴백만 붙이고 폭을 안 고치면 .notdef 박스가
  **겹친 글리프**로 바뀔 뿐이라 결함이 해소되지 않는다. 거부한 대안: 레이어별 폴백 로직
  복제 — 같은 버그를 N곳에 심는다.

### D11: 탭 전환·포커스 이탈 시 preedit를 버리고, `objc2`로 **OS 조합을 실제로 취소**한다
- **Status**: resolved *(reviewer #7 → 개정 → 재검 B1/B2/B3 → 재개정 → **검증 루프에서 재개정**)*
- **커밋 라우팅은 "탭 정체성"으로 한다 — 일회성 가드를 쓰지 않는다**(3차 재검 C-1):
  `composing_tab: Option<TabId>`를 둔다. **새** 조합이 시작될 때(직전에 조합 중이 아니었던
  비어 있지 않은 `Ime::Preedit`) 그때의 활성 탭 id를 기록하고, `Ime::Commit`은 **그 탭**(아직
  열려 있으면)에 쓴다 — 닫혔으면 버린다. 같은 조합이 이어지는 동안에는 소유권을 재지정하지
  않는다(첫 소유자 유지). `composing_tab == None`인 상태의 `Commit`(= 조합을 거치지 않은
  평범한 텍스트)은 평소대로 **활성 탭**에 쓴다.
- **소유권 해제 시점 — 게이트와 분리한다** *(검증 루프 재개정: 원안은 작동하지 않았다)*:
  `composing`(= `suppress_key` 게이트)은 빈 `Preedit`에서 false가 되지만,
  **`composing_tab`은 빈 `Preedit`에서 해제되지 않는다.** 해제는 ① `Ime::Commit`이 바이트를
  배치한 **직후**, ② `Ime::Disabled`, ③ 새 조합의 시작 — 이 셋뿐이다.
  - **원안이 무력했던 이유**: winit `insertText:`는 **모든** `Ime::Commit` 바로 앞에
    `Ime::Preedit("")`를 큐잉한다(view.rs:412-413). 원안처럼 빈 `Preedit`에서
    `composing_tab = None`으로 만들면 `Commit` arm이 읽는 시점엔 **항상 `None`** 이라
    모든 커밋이 활성 탭으로 떨어지고 `CommitTarget::Tab`/`Drop`은 **도달 불가능한 죽은
    코드**가 된다. 즉 D11이 `discard_next_commit` 가드를 기각한 바로 그 이벤트 순서에
    자기 자신도 당했다. 회귀 테스트
    `terminal_app::tests::ownership_survives_the_pre_commit_preedit`가 이 순서를 고정한다.
- `do_tab_action`과 `WindowEvent::Focused(false)`에서, `composing`이 true일 때:
  1. preedit 핸들을 비우고 **`composing = false`를 그 자리에서 인라인으로** 되돌린다.
     (`Ime::Disabled`를 기다려 D12 경로로 정리되기를 바라면 안 된다 — winit 이벤트는
     `queue_event`로 **큐잉**되어 나중 디스패치에 도착한다.)
     **`composing_tab`은 유지한다**: `discardMarkedText`가 무시되거나 커밋이 이미 큐에 올라간
     뒤라면, 그 뒤늦은 `Ime::Commit`이 사용자가 방금 옮겨간 탭이 아니라 **조합을 시작한 탭**에
     떨어져야 한다(R8.3). 이것이 `discardMarkedText` 다음의 두 번째 방어선이다.
  2. winit `Window`의 `ns_view`(`winit::raw_window_handle`의 `AppKitWindowHandle` — winit
     0.30.13이 `rwh_06`을 default feature로 re-export하므로 `app`에 새 의존이 필요 없다)를
     `NSView`로 보고 `inputContext().discardMarkedText()`를 `objc2`로 호출한다.
     `objc2_app_kit::NSView::inputContext()` / `NSTextInputContext::discardMarkedText()`는
     둘 다 safe fn이고 default feature에 포함된다 — `unsafe`는 포인터→`&NSView` 캐스트 한 곳뿐.
- **왜 가드가 아니라 라우팅인가(3차 재검 C-1)**: 원안의 `discard_next_commit` 일회성 가드는
  **반드시 정상 문자를 하나 삼킨다**. winit의 `insertText:`는 모든 `Ime::Commit` **바로 앞에**
  `Ime::Preedit("")`를 큐잉하므로(view.rs:411-414), "비어 있지 않은 Preedit에서 만료"는 영원히
  발동하지 않고 다음 `Commit`이 먹힌다. 반대로 "빈 Preedit에서 만료"로 바꾸면 가드가 항상 즉시
  풀려 무의미해진다 — 이벤트 스트림만으로는 만료 규칙을 세울 수 없다. 게다가 `discardMarkedText`는
  클라이언트의 `unmarkText`를 되불러주지 않으므로 winit의 `marked_text` ivar가 stale로 남아
  다음 타건도 `Commit` 경로로 오는데, 그 문자가 `KeyboardInput`으로도 오지 않아
  (`had_ime_input` 억제, view.rs:478-489) **PTY에 도달할 다른 경로가 없다**. 탭 정체성 라우팅은
  이벤트 순서·타이밍에 전혀 의존하지 않으면서 (a) 다른 탭 오염을 원천 차단하고 (b) 어떤
  문자도 잃지 않는다.
- **Rationale**: 원안 ②(`set_ime_allowed(false)`→`(true)` 토글)는 **사실이 아니었다**.
  winit 0.30.13의 `set_ime_allowed(false)`는 자기 `marked_text` ivar만 비우고
  `ime_state = Disabled` + `Ime::Disabled` 큐잉을 할 뿐, `inputContext.discardMarkedText()`를
  호출하지 않는다(view.rs:880-896; 그 호출은 `unmarkText` 경로에만 있다 — view.rs:342-343).
  즉 OS/IME 서버의 조합은 살아남아 후보창이 떠 있을 수 있다. 게다가 토글 직후에는
  `hasMarkedText()`가 false이고 `is_ime_enabled()`도 false라 `insertText:`가 `Ime::Commit`을
  **큐잉조차 못 하므로**(view.rs:411, 846-848), "다음 커밋에서 가드를 해제한다"는 원안은
  기다리던 커밋이 영영 안 와서 가드가 켜진 채 남고 **다음 정상 음절을 조용히 삼킨다**.
  그래서 (a) 취소는 `objc2` 직호출로 실제로 하고, (b) 가드는 만료 규칙을 갖는다.
  `macos-only` 규칙은 objc2를 명시 허용하고 `app`은 이미 `objc2-app-kit`을 쓴다(clipboard.rs:17).
  원안이 막으려던 사고 자체는 실재한다: 탭 전환은 내부 액션이라 `resignFirstResponder`가
  일어나지 않아 macOS가 marked text를 들고 있고, 뒤이은 `Ime::Commit`이 **새 탭의 셸에**
  써진다. Cmd+T는 Named 키 + modifier라 D8 게이트를 그냥 통과하므로 키보드만으로 도달 가능하다.
  거부한 대안: (a) 가드만 — 후보창이 남고 다음 타건이 이전 음절을 이어갈 수 있다,
  (b) 조합중 탭 전환 차단 — 단축키가 가끔 안 듣는 것처럼 느껴진다.

### D19: `set_ime_cursor_area`로 후보창을 커서에 맞춘다
- **Status**: resolved *(재검 C5)*
- preedit가 갱신될 때마다(그리고 커서가 움직일 때)
  `window.set_ime_cursor_area(PhysicalPosition, PhysicalSize)`를 호출한다.
  좌표는 커서 셀의 **물리 px** — `Renderer::cell_size()` / `left_inset()` / `top_inset()`
  (모두 pub이고 `terminal_app.rs:370-371`이 이미 쓴다)로 조립한다.
- **좌표계 주의(3차 재검 C-2)**: `set_ime_cursor_area`는 `Into<Position>`/`Into<Size>`를 받고
  macOS 구현이 `spot.to_logical(scale_factor)`로 변환한다(window_delegate.rs:1553-1561).
  그런데 `cell_size_px`는 `scale_factor`를 곱해 만들고(`lib.rs:1010-1011`)
  `left_inset`/`top_inset`도 "Physical px"로 문서화돼 있다(`lib.rs:121-126`). 이 값들을
  `LogicalPosition`으로 넘기면 winit이 변환을 건너뛰어 **모든 Retina 맥에서 2배 어긋난다** —
  반드시 `Physical*`로 넘긴다.
- **Rationale**: macOS는 한자 변환·후보 주석창을 `firstRectForCharacterRange`로 배치하고,
  winit은 거기서 `set_ime_cursor_area`로 받아둔 값을 그대로 돌려준다(view.rs:376-390).
  호출하지 않으면 기본값이 쓰여 후보창이 창 좌측 상단에 뜬다 — IME를 켜 놓고 이걸 빼면
  "한글은 되는데 변환창이 엉뚱한 데 뜬다"가 된다. 셀 기하는 D18에서 이미 계산하므로 추가
  비용이 거의 없다.

### D12: `Ime::Disabled`/`Ime::Enabled`를 명시적으로 처리
- **Status**: resolved
- `Ime::Disabled` → preedit 클리어 + `composing = false`. `Ime::Enabled` → no-op(로깅만).
- **Rationale**: winit은 입력 소스가 바뀌면 `Preedit("")` 클리어 없이 곧장 `Ime::Disabled`를
  보낼 수 있다(view.rs:446-452). 이 arm이 없으면 `composing`이 true로 굳어 이후 **모든
  문자키가 영구히 삼켜지는** 데드락이 된다 — D8 게이트의 필수 짝. 거부한 대안:
  `_ => {}` — 위 데드락.

### D13: 아틀라스를 2048²로 확대
- **Status**: resolved
- `ATLAS_SIZE: u32 = 1024` → `2048` (R8Unorm, 1MB → 4MB).
- **Rationale**: 한글은 글리프 종류가 라틴 대비 두 자릿수 많아 한 프레임 안에서 shelf가
  가득 차 `pack`이 전체 evict(atlas.rs:193-200)를 돌릴 확률이 실질적으로 올라간다. 그 순간
  같은 프레임에서 **이미 emit된 인스턴스의 UV가 무효화**돼 글자가 뒤섞인다(선행 결함이지만
  한글이 노출 확률을 끌어올린다). 상수 하나로 확률을 실용 범위 밖으로 밀어낸다.
  거부한 대안: (a) 무대응 — 한글 사용 시 눈에 보이는 깨짐, (b) 프레임 내 generation 감지 후
  재빌드 — 정확하지만 이 스펙의 범위(폴백+IME)를 넘고 최악 케이스 비용이 붙는다. 근본 수정은
  Known Gaps로 남긴다.

### D16: 행을 face 단위 **run**으로 분할한 뒤 run별로 셰이핑
- **Status**: resolved *(L2-reviewer 지적 #3 — 최대 buildability 갭)*
- Confirmed Goal의 "클러스터별로 폰트를 고른다"는 **순환 정의**다(클러스터는 셰이핑의
  *출력*). 실제 순서는: ① 행을 훑어 각 `char`의 face를 `resolve_face`로 해석 →
  ② 연속으로 같은 face인 구간을 하나의 **run**으로 묶음 → ③ run마다 그 face의 `FontRef`로
  `ShapeContext` 셰이퍼를 만들어 run 문자열만 셰이핑.
- 순수 함수로 분리: `fn split_runs(faces: &[Option<usize>]) -> Vec<Run>`,
  `Run { face: Option<usize>, start_char: usize, end_char: usize }`. face가 `None`인
  (미커버) 문자는 자기들끼리 run을 이루고 D5에 따라 처리된다.
- **인덱싱 기준(재검 B8)**: `faces`/`Run`의 인덱스는 **continuation을 스킵한 뒤의 char 수열**
  — 즉 `lib.rs:847-860`이 만드는 `line`의 char 순서 — 기준이다. 원시 열 번호 기준으로 잡으면
  wide-char 스페이서 때문에 run 경계가 어긋난다. 따라서 행을 훑을 때 `line`과 함께
  `char_col: Vec<u16>`(각 char → 절대 열)을 나란히 만들고, `split_runs`는 face 열만 보므로
  호출자가 `char_col`을 들고 있다가 run별 `byte_col` 재구성에 쓴다.
- `byte_col`은 **run-relative**로 다시 만든다: run 문자열을 쌓으면서 바이트 오프셋 →
  절대 열 번호 맵을 run마다 새로 채우고(`char_col`에서 끌어온다), `cluster.source.start`를
  그 맵으로 조회한다.
- 리가처 feature(`liga`/`calt`)는 run **내부**에만 적용된다 — run 경계에서 리가처가 끊기는
  것은 의도된 동작이다(프라이머리↔폴백 경계를 넘는 리가처는 애초에 성립하지 않는다).
- **폰트 바이트 소유권**: `FontSet`은 `Renderer`가 소유하고 `build_glyph_instances`에
  `&FontSet`으로 넘긴다(현행 `font_data: &Arc<Vec<u8>>` 파라미터를 대체 — `lib.rs:815,831`).
  `Atlas`는 같은 `FontData`(`Arc<Mmap>`) 핸들을 복제해 rasterizer를 만든다 — 셰이핑과
  래스터가 **같은 바이트를 공유**하되 소유자는 하나(`FontSet`)로 명확히 한다.
- **Rationale**: run 분할 없이는 구현이 불가능하다("무엇을 어떤 폰트로 셰이핑할지"가 정해지지
  않음). 순수 함수로 뺐기 때문에 GPU/폰트 없이 `split_runs`를 표 기반으로 단위 테스트할 수 있다.

### D17: 아틀라스 `generation`이 바뀌면 re-shape 캐시를 무효화
- **Status**: resolved *(L2-reviewer 지적 #9 — 영구 손상 경로)*
- `Renderer::draw_to`가 프레임 시작 시 `atlas.generation()`을 기록하고, 레이어 기록까지 끝난
  뒤 값이 바뀌었으면 `self.last_snapshot = None`으로 되돌린다 → 다음 프레임이 강제로 re-shape.
- **Rationale**: `generation`은 지금 `atlas.rs` 밖에서 **어디서도 참조되지 않는다**(프로덕션
  코드 기준 참조 0건; `lib.rs:1312,1323`은 테스트). 그래서 탭바/오버레이/preedit 레이어가
  새 한글 글리프를 팩하며 evict를 유발하면(`atlas.rs:190-197`), 그리드의 상주
  `glyph_vbuf`는 **낡은 UV를 든 채 스냅샷이 같다는 이유로 영원히 재빌드되지 않는다**
  (`lib.rs:646`). D8이 이 경로를 *구조적으로 도달 가능*하게 만든다: 조합중에는 키 입력이
  억제되므로 그리드 스냅샷이 정지한 채 preedit 레이어만 매 타건 새 음절을 래스터한다.
  D13(2048²)은 확률을 낮출 뿐 이 경로를 없애지 못한다. 거부한 대안: 아무 것도 안 함 —
  화면이 영구히 깨진 채 남는다(프레임 내 무효화보다 나쁘다).
- **전제 의존 명시(재검 B7)**: 이 수정이 실제로 화면을 고치는 것은 `terminal_app.rs:428-429`의
  **연속 redraw** 덕분이다(렌더러 스스로는 프레임을 요청하지 않는다). 해당 코드 주석과 D17에
  이 결합을 적어, 훗날 on-demand redraw로 되돌릴 때 영구 손상 경로가 조용히 다시 열리지
  않게 한다. 해당 프레임 자체는 여전히 낡은 UV로 그려진다(프레임 *내* 무효화는 Known Gaps).

### D18: preedit 레이어는 z=6 (커서 위), 경계 상태는 명시적으로 정의
- **Status**: resolved *(L2-reviewer 지적 #8 반영)*
- z-order는 **6** — `CURSOR_Z = 5`(`cursor_layer.rs:18`) 바로 위. 커서 블록이 0.5 알파로
  preedit 첫 셀을 덮어 D14의 가독성 근거를 무너뜨리는 것을 막는다.
- 인셋: `FrameCtx`의 `left_inset`/`top_inset`(`lib.rs:121-126`)을 반드시 반영한다 —
  `cursor_layer.rs:225-227`과 그 `block_rect_shifts_with_left_inset` 테스트는 이 인셋을
  한 번 틀렸던 흔적이므로 같은 회귀 테스트를 preedit 레이어에도 둔다.
- 경계 상태: ① 스크롤백을 보는 중(`snapshot.at_bottom == false`)이면 그리지 않는다
  (`cursor_layer.rs:239`와 동일 규칙). ② `cursor.col == cols`(랩 대기)면 `cols - 1`로 클램프.
  ③ preedit가 남은 열보다 길면 행 끝에서 **자른다**(줄바꿈/스크롤 없음).
- **Rationale**: 조합중 텍스트는 셸이 모르는 임시 오버레이이므로 그리드의 줄바꿈 규칙을
  흉내 낼 이유가 없고, 흉내 내면 아래 셸 내용을 더 많이 가린다.

### D14: preedit는 배경을 칠하고 밑줄과 함께 그린다
- **Status**: resolved
- preedit 문자열이 차지할 셀들을 먼저 **테마 기본 배경색**으로 칠한 뒤 그 위에 글리프를
  올리고, 그 아래 밑줄을 그린다. 아래 셀의 bg가 아니라 테마 기본 bg를 쓴다 — 아래 내용을
  확실히 덮는 것이 목적이기 때문(재검 C4).
- **색 전달 경로(3차 재검 minor)**: `Renderer.bg_default`는 private이고 `FrameCtx`에는 색이
  없다(`lib.rs:113-131, 174`) — 레이어가 읽을 방법이 없다. 따라서 `PreeditHandle`이
  문자열과 함께 `bg`/`fg`도 나른다: `app`이 `cfg.theme.background`/`foreground`를 넣어주고,
  `settings::apply`(config 리로드 경로)에서도 갱신한다. `FrameCtx`를 넓히지 않고 D9의
  `TabModelHandle` 패턴을 그대로 유지하는 쪽을 택한다.
- **밑줄 기하는 그리드 경로의 상수를 그대로 재사용한다**: 두께 `(ch * 0.06).max(1.0)`,
  베이스라인 아래 오프셋 `(ch * 0.08).max(1.0)` — `lib.rs:906-910`과 동일. 아틀라스의
  `solid_entry`(atlas.rs:155)로 그린다.
- 폭 계산은 `unicode-width`(D10에서 `renderer`에 추가하는 것과 동일 crate/버전)로 한다.
- **Rationale**: preedit는 `term-core` 그리드 위에 겹쳐 그려지므로(D9), 배경을 안 칠하면
  아래 셸 문자와 획이 섞여 판독 불가능해진다. 밑줄은 "아직 확정되지 않은 텍스트"의 표준
  IME 관례이며 이미 `solid_entry`(atlas.rs:155)로 그릴 수 있다. 거부한 대안: (a) 글리프만 —
  위 겹침, (b) 반전 색상 — 셸의 reverse-video 셀과 구분이 안 된다.

### D15: 결함③ — `Pty::spawn`이 로케일 없을 때만 `LC_CTYPE=UTF-8`을 주입
- **Status**: resolved
- **인버전 프로브가 찾아낸 세 번째 결함.** `Pty::spawn`은 `TERM`만 설정한다
  (`term-core/src/pty.rs:38-39`). 로케일이 없으면 zsh는 `한글`을 **6문자**로 센다
  (실측: `env -i /bin/zsh -c 's="한글"; echo ${#s}'` → `6`, `LC_CTYPE=UTF-8` → `2`).
  우리 그리드는 2문자/4열(`grid.rs:255-292`)이므로 ZLE의 커서 산술과 `\b\b  \b\b`
  이레이즈가 전부 어긋나 한글 명령줄 편집이 깨진다.
- 순수 함수는 **3상태**를 반환한다(재검 C1 — 원안 `Option`은 "명시적 비-UTF8"을 표현하지 못했다):
  ```rust
  pub enum LocaleAction { Inject(&'static str, &'static str), Ok, NonUtf8 }
  pub fn locale_action(get: &dyn Fn(&str) -> Option<String>) -> LocaleAction
  ```
  `LC_ALL`/`LC_CTYPE`/`LANG`이 모두 부재 → `Inject("LC_CTYPE", "UTF-8")`;
  우선순위 최상위 값이 UTF-8을 함의(대소문자 무시 `utf-8`/`utf8` 포함) → `Ok`;
  그 외(예: `LC_ALL=C`) → `NonUtf8`.
- **"부재"의 정의(리뷰어 지적 #2)**: `None` **그리고 `Some("")`** 를 모두 부재로 친다.
  빈 문자열 로케일은 설정된 것이 아니라 비워진 것이므로 주입 대상이다 — 테스트가 단언할
  discriminator다.
- **`NonUtf8`은 덮어쓰지 않되** `tracing::warn!`으로 "한글 등 wide char 편집이 셸과 어긋날 수
  있음"을 남긴다 — 사용자 의도는 존중하되 무성 실패로는 두지 않는다. `Pty::spawn`은 탭마다
  호출되므로 **프로세스당 1회**로 제한한다(`std::sync::Once`) — 탭 10개 열었다고 같은 경고가
  10줄 나오면 안 된다.
- **환경 오라클 근거**: `portable_pty::CommandBuilder::new`는 `std::env::vars_os()`로 환경을
  씨딩하고 `spawn`이 `env_clear()` 후 재적용한다(`portable-pty-0.9.0/src/cmdbuilder.rs:74-86,
  528-532`) — 따라서 실사용 `get`은 `std::env::var`가 맞다.
- **Rationale**: `.app` 번들을 Finder에서 띄우면 로케일 없는 최소 환경을 상속하고, 셸에서
  `cargo run`하면 상속받아 정상 동작한다 — 개발 중엔 안 보이고 배포판에서만 깨지는 형태라
  더 위험하다. `LC_CTYPE=UTF-8`은 macOS Terminal.app이 쓰는 관례이고 locale DB 설치 여부와
  무관하게 항상 유효하다. 거부한 대안: (a) 항상 강제 — 의도적으로 비-UTF8 로케일을 쓰는
  사용자를 깨뜨린다, (b) `LANG=en_US.UTF-8` 주입 — 해당 로케일 설치가 전제되고 메시지 언어까지
  바꾼다. `term-core`는 GPU/윈도우 무의존이라 이 수정은 의존 방향을 위반하지 않는다.

## Constraints
- **의존 방향 불가침**: 폰트/폴백/preedit 렌더 = `renderer`, IME 이벤트 = `app`, 로케일/PTY =
  `term-core`. `term-core`는 wgpu/winit을 직·간접으로 참조하지 않는다 (`.claude/rules/dependency-direction.md`).
- **GUI 크레이트 금지**: preedit 렌더는 `DrawLayer` 자작 패스로만 (`.claude/rules/no-gui-crates.md`).
- **macOS ARM64 전용**: `#[cfg(target_os)]` 분기 추가 금지, 시스템 폰트 절대경로 사용 OK (`.claude/rules/macos-only.md`).
- **zsh 로컬 전용**: `SHELL` 상수 고정, 셸 선택 로직 추가 금지 (`.claude/rules/zsh-local-only.md`).
- **테스트 밖 `unwrap()` 금지**: `glyph.rs:59,90`의 기존 `unwrap()` 패턴을 face 수만큼 복제하지 않는다.
- **에러는 per-crate `thiserror` enum**: 새 실패 모드는 `RendererError`/`term_core::Error`에 variant로.
- **라틴 렌더 회귀 0**: 프라이머리(Menlo)가 커버하는 문자의 배치 산식·글리프는 픽셀 단위로 무변경.
- **검증 게이트**: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings` 전부 green.

## Requirements

### R0: 한글 타이핑이 조합되고 실제 글리프로 그려진다 (goal)

#### R0.1: 한글 음절이 조합돼 셸에 도달한다
- **Given**: macOS 한글 입력기가 선택된 상태로 터미널이 실행 중
- **When**: 사용자가 `ㅎ` `ㅏ` `ㄴ` 순서로 타이핑한다
- **Then**: PTY에 `ㅎ`/`ㅏ`/`ㄴ` 낱자가 아니라 조합된 `한`(UTF-8 3바이트)이 **한 번** 써진다

#### R0.2: 한글 셀이 .notdef가 아닌 한글 글리프로 렌더된다
- **Given**: 그리드에 `한글`이 들어 있고 폴백 폰트가 정상 로드된 상태
- **When**: 프레임을 그린다
- **Then**: 두 셀에 대해 face 0이 아닌 폴백 face의 0이 아닌 glyph id로 인스턴스가 emit된다

#### R0.3: 라틴 렌더에 회귀가 없다
- **Given**: 폴백 체인 도입 전/후의 동일한 ASCII 전용 `GridSnapshot`
- **When**: `build_glyph_instances`를 호출한다
- **Then**: 생성된 `GlyphInstance`의 `pos`/`size`/`uv`가 도입 전과 동일하다 (face 0 → `offset = 0`)

---

### R1: 폰트 세트 로딩 — mmap · TTC 프로브 · degraded (D1, D3, D4)

#### R1.1: 프라이머리와 폴백을 순서 있는 체인으로 연다
- **Given**: `/System/Library/Fonts/Menlo.ttc`와 `AppleSDGothicNeo.ttc`가 존재
- **When**: `FontSet::load()`를 호출한다
- **Then**: `faces[0]`은 Menlo, `faces[1]`은 한글 face이고 `is_degraded() == false`

#### R1.2: 폰트는 mmap으로 열린다
- **Given**: `FontSet::load()`가 성공한 상태
- **When**: `FontFace.data`의 타입을 본다
- **Then**: `Arc<memmap2::Mmap>`이며, `std::fs::read`로 55MB를 힙에 복사하지 않는다.
  `unsafe`는 `load_font` 한 곳에만 등장하고 `// SAFETY:` 주석을 동반한다

#### R1.3: TTC face 인덱스는 프로브 문자로 결정된다
- **Given**: 여러 face를 담은 `AppleSDGothicNeo.ttc`
- **When**: 프로브 `'가'`로 face를 고른다
- **Then**: charmap이 `'가'`를 0이 아닌 gid로 매핑하는 **첫** face 인덱스가 `FontFace.index`에 담긴다

#### R1.4: 어느 face도 프로브를 커버 못하면 그 후보는 탈락한다
- **Given**: 후보 경로가 열리지만 어느 face도 프로브 문자를 커버하지 않는다
  (예: `NotoSansGothic-Regular.ttf` — 고대 고딕 문자 폰트)
- **When**: 그 후보를 로드한다
- **Then**: `FontSet`에 추가되지 않고 다음 후보로 넘어간다

#### R1.5: 첫 한글 후보가 실패하면 두 번째 후보로 넘어간다
- **Given**: `AppleSDGothicNeo.ttc`는 열리지 않고 `Supplemental/AppleGothic.ttf`는 열린다
- **When**: `FontSet::load()`를 호출한다
- **Then**: `faces[1]`이 AppleGothic이고 `is_degraded() == false`

#### R1.6: 폴백이 전부 실패해도 터미널은 기동한다
- **Given**: 모든 한글 후보 경로가 열리지 않는다
- **When**: `Renderer::new`가 실행된다
- **Then**: `Err`가 아니라 `Ok`를 반환하고, `tracing::warn!`이 한 번 남으며 `is_degraded() == true`

#### R1.7: 프라이머리가 전부 실패할 때만 hard error
- **Given**: `FONT_CANDIDATES`의 모든 경로가 열리지 않는다
- **When**: `Renderer::new`가 실행된다
- **Then**: `RendererError::NoFont`를 반환한다 (기존 동작 유지)

---

### R2: 문자 → face 해석 (D2, D5)

#### R2.1: 프라이머리가 커버하면 프라이머리를 쓴다
- **Given**: `covers(0, 'A') == true`인 스텁
- **When**: `resolve_face(covers, n_faces, 'A')`
- **Then**: `Some(0)`

#### R2.2: 프라이머리가 커버 못하면 첫 커버 face로 넘어간다
- **Given**: `covers(0, '한') == false`, `covers(1, '한') == true`인 스텁
- **When**: `resolve_face(covers, 2, '한')`
- **Then**: `Some(1)`

#### R2.3: 아무 face도 커버 못하면 None
- **Given**: 모든 face가 `false`를 반환하는 스텁
- **When**: `resolve_face(covers, 3, '🎉')`
- **Then**: `None`

#### R2.4: 정상 모드에서 미커버 문자는 그려지지 않는다
- **Given**: `Atlas::is_degraded() == false`이고 `'🎉'`를 어느 face도 커버하지 않음
- **When**: `Atlas::resolve('🎉')`
- **Then**: `None`을 반환하고, 호출자는 그 셀에 `GlyphInstance`를 emit하지 않는다

#### R2.5: degraded 모드에서는 .notdef가 유지된다
- **Given**: `Atlas::is_degraded() == true`이고 `'한'`을 프라이머리가 커버하지 않음
- **When**: `Atlas::resolve('한')` → `get_or_insert_styled(key)`
- **Then**: `Some(GlyphKey { face: 0, id: 0, .. })`이고, id-0 가드가 비활성이라 .notdef 비트맵이 팩된다

#### R2.6: 정상 모드에서 glyph id 0은 아틀라스에 들어가지 않는다
- **Given**: `is_degraded() == false`
- **When**: `get_or_insert_styled(GlyphKey { face: 0, id: 0, .. })`
- **Then**: `None`을 반환하고 아틀라스에 팩되지 않는다 (기존 테스트 `lib.rs:1190-1193`을 이 방향으로 재작성)

---

### R3: 아틀라스 face 축 · 용량 · 캐시 무효화 (D7, D13, D17)

#### R3.1: 같은 glyph id라도 face가 다르면 다른 캐시 엔트리다
- **Given**: 두 face를 가진 아틀라스
- **When**: `get_or_insert_styled(face:0, id:42)`와 `(face:1, id:42)`를 차례로 호출
- **Then**: `cached_len()`이 2 증가하고 두 엔트리의 UV가 서로 다르다

#### R3.2: 아틀라스는 폰트 바이트를 복사하지 않는다
- **Given**: `FontData = Arc<Mmap>`인 `FontSet`
- **When**: `Atlas::new(device, &font_set, ppem)`
- **Then**: `GlyphRasterizer`가 `Arc`를 클론할 뿐 `(*font).clone()` 딥카피를 하지 않는다

#### R3.3: 아틀라스 크기는 2048²이다
- **Given**: 갓 만든 아틀라스
- **When**: `ATLAS_SIZE`를 본다
- **Then**: `2048`. 기존 evict 회귀 테스트(`lib.rs:1311-1325`)는 새 용량 기준으로 조정돼 여전히
  "언젠가 evict가 일어나고 panic하지 않는다"를 검증한다

#### R3.4: 레이어가 evict를 유발하면 다음 프레임이 강제로 re-shape한다
- **Given**: 스냅샷이 변하지 않은 상태에서 `DrawLayer`가 아틀라스를 채워 `generation`이 오른다
- **When**: `draw_to`가 레이어 기록을 마친다
- **Then**: `last_snapshot`이 `None`이 되어 다음 프레임이 동일 스냅샷에도 인스턴스를 재빌드한다
  (`reshape_count`가 증가한다)

---

### R4: 행 셰이핑 — run 분할과 배치 (D16, D6)

#### R4.1: 연속 동일 face 문자는 하나의 run으로 묶인다
- **Given**: face 열 `[Some(0), Some(0), Some(1), Some(1), Some(0)]`
- **When**: `split_runs(&faces)`
- **Then**: `[Run{face:Some(0),0..2}, Run{face:Some(1),2..4}, Run{face:Some(0),4..5}]`

#### R4.2: 미커버 문자는 자기들끼리 run을 이룬다
- **Given**: face 열 `[Some(0), None, None, Some(1)]`
- **When**: `split_runs(&faces)`
- **Then**: 3개 run이 나오고 가운데 run의 `face == None`

#### R4.3: run 인덱스는 continuation 스킵 후 char 수열 기준이다
- **Given**: `한a` — 그리드상 열 0(한) / 1(continuation) / 2(a)
- **When**: 행을 훑어 `line`, `char_col`, `faces`를 만든다
- **Then**: `faces.len() == 2`, `char_col == [0, 2]` (스페이서는 세지 않는다)

#### R4.4: run마다 자기 face의 FontRef로 셰이핑된다
- **Given**: `Menlo한글` 한 줄
- **When**: `build_glyph_instances`가 실행된다
- **Then**: 라틴 run은 face 0 `FontRef`로, 한글 run은 face 1 `FontRef`로 셰이핑되고
  emit된 `GlyphKey.face`가 각각 0과 1이다

#### R4.5: 클러스터는 자기 리드 열에 앵커된다
- **Given**: run 경계를 넘는 행
- **When**: 클러스터의 `source.start`를 run-relative `byte_col`로 조회한다
- **Then**: 절대 열 번호가 나오고 `pen`이 `col * cw`에서 시작한다 (run 오프셋이 새지 않는다)

#### R4.6: 폴백 글리프는 자기 span 안에서 중앙 정렬된다
- **Given**: `한`이 열 4에 있고 열 5가 continuation, `cw`=10, 클러스터 총 advance=16
- **When**: 배치 오프셋을 계산한다
- **Then**: `offset = ((2*10 - 16) * 0.5).max(0.0) = 2.0`, `pen = 4*10 + 2 = 42`

#### R4.7: 글리프가 span보다 넓어도 왼쪽으로 번지지 않는다
- **Given**: span 2셀(=20px)인데 클러스터 총 advance가 24px
- **When**: 오프셋을 계산한다
- **Then**: `offset == 0.0` (`max(0.0)`이 음수를 막는다)

#### R4.8: face 0 클러스터는 오프셋 0이다
- **Given**: 프라이머리로 해석된 라틴 클러스터
- **When**: 배치한다
- **Then**: `offset == 0.0` — 기존 배치 산식과 동일 (R0.3의 근거)

---

### R5: 레이어 텍스트 경로도 폴백 + 폭 인식 (D10)

#### R5.1: 탭 제목이 폴백 face로 그려진다
- **Given**: cwd basename이 `프로젝트`인 탭
- **When**: `TabBar`가 기록된다
- **Then**: 각 한글 char에 대해 `ctx.atlas.resolve(c)`가 face 1 키를 주고 그 키로 인스턴스가 emit된다

#### R5.2: 레이어의 pen이 문자 폭만큼 전진한다
- **Given**: 제목 `프a`, `cw`=8
- **When**: 텍스트 인스턴스를 만든다
- **Then**: `프` 뒤 pen이 `+16`, `a` 뒤 pen이 `+8` 전진한다 (기존의 char당 고정 `+cw`가 아님)

#### R5.3: 제목 자르기가 열 예산 기준이다
- **Given**: `truncate_cols("한글abc", 5)`
- **When**: 호출한다
- **Then**: `"한글a"` (2+2+1 = 5열). char 수 기준이면 `"한글abc"`가 나와 사이드바를 넘친다

#### R5.4: 오버레이 텍스트도 같은 경로를 쓴다
- **Given**: 한글을 포함한 오버레이 라인
- **When**: `overlay_layer`가 기록된다
- **Then**: `resolve` + 폭 인식 전진을 쓰며, `tab_bar`와 동일한 함수/규칙을 공유한다

---

### R6: IME 배선 — 허용 · 이벤트 · 게이트 · 후보창 (D8, D12, D19)

#### R6.1: 창 생성 시 IME가 허용된다
- **Given**: `App::start`가 창을 만든다
- **When**: 창 생성 직후
- **Then**: `window.set_ime_allowed(true)`가 호출된다

#### R6.2: Commit 바이트가 PTY에 써진다
- **Given**: 조합중이 아닌 상태
- **When**: `WindowEvent::Ime(Ime::Commit("한".into()))`이 도착
- **Then**: `"한"`의 UTF-8 바이트가 대상 탭의 PTY writer에 써진다

#### R6.3: 조합중 문자키는 셸로 가지 않는다
- **Given**: `composing == true`
- **When**: `suppress_key(true, &Key::Character("ㅎ".into()))`
- **Then**: `true` — `encode_key` 경로를 타지 않는다

#### R6.4: 조합중에도 Named 키는 셸로 간다
- **Given**: `composing == true`
- **When**: `suppress_key(true, &Key::Named(NamedKey::ArrowLeft))` / `Enter` / `Backspace`
- **Then**: 모두 `false` — winit이 `doCommandBySelector`로 통과시킨 키를 잘못 삼키지 않는다

#### R6.5: 조합중이 아니면 아무것도 억제하지 않는다
- **Given**: `composing == false`
- **When**: `suppress_key(false, &Key::Character("a".into()))`
- **Then**: `false` (ASCII 경로 무변경)

#### R6.6: Preedit이 composing 상태를 켜고 끈다
- **Given**: `composing == false`
- **When**: `Ime::Preedit("한", Some(..))` → `Ime::Preedit("", None)`
- **Then**: 각각 `composing`이 `true` → `false`가 된다

#### R6.7: 입력 소스 전환이 composing 데드락을 만들지 않는다
- **Given**: `composing == true`인 상태에서 사용자가 한↔영을 전환
- **When**: `Ime::Disabled`가 도착 (빈 `Preedit` 없이)
- **Then**: preedit이 비워지고 `composing == false`가 된다 — 이후 문자키가 정상적으로 셸에 간다

#### R6.8: 후보창이 커서 위치에 뜬다
- **Given**: 커서가 (col 10, row 3)이고 `left_inset`이 112px인 상태
- **When**: preedit이 갱신된다
- **Then**: `set_ime_cursor_area`가 **`PhysicalPosition`/`PhysicalSize`** 로 호출되고 좌표에
  `left_inset`/`top_inset`이 반영된다

---

### R7: preedit 인라인 렌더 (D9, D14, D18)

#### R7.1: app이 preedit 문자열을 핸들로 밀어넣는다
- **Given**: `PreeditLayer::new()`가 준 핸들을 `App`이 들고 있다
- **When**: `Ime::Preedit("한", ..)`이 도착
- **Then**: 핸들의 문자열이 `"한"`으로 갱신되고 redraw가 요청된다

#### R7.2: 레이어가 커서 위치에 preedit을 그린다
- **Given**: 핸들에 `"한"`, 스냅샷 커서가 (col 5, row 2)
- **When**: 프레임을 그린다
- **Then**: 열 5부터 2셀 범위에 배경 quad + 글리프 + 밑줄이 z=6으로 기록된다

#### R7.3: preedit이 커서 블록에 가려지지 않는다
- **Given**: `CURSOR_Z == 5`
- **When**: 레이어를 z 순으로 정렬한다
- **Then**: preedit 레이어의 z가 `6`으로 커서보다 뒤에(위에) 기록된다

#### R7.4: 배경과 밑줄이 그리드 규칙을 따른다
- **Given**: 셀 높이 `ch`
- **When**: preedit을 그린다
- **Then**: 배경은 테마 기본 bg로 칠해지고, 밑줄 두께는 `(ch*0.06).max(1.0)`,
  베이스라인 아래 오프셋은 `(ch*0.08).max(1.0)` — `lib.rs:908,910`과 같은 값

#### R7.5: preedit 색이 config 리로드에 따라간다
- **Given**: 실행 중 `config.toml`의 테마가 바뀐다
- **When**: `settings::apply`가 실행된다
- **Then**: `PreeditHandle`의 `bg`/`fg`도 새 테마 값으로 갱신된다

#### R7.6: 인셋이 반영된다
- **Given**: `left_inset`이 0 → 112px로 바뀐다
- **When**: 같은 커서 위치로 preedit을 그린다
- **Then**: 그려지는 x가 112px 만큼 이동한다 (`cursor_layer`의
  `block_rect_shifts_with_left_inset`와 같은 형태의 회귀 테스트)

#### R7.7: 스크롤백을 보는 중에는 그리지 않는다
- **Given**: `snapshot.at_bottom == false`
- **When**: preedit이 비어 있지 않은 상태로 프레임을 그린다
- **Then**: preedit 인스턴스가 하나도 기록되지 않는다

#### R7.8: 랩 대기 커서와 행 오버플로가 정의대로 처리된다
- **Given**: `cursor.col == cols` 이거나 preedit이 남은 열보다 길다
- **When**: preedit을 배치한다
- **Then**: 시작 열은 `cols - 1`로 클램프되고, 남은 열을 넘는 부분은 행 끝에서 잘린다
  (줄바꿈/스크롤 없음)

---

### R8: 조합 취소와 커밋 라우팅 (D11)

#### R8.1: 조합중 탭 전환이 preedit을 즉시 정리한다
- **Given**: `composing == true`인 상태
- **When**: `do_tab_action`이 실행된다
- **Then**: preedit 핸들이 비고 `composing`이 **그 자리에서** false가 된다 (큐잉된
  `Ime::Disabled`를 기다리지 않는다). **`composing_tab`은 의도적으로 유지된다** — 해제는
  `Ime::Commit`이 바이트를 배치한 직후 · `Ime::Disabled` · 새 조합의 시작, 이 셋뿐이다
  (검증 루프에서 D11과 함께 개정: 취소 시 소유권까지 버리면 뒤늦게 도착한 커밋이 사용자가
  방금 옮겨간 탭으로 새어 들어간다)

#### R8.2: OS 조합이 실제로 취소된다
- **Given**: `composing == true`인 상태에서 탭 전환 또는 `Focused(false)`
- **When**: 취소 경로가 실행된다
- **Then**: `NSView::inputContext().discardMarkedText()`가 호출된다
  (`set_ime_allowed` 토글이 아니라 — 토글은 winit 자체 ivar만 비운다)

#### R8.3: 커밋은 조합이 시작된 탭으로 간다
- **Given**: 탭 A에서 조합을 시작해 `composing_tab == Some(A)`
- **When**: 탭 B로 전환된 뒤 `Ime::Commit`이 도착한다
- **Then**: 바이트가 **탭 A**의 PTY에 써진다 (탭 B가 아니다)

#### R8.4: 조합 탭이 닫혔으면 커밋은 버려진다
- **Given**: `composing_tab == Some(A)`이고 탭 A가 닫혔다
- **When**: `Ime::Commit`이 도착한다
- **Then**: 아무 PTY에도 써지지 않고 조용히 버려진다

#### R8.5: 평범한 커밋은 활성 탭으로 간다
- **Given**: `composing_tab == None` (조합을 거치지 않은 입력)
- **When**: `Ime::Commit("a")`이 도착한다
- **Then**: 활성 탭의 PTY에 써진다 — **문자가 유실되지 않는다**

---

### R9: 결함③ — PTY 로케일 (D15)

#### R9.1: 로케일이 전혀 없으면 LC_CTYPE을 주입한다
- **Given**: `LC_ALL`/`LC_CTYPE`/`LANG`이 모두 부재
- **When**: `locale_action(get)`
- **Then**: `LocaleAction::Inject("LC_CTYPE", "UTF-8")`

#### R9.2: 빈 문자열은 부재로 친다
- **Given**: `LANG == Some("")`, 나머지 부재
- **When**: `locale_action(get)`
- **Then**: `Inject(..)` — `Some("")`은 설정된 것이 아니다

#### R9.3: UTF-8 로케일은 건드리지 않는다
- **Given**: `LANG == Some("ko_KR.UTF-8")`
- **When**: `locale_action(get)`
- **Then**: `LocaleAction::Ok` — 주입하지 않는다

#### R9.4: 비-UTF8 로케일은 존중하되 경고한다
- **Given**: `LC_ALL == Some("C")`
- **When**: `locale_action(get)`
- **Then**: `LocaleAction::NonUtf8` — 덮어쓰지 않고, `Pty::spawn`이 프로세스당 **1회**만
  `tracing::warn!`을 남긴다 (탭 10개를 열어도 경고는 1줄)

#### R9.5: 주입이 실제 자식 환경에 반영된다
- **Given**: 로케일 없는 환경에서 `Pty::spawn`
- **When**: zsh가 뜬다
- **Then**: `CommandBuilder`에 `LC_CTYPE=UTF-8`이 설정돼 zsh가 `한글`을 6자가 아닌 **2자**로 센다

#### R9.6: 우선순위가 POSIX 규칙을 따른다
- **Given**: `LC_ALL == Some("C")`이고 `LANG == Some("ko_KR.UTF-8")`
- **When**: `locale_action(get)`
- **Then**: `NonUtf8` — `LC_ALL`이 `LANG`을 이긴다

---

### 커버리지 자체 점검
- **결정 → 요구사항**: D1→R1, D2→R2, D3→R1, D4→R1, D5→R2, D6→R4, D7→R3, D8→R6, D9→R7,
  D10→R5, D11→R8, D12→R6, D13→R3, D14→R7, D15→R9, D16→R4, D17→R3, D18→R7, D19→R6 — **고아 없음**.
- **경계 점검**: `app`이 만들고 `renderer`가 소비하는 preedit은 양쪽에 서브요구가 있다
  (R7.1 생산 / R7.2 소비). `Atlas::resolve`는 생산(R2.4-2.6)과 소비(R4.4, R5.1) 양쪽에 있다.
  `FontSet`은 로딩(R1)과 소비(R3.2, R4.4) 양쪽에 있다.
- **GWT 완전성**: 모든 서브요구가 Given/When/Then 3필드를 갖는다.

## Tasks

### T1: `term-core` PTY 로케일 주입 (결함③) [infra]
- **Fulfills**: R9
- **Depends on**: (none) — 다른 모든 태스크와 병렬
- **Touches**: `crates/term-core/src/pty.rs` (+ 같은 파일 안 `#[cfg(test)]`)
- `LocaleAction` 3상태 enum + `locale_action(get)` 순수 함수 + `Pty::spawn` 배선 +
  `Once` 기반 1회 경고. 표 기반 단위 테스트(부재 / `Some("")` / UTF-8 / `C` / `LC_ALL` 우선순위).

### T2: `FontSet` — mmap 로딩 · TTC 프로브 · degraded [infra]
- **Fulfills**: R1
- **Depends on**: (none) — T1/T7과 병렬
- **Touches**: `crates/renderer/Cargo.toml`(memmap2 추가), `crates/renderer/src/lib.rs`
  (`FONT_CANDIDATES`/`FALLBACK_CANDIDATES`/`load_font` → `FontSet::load`), `Renderer.font` 필드 교체
- `FontFace { data: Arc<Mmap>, index }`, `FontSet { faces }`, `is_degraded()`.
  `unsafe { Mmap::map(..) }`는 `load_font` 한 곳 + `// SAFETY:` 주석.
  프로브(`'A'` / `'가'`)로 TTC face 선택. 시스템 폰트 의존 테스트는 기존 관행대로
  `#[ignore = "depends on a system font path; run with --ignored"]`.

### T3: 아틀라스 face 축 · `resolve` · degraded · 2048² [infra]
- **Fulfills**: R2, R3.1, R3.2, R3.3
- **Depends on**: T2 (`FontSet` 타입 필요)
- **Touches**: `crates/renderer/src/atlas.rs`, `crates/renderer/src/glyph.rs`
- `GlyphKey`에 `face` 추가, `Atlas { rasterizers: Vec<GlyphRasterizer> }`,
  `Atlas::new(device, &FontSet, ppem)`, `resolve(ch) -> Option<GlyphKey>`,
  `is_degraded()`, `!is_degraded()` 조건부 id-0 가드, `ATLAS_SIZE = 2048`.
  `GlyphRasterizer::new`가 `Arc<Mmap>`을 받도록 시그니처 변경(딥카피 제거).
  기존 evict 테스트(`lib.rs:1311-1325`)와 .notdef 테스트(`lib.rs:1190-1193`) 갱신.

### T4: 그리드 셰이핑 — run 분할 · 중앙 정렬 · re-shape 무효화 [vertical]
- **Fulfills**: R0.2, R0.3, R4, R3.4
- **Depends on**: T3
- **Touches**: `crates/renderer/src/lib.rs` (`build_glyph_instances`, `draw_to`, `apply_config`)
- `split_runs` 순수 함수 + `char_col` 병렬 배열 + run별 `ShapeContext` 셰이핑 +
  run-relative `byte_col` + D6 중앙 정렬 산식. `draw_to`에서 `generation` 변화 시
  `last_snapshot = None`(그리고 `terminal_app.rs:428-429` 연속 redraw 의존을 주석에 명시).
  `apply_config`의 `Atlas::new` 호출부를 `FontSet` 시그니처로 갱신.

### T5: 레이어 텍스트 — 폴백 + 폭 인식 [vertical]
- **Fulfills**: R5
- **Depends on**: T3 — **T4와 병렬** (파일 겹침 없음: `tab_bar.rs`/`overlay_layer.rs` ↔ `lib.rs`)
- **Touches**: `crates/renderer/src/tab_bar.rs`, `crates/renderer/src/overlay_layer.rs`,
  `crates/renderer/Cargo.toml`(unicode-width `"0.2.0"` 핀)
- `glyph_id`+`get_or_insert` → `resolve`+`get_or_insert_styled`,
  `pen += ch.width().unwrap_or(1) as f32 * cw`, `truncate` → `truncate_cols`(열 예산).
  두 레이어가 같은 헬퍼를 공유한다.

### T6: IME 배선 — 허용 · 이벤트 · 게이트 · 후보창 [vertical]
- **Fulfills**: R6, R0.1
- **Depends on**: (none) — T1/T2와 병렬 (`app` 크레이트만 건드림)
- **Touches**: `crates/app/src/terminal_app.rs`, `crates/app/src/input.rs`
- `start()`에 `set_ime_allowed(true)`. `window_event`에 `WindowEvent::Ime` arm
  (Commit / Preedit / Enabled / Disabled). `input.rs`에 순수 함수
  `suppress_key(composing, key)` + 단위 테스트. `set_ime_cursor_area`를
  `PhysicalPosition`/`PhysicalSize`로 호출.

### T7: preedit 인라인 렌더 레이어 [vertical]
- **Fulfills**: R7
- **Depends on**: T3(폴백 `resolve` 필요), T4(`lib.rs`의 `pub mod` 선언 충돌 회피), T6(`Ime` arm 필요)
- **Touches**: `crates/renderer/src/preedit_layer.rs`(신규), `crates/renderer/src/lib.rs`(mod 선언),
  `crates/app/src/terminal_app.rs`(핸들 배선), `crates/app/src/settings.rs`(테마 갱신)
- `PreeditLayer::new() -> (PreeditLayer, PreeditHandle)` — `TabModelHandle` 패턴 복제.
  핸들이 문자열 + `bg`/`fg`를 나른다. z=6, 인셋 반영, 배경 quad + 글리프 + 밑줄
  (`lib.rs:908,910` 상수 재사용), `at_bottom == false` 미출력, `col == cols` 클램프, 행 끝 클립.
  `block_rect_shifts_with_left_inset`와 같은 형태의 인셋 회귀 테스트.

### T8: 조합 취소 + 탭 정체성 커밋 라우팅 [vertical]
- **Fulfills**: R8
- **Depends on**: T7 (preedit 핸들을 비워야 하므로)
- **Touches**: `crates/app/src/terminal_app.rs`
- `composing_tab: Option<TabId>` + `Ime::Commit`의 탭 라우팅.
  `do_tab_action`/`Focused(false)`에서 인라인 정리 + `winit::raw_window_handle`의
  `AppKitWindowHandle`로 `ns_view` 취득 → `objc2_app_kit::NSView::inputContext()`
  → `discardMarkedText()`. 포인터→`&NSView` 캐스트만 `unsafe` + `// SAFETY:` 주석.

### DAG

```
T1 (term-core 로케일)  ─────────────────────────────┐   ← 완전 독립
T2 (FontSet/mmap) ── T3 (Atlas face) ─┬─ T4 (셰이핑) ─┐
                                      └─ T5 (레이어)   │   ← T4 ∥ T5
T6 (IME 배선) ────────────────────────────────────────┴─ T7 (preedit) ── T8 (취소/라우팅)
```

- 1차 병렬: **T1 ∥ T2 ∥ T6** (각각 term-core / renderer / app — 크레이트 자체가 겹치지 않음)
- 2차 병렬: **T4 ∥ T5** (둘 다 T3 이후, 편집 파일이 서로 배타적)
- 순환 없음. 모든 요구사항이 최소 1개 태스크에 매핑됨:
  R0→T4·T6, R1→T2, R2→T3, R3→T3·T4, R4→T4, R5→T5, R6→T6, R7→T7, R8→T8, R9→T1.

## External Dependencies

### Pre-work
- (none) — 필요한 시스템 폰트는 이 머신에서 존재를 이미 확인했다
  (`AppleSDGothicNeo.ttc` 55,373,848 B / `Supplemental/AppleGothic.ttf` 15,255,648 B).
  새 crate 2개(`memmap2`, `renderer`의 `unicode-width`)는 cargo가 자동으로 받는다.
  단 `Cargo.lock` 편집은 훅이 차단하므로 **cargo가 갱신하게 둔다**.

### Post-work
- `/run-term`으로 육안 확인 (L2에서 합의한 검증 수준):
  ① 한글 타이핑 → 조합된 음절 + 실제 글리프 ② 한글 명령줄에서 백스페이스/커서 이동
  ③ 조합중 탭 전환 후 새 탭이 오염되지 않음 ④ 한글 cwd에서 연 탭 제목
- `cargo build` / `cargo test --workspace` /
  `cargo clippy --workspace --all-targets -- -D warnings` 전부 green 확인
- **PR 오픈** — 제목에 결함①②를 명시하고, 인터뷰 중 발견된 결함③(로케일)도 본문에 기술

## Known Gaps
- 아틀라스의 **프레임 *내* evict UV 무효화**(atlas.rs:193-200) 근본 수정은 범위 밖 — D13(2048²)은
  발생 확률을 낮출 뿐이다. (프레임 *간* 영구 손상 경로는 D17이 닫는다.) 별도 이슈로 남긴다.
- 기존 evict 회귀 테스트(`lib.rs:1311-1325`)는 1024² 기준 루프 횟수를 가정 — D13에 맞춰 조정 필요.
- **CoreText 캐스케이드 질의**(`CTFontCopyDefaultCascadeListForLanguages`)는 채택하지 않았다 —
  경로 하드코딩은 Apple의 폰트 재배치에 취약하다. D1(후보 2개) + D3(프로브) + D5(degraded
  .notdef)로 완화했을 뿐, 근본 해결이 아니다. 가장 유력한 후속 작업.
- 폴백 체인이 config로 커스터마이즈되지 않는다(상수 배열 고정) — Non-goal이지만 후속 후보.
- 이모지 등 어떤 face도 커버 못하는 문자는 정상 모드에서 **보이지 않는다**(D5). 컬러 이모지는
  아틀라스가 R8Unorm 알파 전용이라 별도 feature.
- `GlyphRasterizer`가 face마다 `FontRef::from_index`를 매 호출 재구성하는 기존 비용은 face 수만큼
  늘어난다 — 측정 전까지 최적화하지 않는다(G1: 성능 수치 미확정).
- mmap한 폰트 파일을 OS 업데이트가 도중에 교체하면 매핑이 무효해질 수 있다(SIGBUS). 실사용에서는
  시스템 폰트가 실행 중 교체되지 않으므로 수용하고, 발생 시 재현 경로만 기록한다.
- **취소 후 소유권 잔류 창(D11 개정이 감수한 비용)**: `cancel_composition`은 `composing_tab`을
  일부러 유지한다(R8.1). `discardMarkedText`가 IME 서버에 먹히지 않으면 winit의 `marked_text`
  ivar가 stale-true로 남고, 그 상태에서 **입력 소스를 바꾸지 않은 채**(→ `Ime::Disabled`가 오지
  않음) 다음 글자를 치면 그 한 글자가 `Commit`으로 올라와 **조합을 시작했던 탭**으로 간다 —
  그 탭이 이미 닫혔다면 버려진다. 최대 한 글자이며(어떤 `Commit`이든 소유권을 해제한다),
  개정 전 코드였다면 활성 탭으로 갔을 입력이다. 교차탭 오염(더 나쁜 쪽)을 막기 위한 맞교환으로
  수용한다. 완전히 닫으려면 `ApplicationHandler::about_to_wait`에서 소유권을 해제하면 된다 —
  D11이 방어하는 "이미 큐에 올라간 커밋"은 탭 전환과 **같은 이벤트 배치**에서 디스패치되므로
  배치 끝에서 해제해도 R8.3/R8.4는 온전히 유지된다. 후속 작업 후보.
