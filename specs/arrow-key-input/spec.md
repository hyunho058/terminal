# Spec: arrow-key-input

## Meta
- **Created**: 2026-07-19
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-07-19
- **Complexity**: Medium (L2-reviewer가 Simple에서 상향) — 신호 4개:
  ① 모드 플래그가 `term-core → app` 경계를 넘음, ② `Perform::esc_dispatch`라는
  **새 파서 콜백 표면**, ③ `{normal, application} × {plain, modified} × 5개 키 계열`의
  인코딩 매트릭스, ④ `encode_key` 시그니처 변경(기존 테스트 동반 수정) — 2개
  크레이트 4개 파일에 걸침.

## Goal
방향키(↑↓←→)가 동작하지 않는 문제를 고친다. 원인은 `crates/app/src/input.rs`의
`encode_key`에 방향키/네비게이션 키 arm이 없다는 것 — winit이 이 키들에 `text`를 주지
않아 `_ => text.map(...)`가 `None`을 반환하고, `on_key`(terminal_app.rs:286)가 PTY에
아무것도 쓰지 않는다. 그래서 TUI(claude 등)가 커서 이동 시퀀스를 전혀 받지 못한다.

수정 범위:
1. `encode_key`에 arm 추가 — ↑=`ESC[A` ↓=`ESC[B` →=`ESC[C` ←=`ESC[D` (normal),
   application cursor mode면 `ESC O A/B/C/D`. 함께 Home/End/PageUp/PageDown/
   Delete/Insert, F1–F12, 수정키 조합(Ctrl/Alt-화살표)도.
2. `term-core`에 DECCKM(application cursor key mode) 추적 추가 — 현재 전무.
   `ESC[?1h` set / `ESC[?1l` reset을 파서에서 처리하고 app에 노출 → app이
   normal/application 변형을 골라 전송.

## Non-goals
- Kitty keyboard protocol / `modifyOtherKeys` (XTMODKEYS) 확장 인코딩 — 현재 파서는
  `CSI > … m`을 의도적으로 무시한다(`parser.rs:273`). 그 확장은 별도 re-scope.
- DECNKM(application **keypad** mode, `ESC=` / `ESC>`)과 numpad 인코딩.
- 마우스 리포팅, bracketed paste, focus 리포팅 등 다른 미구현 입력 모드.
- 스크롤백을 키보드에 바인딩하는 것 (PageUp/PageDown은 셸로 보낸다, 뷰포트 스크롤이
  아니다).
- 크로스플랫폼 키 매핑 — macOS ARM64 전용(`macos-only` rule).

## Confirmed Goal
커서/네비게이션 키가 실제로 셸에 도달하게 만든다.

- `crates/app/src/input.rs`: `encode_key`에 화살표 4종, Home/End/PageUp/PageDown/
  Delete/Insert, F1–F12, 그리고 Ctrl/Alt/Shift 수정자 조합의 인코딩을 추가한다.
- `crates/term-core`: DECCKM(`ESC[?1h` / `ESC[?1l`)을 추적하고, `app`이 normal
  (`ESC[A`) vs application (`ESC O A`) 변형을 고를 수 있도록 조회 경로를 노출한다.
- DECCKM은 **탭별(per-session) 상태**다 — 각 탭이 자기 PTY와 `Terminal`을 소유하므로
  (`session.rs:23-31`), 조회 경로는 활성 탭의 `Terminal`을 통해야 한다.
- `term-core`는 GPU-free를 유지한다(`dependency-direction` rule).

**Done when**: zsh 라인 편집에서 화살표가 커서를 옮기고, 풀스크린 TUI(claude 신뢰
프롬프트)에서도 옮긴다. `cargo test --workspace` 그린 — term-core에 headless DECCKM
토글 테스트, `input.rs`에 방향키/네비게이션 인코딩 유닛테스트 포함.
`cargo clippy --workspace --all-targets -- -D warnings` 클린. `/run-term`으로 육안 검증.

## Research

### 근본 원인 (확인됨)
- `encode_key`는 Enter/Backspace/Tab/Escape/Space 5개 arm만 갖고, 나머지는
  `_ => text.map(|t| t.as_bytes().to_vec())`로 폴백한다 (`crates/app/src/input.rs:65-74`).
  winit은 `NamedKey::ArrowUp` 등에 `text`를 주지 않으므로 → `None`.
- 유일한 호출부: `crates/app/src/terminal_app.rs:286` — `if let Some(bytes) =
  encode_key(...)`. `None`이면 PTY 쓰기가 통째로 스킵된다. 시그니처를 바꿔도
  고칠 곳은 이 한 줄.
- `KeyInput`은 이미 `key` / `text` / `mods`를 전부 들고 있다 (`crates/app/src/seam.rs:38-42`),
  그리고 `Running.mods`가 `ModifiersState`를 추적한다 (`terminal_app.rs:93`). 즉 수정자
  조합에 필요한 입력은 이미 호출부에 있고, `encode_key`가 안 받고 있을 뿐이다.

### DECCKM이 들어갈 자리 (기존 선례 그대로)
- 프라이빗 모드 arm이 이미 있다: `intermediates.first() == Some(&b'?') && matches!(action, 'h' | 'l')`
  → 현재 `1049 | 1047 | 47`(alt-screen)만 처리하고 **나머지 프라이빗 모드는 전부 무시하고
  early-return** 한다 (`crates/term-core/src/parser.rs:257-266`). `?1`은 이 블록 안에
  arm 하나 추가하는 일이다.
- alt-screen이 정확한 선례 형태를 보여준다: `Grid`의 `bool` 필드 + `set_alt_screen`/
  `alt_screen` 접근자 (`crates/term-core/src/grid.rs:152, 379-384`) → `Terminal::alt_screen()`
  퍼사드 (`crates/term-core/src/lib.rs`) → headless 테스트에서
  `t.advance(b"\x1b[?1049h"); assert!(t.alt_screen());` (`crates/term-core/tests/scrollback.rs:129-148`).
- **`alt_screen`은 `GridSnapshot`에 없다** (`crates/term-core/src/snapshot.rs:12-28`) —
  렌더러가 아니라 로직이 읽는 상태이기 때문. DECCKM도 같은 부류(입력 인코딩이 읽음)이라
  스냅샷 확장이 아닌 `Terminal` 접근자가 선례에 맞는다.

### 탭/세션 구조
- `Session`은 자기 `Terminal` + `Pty` + writer를 소유한다 (`crates/app/src/session.rs:23-31`).
  탭마다 zsh가 따로 뜨므로 DECCKM도 탭마다 다르다.
- `on_key`의 기본 경로는 `self.tabs.active_mut().write(&bytes)`로 활성 탭에만 쓴다
  (`terminal_app.rs:286-290`). 활성 탭의 `terminal`을 읽어 플래그를 얻는 자연스러운 지점.

### 충돌 검사
- `crates/app/src`에 마우스 휠/스크롤 입력 처리가 **전혀 없다** (`Wheel|Delta|scroll` grep 무결과).
  PageUp/PageDown/Home/End은 현재 어떤 것에도 바인딩되어 있지 않으므로 셸로 보내도 충돌 없음.
- 탭 코드는 Cmd(super) 조합만 가로챈다 (`input.rs:26-42`) — Ctrl/Alt-화살표와 겹치지 않는다.
- `on_key`의 디스패치 순서는 오버레이 → 등록 핸들러 → 탭 코드 → `encode_key`
  (`terminal_app.rs:254-292`). 이 스펙은 4단계만 바꾼다. 현재 등록된 것 중
  화살표를 소비하는 것은 **없다**: `OverlayHost::dispatch`는 `o.visible()` 가드를
  거치고 (`seam.rs:156`), `ImagePresenter::visible()`은 `false` 고정,
  `AiMonitorOverlay`는 자기 토글 외에는 통과시킨다. `ClipboardHandler`는 Cmd+V만
  소비한다.
- `tab_action`은 `Key::Character`만 본다 (`input.rs:30-33`) — 따라서 **`Key::Named`인
  화살표는 탭 코드 단계를 그냥 통과**한다. Cmd+화살표를 삼키는 일은 `encode_key`
  안에서 일어나야 한다 (D15).

### 실측 증거 — 대상 환경에서 zsh가 실제로 하는 일
스펙 결정이 추정이 아니라 측정 위에 서도록, 이 저장소의 대상(zsh 5.9,
arm64-apple-darwin25, `TERM=xterm-256color`)에서 직접 확인했다.

- **zsh는 DECCKM을 켜지 않는다.** pty 위에서 프롬프트 사이클 3회를 캡처한 결과
  `ESC[?2004h`/`ESC[?2004l`(bracketed paste)는 각 3회, **`ESC[?1h`는 0회,
  `ESC=`도 0회**. 즉 zsh 프롬프트에서 DECCKM은 항상 **off**이고 →
  **normal CSI(`ESC[A`)가 1차 경로**다. 동시에, zsh가 DECCKM을 만지지 않으므로
  **TUI가 `ESC[?1l` 없이 죽으면 그 상태는 실제로 남는다.** 측정된 것은 여기까지 —
  *상태의 지속*이다. 그것이 사용자에게 보이는 고장이 되는지는 별개이고, 아래
  bindkey 실측이 보여주듯 zsh 프롬프트에서는 증상이 없다 (D9 참조).
- **zsh 기본 emacs 키맵의 바인딩** (`zsh -ic bindkey`):
  `^[[A`/`^[OA`, `^[[B`/`^[OB`, `^[[C`/`^[OC`, `^[[D`/`^[OD` — 화살표는 **양쪽
  형식 모두** 바인드됨. `^[[1~`→`beginning-of-line`, `^[[4~`→`end-of-line`,
  `^[[3~`→`delete-char`, `^[[200~`→`bracketed-paste`.
  **`^[[H` / `^[[F` / `^[OH` / `^[OF`는 아무것에도 바인드되어 있지 않다.**
- **terminfo와 실제 바인딩은 다르다.** `infocmp xterm-256color`는
  `kcuu1=\EOA`, `khome=\EOH`, `kend=\EOF`, `smkx=\E[?1h\E=`, `rmkx=\E[?1l\E>` —
  전부 SS3 형식이다. terminfo 항목은 "`smkx`를 적용한 뒤"를 전제하는데 zsh는
  `smkx`를 보내지 않는다. 따라서 **terminfo 값을 그대로 보내는 것은 근거가 아니며**,
  판단 기준은 zsh가 실제로 바인드한 시퀀스 + xterm의 실제 동작이다 (D11이 이 증거로
  뒤집혔다).
- 부수 확인: `ESC=`/`ESC>`(DECKPAM/DECKPNM)는 vim 등이 terminfo `smkx`를 통해
  보낼 수 있으므로 새로 생기는 `esc_dispatch`에 도달한다 → 반드시 무해한 no-op여야
  한다 (D9).

### 빌드/테스트/린트
- `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings` (CLAUDE.md).
- `input.rs`에 이미 `#[cfg(test)] mod tests`가 있어 `encode_key` 유닛테스트를 붙일 자리가 있다
  (`crates/app/src/input.rs:76-138`, 기존 `encode_named_and_text` 테스트).
- term-core 통합 테스트는 `crates/term-core/tests/*.rs`에 headless로 존재
  (`scrollback.rs`가 모드 토글 테스트의 형태를 보여준다).
- PostToolUse 훅이 편집된 `*.rs`에 `rustfmt`를 돌린다.

### Tier-2 검증 결과 (코드로 확인)
- `crates/term-core/src/parser.rs`의 `impl Perform for Performer`는 `print` /
  `execute` / `csi_dispatch` **셋만** 구현한다 — `esc_dispatch`가 없다.
  따라서 **RIS(`ESC c`)는 현재 완전한 no-op**이다. "RIS가 DECCKM을 리셋한다"는
  `esc_dispatch` arm을 새로 추가하지 않으면 구현 불가능하다.
- `Performer`는 `&mut Grid` 하나만 들고 있다 (`parser.rs:136-139`), 그리고
  `flush()`가 매 청크마다 새로 만든다 (`parser.rs:128-134`). 모드 상태를
  `Processor`에 두려면 `flush`/`Performer`에 두 번째 필드를 배선해야 하고,
  `Grid`에 두면 배선이 0이다 (alt-screen과 동일).

## Decisions

### D1: 화살표 인코딩 — normal은 CSI, application은 SS3
- **Status**: resolved
- **Rationale**: normal cursor key mode에서 ↑↓→← = `ESC[A`/`ESC[B`/`ESC[C`/`ESC[D`,
  DECCKM이 켜지면 `ESC O A`/`B`/`C`/`D`. DEC/xterm 표준이며 zsh ZLE와 모든 TUI가
  이걸 기대한다. 대안이었던 "항상 CSI만 전송"은 기각 — application mode를 쓰는 TUI가
  화살표를 인식하지 못하거나(readline 계열) `[`를 리터럴로 먹는 오작동이 난다.

### D2: DECCKM은 `Terminal` 접근자로 노출한다 — `GridSnapshot`을 확장하지 않는다
- **Status**: resolved
- **Rationale**: `alt_screen`이 정확히 같은 부류(렌더러가 아니라 로직이 읽는 모드
  상태)이고, 이미 `Terminal::alt_screen()` 접근자로만 노출되며 `GridSnapshot`에는
  없다 (`snapshot.rs:12-28`). 기각된 대안: (a) `GridSnapshot`에 필드 추가 —
  스냅샷은 "렌더러가 소비하는 버전드 계약"인데 렌더러는 이 값을 절대 안 읽으므로
  계약 오염이고, 프레임마다 클론되는 타입을 입력 인코딩 때문에 넓히게 된다.
  (b) `app` 쪽에 별도 플래그를 두고 PTY 바이트를 두 번 스캔 — 파싱 중복이자
  term-core가 이미 도는 vte 상태기계와 어긋날 수 있다.

### D3: DECCKM은 지속된다 — `ESC[?1l` 또는 RIS만 normal로 되돌린다
- **Status**: resolved
- **Rationale**: xterm/iTerm2와 동일한 예측 가능한 의미론. TUI가 `ESC[?1l` 없이
  죽으면 탭은 application mode로 남고, 사용자는 `reset`(RIS)으로 복구한다.
  기각: (a) alt-screen 퇴장(`ESC[?1049l`) 시 자동 리셋 — 실전 복구력은 좋지만
  alt-screen 없이 DECCKM만 쓰는 라인 기반 TUI에서 잘못 리셋되어 멀쩡한 앱의
  화살표를 깨뜨린다(우리가 만든 버그가 됨). (b) 셸 프롬프트 감지 후 리셋 —
  휴리스틱 오탐이 크고 term-core에 셸 지식이 스며든다.

### D4: Cmd+화살표는 삼키고, Ctrl/Alt/Shift는 xterm 수정자 파라미터로 보낸다
- **Status**: resolved
- **Rationale**: `CSI 1;{m}{A|B|C|D}`, `m = 1 + (Shift 1) + (Alt 2) + (Ctrl 4)` —
  Shift+← = `ESC[1;2D`, Ctrl+→ = `ESC[1;5C`, Alt+↑ = `ESC[1;3A`. TUI가 실제로
  파싱하는 표준 형식. Cmd(super)+화살표는 셸로 보내지 않고 삼킨다 — macOS 네이티브
  관용(Cmd = 앱 레벨 명령)에 맞고, 향후 앱 레벨 바인딩 자리를 남긴다.
  기각: (a) Cmd를 Meta 비트(9)로 매핑 — 표준이 아니라 TUI 지원이 드물고 Cmd 자리를
  영구히 소모한다. (b) 수정자 전부 무시하고 맨 화살표 전송 — 수정자 정보가 소실되어
  TUI가 Ctrl+←와 ←를 구별할 수 없다.
  **정확히 말하면**: `ESC[1;5D`를 보내는 것 자체가 zsh에서 단어 이동을 만들어주지는
  않는다 — zsh 기본 키맵에 `^[[1;5D` 바인딩이 없어 사용자가 `bindkey`를 추가해야
  한다(실측 확인). 이 결정의 근거는 "프로토콜 상 올바른 바이트를 보내 TUI가 구별할
  수 있게 한다"이지 "zsh에서 즉시 단어 이동이 된다"가 아니다.

### D5: 수정자가 붙으면 application mode여도 항상 CSI 형식을 쓴다
- **Status**: assumed
- **Rationale**: xterm의 실제 동작이다 — SS3(`ESC O`) 형식에는 파라미터를 실을
  자리가 없으므로, DECCKM이 켜져 있어도 수정자가 있는 순간 CSI 형식으로 넘어간다.
  선택지가 아니라 프로토콜의 귀결이라 자동 확정. (`ESC O 1;5 C` 같은 변형을
  만들어내면 어떤 TUI도 파싱하지 못한다.)
  **적용 범위는 화살표만이 아니다** — SS3를 쓰는 모든 키(화살표, F1–F4)에 동일하게
  적용된다. 구체적인 per-family 형식은 D13이 정의한다.

### D6: 매핑되지 않은 키는 기존 `text` 폴백을 그대로 유지한다
- **Status**: assumed
- **Rationale**: `_ => text.map(...)` arm이 글자/기호/IME(한글 조합) 출력을 나르는
  경로다 (`input.rs:72`). 새 arm은 그 앞에 얹기만 하고 폴백은 건드리지 않는다 —
  IME 회귀를 원천 차단한다.

### D7: DECCKM은 세션(탭)별 상태다
- **Status**: assumed
- **Rationale**: 각 `Session`이 자기 `Terminal`을 소유하므로 (`session.rs:23-31`)
  플래그는 자연히 탭별이다. `on_key`는 활성 탭의 `terminal`에서 읽고 활성 탭의
  writer로 쓴다 — 앱 전역 플래그를 두면 vim이 열린 탭과 셸 탭이 서로를 오염시킨다.

### D8: 새 의존성 없음 — `term-core`는 GPU-free를 유지한다
- **Status**: assumed
- **Rationale**: 전부 상수 바이트 테이블과 `bool` 하나다. `dependency-direction`,
  `no-gui-crates`, `macos-only` 룰을 그대로 지킨다.

### D9: `esc_dispatch`를 신설하되 RIS(`ESC c`)는 **DECCKM만** 리셋한다 (최소 RIS)
- **Status**: resolved *(L2-reviewer의 steelman으로 전체 RIS에서 축소 — 개정됨)*
- **Rationale**: D3의 복구 경로("`reset`으로 되살린다")는 `esc_dispatch` 없이는
  **구현 불가능**하다 — 현재 `Perform` 구현에 `esc_dispatch`가 아예 없어 `ESC c`가
  no-op다. 따라서 `esc_dispatch`를 추가하고 `'c'` arm에서 `app_cursor_keys = false`
  **만** 수행한다. 화면 clear·이미지 삭제·alt-screen 강제 해제·pen 리셋은 하지 않는다.
  - **왜 전체 RIS를 접었나 (reviewer steelman, 채택)**: 전체 RIS는 복구를 사는 대신
    **새로운 파괴적 프리미티브**를 만든다. 오늘 `ESC c`는 무해한 no-op인데, 구현
    후에는 아무 프로그램(혹은 바이너리를 `cat`한 것)이 2바이트로 화면을 날릴 수 있다.
    게다가 `Grid::clear()`는 `self.images.clear()`도 호출하므로 (`grid.rs:401`)
    **인라인 이미지 배치가 전부 삭제**되고, 이 코드베이스의 `alt_screen`은 버퍼
    교체가 아니라 *히스토리 보존 스위치*라 (`grid.rs:328-338`) vim 도중 강제 해제하면
    vim 프레임이 주 그리드에 남은 채 스크롤백 오염이 시작된다. 최소 RIS는 D3의
    복구 목적을 100% 달성하면서 이 파급을 전부 피한다.
  - **RIS는 방어적 조치다 — 근거를 정확히 적는다.** 상태가 남는다는 것은 실측으로
    확인됐다: zsh는 DECCKM을 만지지 않으므로 (Research 실측 증거) TUI가 남긴 `?1h`는
    프롬프트로 돌아와도 그대로다. 리뷰어의 "zsh가 매 프롬프트마다 `smkx`/`rmkx`를
    보내므로 도달 불가"라는 주장은 3회 캡처에서 `ESC[?1h` **0회**로 반증됐다.
    **다만 "먹통"은 과장이다** — zsh는 `^[[A`와 `^[OA`를 **둘 다** 바인드하므로
    (D11 실측) DECCKM이 켜진 채 남아도 **zsh 프롬프트에서는 아무 증상이 없다**.
    실제로 깨지는 대상은 *CSI 형식만 하드코딩한 프로그램*(직접 만든 TUI, 일부
    파이프라인 도구)이고, 그건 이 스펙이 통제하지 않는 서드파티다. 따라서 D9는
    "사용자가 `reset`을 쳤을 때 다른 터미널처럼 최소한의 일을 한다"는 방어적
    가치로 정당화되며, 그래서 **최소 형태여야 한다** — 불확실한 이득에 큰 파급을
    지불하지 않는다. (이 한계는 Known Gaps에도 적는다.)
  - **다른 ESC 시퀀스는 전부 no-op**여야 한다. arm은 `intermediates.is_empty() &&
    byte == b'c'`로 정확히 고정한다 — `esc_dispatch(&mut self, intermediates: &[u8],
    ignore: bool, byte: u8)` 시그니처상 intermediate를 가진 `c`-종결 시퀀스가
    DECCKM을 리셋해서는 안 된다. 특히 `ESC=` / `ESC>` (DECKPAM/DECKPNM)는 vim 등이
    terminfo `smkx`/`rmkx`를 통해 실제로 보내므로 새 `esc_dispatch`에 도달한다 —
    DECNKM은 선언된 non-goal이므로 명시적으로 아무 일도 하지 않아야 하고, 이는
    테스트로 고정한다.
  - 기각: (a) `ESC[!p`(DECSTR)만 추가 — `csi_dispatch`만으로 끝나 싸지만 사용자가
    실제로 치는 `reset`/`tput reset`은 RIS를 보내므로 복구가 안 된다. (b) 리셋 경로
    제외 — 유일한 복구 수단이 "탭을 닫고 새로 열기"가 되어 D3의 "지속" 선택이
    사용자에게 함정이 된다. (c) 전체 RIS — 위 참조.

### D10: DECCKM 플래그는 `Grid`에 둔다 — `alt_screen`과 동일한 형태
- **Status**: resolved
- **Rationale**: `Grid`에 `app_cursor_keys: bool` + `set_app_cursor_keys` /
  `app_cursor_keys` 접근자, `Terminal`이 위임. 배선 0줄이고 (`Performer`는
  `&mut Grid` 하나만 들고 `flush()`가 매 청크 새로 만든다 — `parser.rs:128-139`),
  기존 alt-screen 경로와 형태가 100% 같아 테스트도 `scrollback.rs:129-148`의
  모양을 그대로 따른다. 기각: (a) `Processor`에 `Modes` 구조체 신설 — 의미론적으로는
  더 맞지만 `flush`/`Performer` 시그니처를 바꿔야 하고 모드 상태가 `Grid`(alt_screen)와
  `Processor`(DECCKM) 두 곳으로 갈라져 오히려 나빠진다. (b) `alt_screen`까지 함께
  `Processor`로 이사 — 올바른 최종 형태지만 버그 수정 범위를 넘어 scrollback/
  alt-screen 테스트까지 흔든다. 모드가 3개 이상으로 늘면 그때 별도 리팩터링.

### D11: 네비게이션/F 키 바이트 테이블 — **zsh가 실제로 바인드한 것**을 기준으로 한다
- **Status**: resolved *(실측 증거로 `ESC[H`/`ESC[F`에서 개정됨)*
- **Rationale**: 판단 기준은 terminfo 값 자체가 아니라 **1차 게이트(D12: zsh)에서
  실제로 동작하는 바이트**다. `zsh -ic bindkey` 실측 결과 zsh는 `^[[1~`/`^[[4~`를
  Home/End로 바인드하고 `^[[H`/`^[[F`/`^[OH`/`^[OF`는 **어디에도 바인드하지 않는다**.
  - Home/End = `ESC[1~` / `ESC[4~` — zsh에서 즉시 동작하고, PageUp/PageDown/Insert/
    Delete와 같은 tilde 계열이라 형식도 일관된다. vim/less도 이 형식을 받아들인다.
  - PageUp/PageDown = `ESC[5~` / `ESC[6~`
  - Insert/Delete = `ESC[2~` / `ESC[3~` (`^[[3~`은 zsh에서 `delete-char`로 바인드됨을
    실측 확인. macOS의 forward-delete는 Fn+Delete → `NamedKey::Delete`; 라벨이
    "delete"인 키는 Backspace라 기존 `0x7f` arm이 담당한다)
  - 화살표 = `ESC[A`–`ESC[D`, application mode에서 `ESC O A`–`ESC O D`
    (zsh는 **양쪽 모두** 바인드하므로 어느 쪽이든 프롬프트에서 동작한다 — 실측 확인)
  - F1–F4 = `ESC O P` / `Q` / `R` / `S` (SS3), F5–F12 = `ESC[15~` `17~` `18~` `19~`
    `20~` `21~` `23~` `24~` (16과 22는 표준적으로 비어 있다)
  - tilde 계열(`ESC[n~`)에는 application-mode 변형이 **없다** — DECCKM은 커서 키에만
    적용된다. Home/End도 tilde 형식을 쓰므로 DECCKM과 무관하게 항상 같은 바이트다.
  **기준의 한계를 명시한다**: "zsh가 바인드한 것"이라는 기준이 실제로 결정하는 것은
  화살표·Home·End·Delete뿐이다. `^[[5~`·`^[[6~`·`^[[2~`(PageUp/PageDown/Insert)와
  F키는 zsh 기본 키맵에 바인딩이 **없어** tilde 계열 일관성으로 고른 것이고,
  수정자 형식(`ESC[5;5~` 등)도 마찬가지다. 따라서 **D12의 zsh 게이트(a)는 이 행들을
  검증하지 못한다** — 이들의 통과 근거는 `cat -v` 바이트 확인과 유닛테스트뿐이며,
  그래서 D12(b)가 이 행들을 이름으로 열거하도록 확장했다. 게이트를 실제보다 강해
  보이게 적지 않는다.
  기각: (a) `ESC[H`/`ESC[F` (xterm이 실제로 보내는 형식) — 대부분의 TUI는 알아듣지만
  **zsh 프롬프트에서 무반응**이라 1차 게이트를 통과하지 못한다. (b) `ESC O H`/`ESC O F`
  (terminfo `khome`/`kend`) — terminfo 항목은 `smkx` 적용을 전제하는데 zsh는 `smkx`를
  보내지 않고, 이 시퀀스도 바인드하지 않는다. (c) `ESC[7~`/`ESC[8~` (rxvt) — 무관한
  방언.

### D13: 수정자 + 네비게이션/F 키는 xterm 확장 형식으로 완성한다
- **Status**: resolved
- **Rationale**: D4의 공식이 화살표 전용이면 Ctrl+Home·Shift+PageUp·Ctrl+F5 같은
  조합이 테이블에 없어 `_` 폴백 → `None` → **조용히 드롭**된다. 이는 이 스펙이
  없애려는 바로 그 버그를 부분집합에 대해 재도입하는 것이다. 수정자 값은 D4와 같은
  공식(`m = 1 + Shift·1 + Alt·2 + Ctrl·4`)을 쓰되, 형식은 계열마다 다르다:
  - **커서/SS3 계열** (화살표, F1–F4 — 이 스펙에서 SS3 변형을 갖는 키 전부):
    `CSI 1;{m}{final}` — 예 `Ctrl+→` = `ESC[1;5C`, `Ctrl+F1` = `ESC[1;5P`.
    **Home/End는 여기 속하지 않는다** — D11에서 tilde 계열로 확정했으므로 아래
    두 번째 규칙만 적용된다 (`Ctrl+Home` = `ESC[1;5~`, `ESC[1;5H`가 아니다).
  - **tilde 계열** (`ESC[n~`: Home 1, Insert 2, Delete 3, End 4, PgUp 5, PgDn 6,
    F5–F12 15–24): 수정자는 **두 번째 파라미터**에 들어간다 → `CSI {n};{m}~` —
    예 `Ctrl+PageUp` = `ESC[5;5~`, `Shift+Delete` = `ESC[3;2~`
  - 수정자가 없으면(`m == 1`) 파라미터를 생략한 기본 형식을 보낸다 (`ESC[5~`) —
    `ESC[5;1~`이나 `ESC[1;1C`를 보내면 안 된다.
  - **Shift+Tab = `ESC[Z`** (CBT). Tab은 아래 "레거시 5개 arm"에 속하지만 이 한
    조합만은 예외로 인코딩한다 — 역방향 탭 완성/TUI 포커스 이동에 실제로 쓰이고,
    빠뜨리면 D13이 없애려는 "조용한 드롭"이 그대로 남는다.
    **단 Cmd+Shift+Tab은 제외** — D15/R6.2가 이를 기존 `\t`로 못박았고, 이는
    macOS 앱 전환 코드지 터미널 back-tab이 아니다. (구현 중 발견된 D13↔D15
    모순을 D15 쪽으로 해소한 것이다: CBT arm에 `&& !mods.super_key()` 가드.)
  - **레거시 5개 arm(Enter/Backspace/Tab/Escape/Space)은 Shift+Tab을 빼면 수정자를
    무시하고 기본 바이트를 보낸다** — 기존 동작 그대로다. 이 스펙은 그것을 바꾸지
    않으며, 여기 명시하는 이유는 "수정자 조합을 지원한다"가 이들까지 포함한다는
    오독을 막기 위해서다.
  - **`Key::Character` + Alt(Option)은 건드리지 않는다** — macOS에서 Option+문자는
    합성 문자(`∫`, `å`)를 만들고 winit이 그걸 `text`로 준다. 여기에 ESC 접두를
    붙이면(meta 관례) 그 입력 방식이 깨진다. D6의 폴백 그대로 둔다.
  기각: (a) 수정자를 무시하고 베이스 시퀀스 전송 — TUI가 Ctrl+Home과 Home을 구별
  못 한다. (b) 화살표만 수정자 지원하고 나머지는 Known Gap — 드롭이 그대로 남아
  같은 증상이 재발한다.

### D14: `encode_key`는 `(Key, Option<&str>, ModifiersState, bool)`을 받는다
- **Status**: resolved
- **Rationale**: 인코딩에 필요한 입력이 셋(논리 키, 수정자, DECCKM)이고 호출부는
  하나뿐이다 (`terminal_app.rs:286`). 시그니처는
  `encode_key(logical: &Key, text: Option<&str>, mods: ModifiersState, app_cursor: bool)
  -> Option<Vec<u8>>`. `KeyInput`이 이미 `key`/`text`/`mods`를 들고 있고
  (`seam.rs:38-42`), `app_cursor`는 `self.tabs.active().terminal.app_cursor_keys()`로
  얻는다. `bool`을 쓰는 이유는 `Terminal`이 노출하는 것이 `bool`이기 때문(D2/D10) —
  타입을 하나 더 만들면 크레이트 경계를 넘는 변환만 늘어난다. 이 시그니처를 스펙에
  못박는 이유는 `Done when`이 요구하는 `input.rs` 유닛테스트를 이것 없이는 쓸 수
  없기 때문이다. 기존 테스트 함수 **하나**(`encode_named_and_text`,
  `input.rs:109-120` — `encode_key` 호출 3곳)의 호출부도 함께 갱신된다.

### D15: Cmd+화살표를 삼키는 일은 `encode_key` 안에서 한다
- **Status**: resolved
- **Rationale**: `tab_action`은 `Key::Character`만 검사하므로 (`input.rs:30-33`)
  `Key::Named`인 화살표는 탭 코드 단계를 그냥 통과한다. 따라서 D4의 "Cmd는 삼킴"은
  `encode_key` 안에서 구현되어야 한다.
  **범위는 좁게**: Cmd(super)가 눌린 상태에서 `None`을 반환하는 것은 다음
  `Key::Named` 키에 **한한다 — 이 목록이 전부다**: 화살표 4종, Home, End, PageUp,
  PageDown, Insert, Delete, F1–F12. 레거시 5개 arm(Enter/Backspace/**Tab**/Escape/
  Space)은 이 스펙이 Shift+Tab을 인코딩하더라도 **삼킴 대상이 아니다** — 따라서
  Cmd+Shift+Tab은 `\t`를 보낸다(기존 동작 유지).
  `Key::Character`도 건드리지 않는다 — 지금 Cmd+A는 폴백을 타고 셸에 `a`를 보내는데,
  그 동작이 옳은지는 이 스펙의 질문이 아니고 여기서 함께 바꾸면 탭 코드가 아닌 모든
  Cmd 조합의 동작이 조용히 달라진다. Cmd+V도 `Key::Character`라 삼킴 범위 밖이며
  영향이 없다 — `ClipboardHandler`는 클립보드에 **이미지가 있을 때만** 소비하고
  그 외에는 `Passthrough`다 (`clipboard.rs:43-51`).
  `tab_action`에 Named arm을 추가하는 대안은 기각 — 탭 코드가 아닌 것을 `TabAction`
  으로 표현해야 하고, 디스패치 순서(불변 제약)의 의미를 흐린다.

### D12: 수락 게이트는 zsh 1차, `claude`는 2차 관찰
- **Status**: resolved
- **Rationale**: 통과 기준은 (a) zsh 라인 편집에서 ←/→ 커서 이동 + ↑/↓ 히스토리 +
  Home/End 줄 이동 + Delete 문자 삭제 (zsh가 실제로 바인드한 것들 — D11),
  (b) `cat -v`로 실제 전송 바이트를 눈으로 확인 — **화살표에 그치지 않고 D11/D13
  테이블 중 zsh가 검증해 주지 못하는 행을 전부 포함한다**: `^[[A`(normal) /
  `^[OA`(application mode 진입 후), `^[[5~` `^[[6~`(PageUp/PageDown),
  `^[[2~`(Insert), `^[OP`(F1), `^[[15~`(F5), 그리고 수정자 형식 최소 2개
  (`^[[1;5C` = Ctrl+→, `^[[5;5~` = Ctrl+PageUp), `^[[Z`(Shift+Tab) — 객관 증거. `claude` 신뢰 프롬프트는 2차 관찰이다: claude가
  Kitty keyboard protocol(`CSI > 1 u`) 같은 확장 입력을 협상한다면 이 스펙을 완벽히
  구현해도 그 화면에서는 안 움직일 수 있고, 그건 이 스펙의 실패가 아니라 별도
  스펙의 신호다. 기각: claude 동작을 필수 게이트로 삼기 — 현재 non-goal인 Kitty
  프로토콜까지 범위가 무한정 번지고, 통제 불가능한 서드파티 동작에 게이트를 건다.

## Constraints
- `term-core`는 wgpu/winit에 직접·간접으로 의존하지 않는다 (`dependency-direction`).
  DECCKM 추적은 순수 `bool` + 파서 arm이며 headless 테스트로 검증 가능해야 한다.
- 새 크레이트 의존성을 추가하지 않는다 (`no-gui-crates`, 그리고 실제로 불필요).
- macOS ARM64 전용 — `#[cfg(target_os = "...")]` 분기를 추가하지 않는다 (`macos-only`).
- `on_key`의 디스패치 순서(오버레이 → 등록 핸들러 → 탭 코드 → `encode_key`)를
  바꾸지 않는다. 이 스펙은 4단계만 건드린다 (`terminal_app.rs:254-292`).
- `encode_key`의 `_ => text.map(...)` 폴백 arm을 반드시 보존한다 — 글자/기호/IME
  (한글 조합) 출력이 그 경로로 흐른다. 새 arm은 앞에 얹기만 한다.
- `GridSnapshot`을 확장하지 않는다 — 렌더러가 읽지 않는 값이다 (D2).
- **RIS는 `Grid::clear()`를 호출하지 않는다** (D9). `Grid::clear()`는 셀뿐 아니라
  `self.images.clear()`도 수행하므로 (`grid.rs:401`) 호출하면 인라인 이미지 배치가
  전부 사라진다.
- **`app_cursor_keys`는 `Grid::resize` / `Grid::clear` / alt-screen 전환을 넘어
  살아남아야 한다.** 현재 세 함수 모두 이 필드를 건드리지 않지만(코드 확인),
  스펙이 이를 명시하지 않으면 미래의 "resize 시 Grid 재생성" 리팩터링이 이 수정을
  조용히 되돌린다 — 테스트로 고정한다.
- **새 `esc_dispatch`는 `'c'`(RIS) 외 모든 시퀀스에 대해 no-op여야 한다.** 특히
  `ESC=` / `ESC>`(DECKPAM/DECKPNM)는 vim 등이 실제로 보내며 DECNKM은 non-goal이다.
- 수정자가 없을 때는 파라미터 없는 기본 시퀀스를 보낸다 — `ESC[1;1C`나 `ESC[5;1~`
  같은 형태를 만들지 않는다 (D13).
- `cargo clippy --workspace --all-targets -- -D warnings` 클린, 테스트 밖에서
  `unwrap()` 금지 (CLAUDE.md).

## Known Gaps
- **Kitty keyboard protocol 미지원.** `claude`(또는 다른 TUI)가 이 확장 입력을
  쓴다면 이 스펙만으로는 그 화면의 방향키가 안 움직인다. D12에 따라 2차 관찰
  대상이며, 확인되면 별도 스펙으로 re-scope한다.
- **`InputRouter::dispatch`에는 `visible()` 가드가 없다** (`seam.rs:127-134`) —
  `OverlayHost`와 달리 등록된 핸들러는 항상 모든 키를 본다. 현재 유일한 핸들러인
  `ClipboardHandler`는 Cmd+V만 소비하므로 무해하지만, 앞으로 화살표를 삼키는
  핸들러가 등록되면 이 수정이 조용히 무력화된다. 이번 범위 밖.
- **DECNKM(application keypad mode, `ESC=`/`ESC>`) 미구현** — numpad 인코딩은
  다루지 않는다. `esc_dispatch`가 생기므로 나중에 arm 하나로 붙일 수 있다.
- **`esc_dispatch`는 RIS 외 모든 ESC 시퀀스를 계속 무시한다** — charset 선택
  (`ESC(B`), DECSC/DECRC(`ESC 7`/`ESC 8`) 등은 여전히 no-op다.
- **RIS는 최소 형태다** (D9) — `reset`을 쳐도 화면은 지워지지 않고 색/속성(pen)과
  alt-screen 상태도 그대로다. 다른 터미널의 `reset` 체감과 다르다. 전체 RIS는
  이미지 삭제와 alt-screen 의미론 문제를 먼저 풀어야 하므로 별도 스펙.
- **DECCKM 복구는 방어적 가치에 그친다** (D9) — zsh가 `^[[A`와 `^[OA`를 둘 다
  바인드하므로 DECCKM이 켜진 채 남아도 zsh 프롬프트에서는 증상이 없다. 이 복구
  경로가 실제로 값을 하는 대상은 CSI 형식만 하드코딩한 서드파티 프로그램이며,
  그 필요는 관측되지 않았다.
- **Shift+Tab 외의 레거시 arm 수정자 조합은 인코딩하지 않는다** (D13) —
  Ctrl+Enter, Alt+Backspace 등은 기본 바이트 그대로 나간다. `Key::Character` +
  Alt(Option)도 macOS 합성 문자 입력을 지키기 위해 의도적으로 제외했다.
- **bracketed paste / 마우스 리포팅 / focus 리포팅 미구현** — 별개의 입력 모드들.
  (zsh는 이미 `ESC[?2004h/l`을 보내고 있고, 현재 파서는 프라이빗 모드 가드에서
  조용히 무시한다 — 무해하지만 붙여넣기가 TUI에 그대로 흘러든다.)
- **스크롤백에 사용자 입력 경로가 없다.** `crates/app/src`에 휠/스크롤 처리가 전혀
  없고(실측), 이번 스펙이 PageUp/PageDown을 셸로 보내면서 관례적인 스크롤백 단축키
  자리를 쓴다. 뷰포트 스크롤 바인딩(예: Shift+PageUp을 앱이 가로채기)은 이번 범위
  밖이며, 그 자리를 예약해 두지도 않는다 — 나중에 스크롤 입력을 붙일 때 Shift+PageUp을
  회수하려면 이 결정을 다시 열어야 한다.
- **`InputRouter::dispatch`에 `visible()` 가드 없음** (위 참조) — 구조적 취약점으로
  남는다.

## Requirements

### R0: 커서/네비게이션 키가 셸에 도달한다 (goal)

#### R0.1: 방향키가 zsh 커서를 움직인다
- **Given**: 터미널이 떠 있고 zsh 프롬프트에 `echo hello`가 입력돼 있다
- **When**: ← 를 3회 누른다
- **Then**: 커서가 3칸 왼쪽으로 이동한다 (현재는 아무 일도 일어나지 않는다)

#### R0.2: 방향키가 히스토리를 불러온다
- **Given**: zsh 프롬프트가 비어 있고 직전에 `ls` 를 실행했다
- **When**: ↑ 를 누른다
- **Then**: 프롬프트에 `ls` 가 채워진다

#### R0.3: `encode_key`가 방향키에 대해 더 이상 `None`을 반환하지 않는다
- **Given**: `Key::Named(NamedKey::ArrowUp)`, `text = None`, 수정자 없음, `app_cursor = false`
- **When**: `encode_key`를 호출한다
- **Then**: `Some(b"\x1b[A".to_vec())` 를 반환한다 (현재는 `None`)

---

### R1: `term-core`가 DECCKM을 추적하고 조회 경로를 노출한다 *(D2, D3, D7, D10)*

#### R1.1: `ESC[?1h`가 application cursor mode를 켠다
- **Given**: `Terminal::new(20, 5, fg, bg)` 로 만든 새 터미널
- **When**: `t.advance(b"\x1b[?1h")`
- **Then**: `t.app_cursor_keys() == true`

#### R1.2: `ESC[?1l`이 normal cursor mode로 되돌린다
- **Given**: `ESC[?1h` 로 application mode에 들어간 터미널
- **When**: `t.advance(b"\x1b[?1l")`
- **Then**: `t.app_cursor_keys() == false`

#### R1.3: 기본값은 normal이다
- **Given**: 방금 만든 터미널, 아무 바이트도 넣지 않음
- **When**: `t.app_cursor_keys()` 를 읽는다
- **Then**: `false`

#### R1.4: 다른 프라이빗 모드는 DECCKM을 건드리지 않는다
- **Given**: `ESC[?1h` 로 application mode에 들어간 터미널
- **When**: `t.advance(b"\x1b[?1049h\x1b[?2004h\x1b[?1049l\x1b[?2004l")`
  (alt-screen 및 zsh가 실제로 보내는 bracketed paste)
- **Then**: `t.app_cursor_keys()` 는 여전히 `true`

#### R1.5: 플래그가 resize / clear / alt-screen 전환을 넘어 살아남는다
- **Given**: `ESC[?1h` 로 application mode에 들어간 터미널
- **When**: `t.resize(40, 10)` 후 `t.advance(b"\x1b[2J\x1b[?1049h\x1b[?1049l")`
- **Then**: `t.app_cursor_keys()` 는 매 단계 이후 `true` — 미래의 "resize 시 Grid
  재생성" 리팩터링이 이 수정을 되돌리면 이 테스트가 잡는다

#### R1.6: `GridSnapshot`은 확장되지 않는다
- **Given**: 이 스펙의 변경이 전부 적용된 트리
- **When**: 기존 `crates/term-core/tests/grid_snapshot.rs` 의 insta 스냅샷을 돌린다
- **Then**: 스냅샷 변경 0건이고 `GridSnapshot`에 DECCKM 관련 필드가 없다 (조회는
  `Terminal::app_cursor_keys()` 로만 한다)

#### R1.7: DECCKM은 터미널 인스턴스마다 독립이다
- **Given**: 두 개의 `Terminal` `a`, `b`
- **When**: `a.advance(b"\x1b[?1h")` 만 실행한다
- **Then**: `a.app_cursor_keys() == true` 이고 `b.app_cursor_keys() == false`

---

### R2: `esc_dispatch`가 생기고 RIS는 DECCKM만 리셋한다 *(D9)*

#### R2.1: `ESC c` 가 DECCKM을 normal로 되돌린다
- **Given**: `ESC[?1h` 로 application mode에 들어간 터미널
- **When**: `t.advance(b"\x1bc")`
- **Then**: `t.app_cursor_keys() == false`

#### R2.2: `ESC c` 는 화면·이미지·alt-screen·pen을 건드리지 않는다
- **Given**: 텍스트가 찍혀 있고 인라인 이미지가 배치됐으며 `ESC[?1049h` 로
  alt-screen에 들어가 있고 SGR로 색이 설정된 터미널
- **When**: `t.advance(b"\x1bc")`
- **Then**: 스냅샷 텍스트가 그대로이고, 이미지 배치 개수가 그대로이며,
  `t.alt_screen() == true` 가 유지되고, 이후 출력의 색도 유지된다 (DECCKM만 바뀐다)

#### R2.3: `ESC=` / `ESC>` 는 아무 일도 하지 않는다
- **Given**: `ESC[?1h` 로 application mode에 들어간 터미널 (vim이 terminfo `smkx`
  로 실제로 보내는 시퀀스다)
- **When**: `t.advance(b"\x1b=\x1b>")`
- **Then**: `t.app_cursor_keys()` 는 `true` 로 유지되고 그리드도 변하지 않는다

#### R2.4: intermediate를 가진 `c` 종결 시퀀스는 RIS가 아니다
- **Given**: `ESC[?1h` 로 application mode에 들어간 터미널
- **When**: `t.advance(b"\x1b(c")` (intermediate `(` + 최종 바이트 `c`)
- **Then**: `t.app_cursor_keys()` 는 여전히 `true` — arm은
  `intermediates.is_empty() && byte == b'c'` 로만 발화한다

#### R2.5: APC 스캐너가 `ESC c` 를 vte에 넘긴다
- **Given**: application mode에 들어간 터미널
- **When**: `t.advance(b"\x1b")` 와 `t.advance(b"c")` 를 **두 번의 호출로 쪼개서**
  넣는다 (`Processor`의 APC 상태기계가 ESC를 가로채 보류한다)
- **Then**: `t.app_cursor_keys() == false` — 청크 경계를 넘어도 RIS가 도달한다

---

### R3: 화살표가 모드에 맞는 형식으로 인코딩된다 *(D1, D5)*

#### R3.1: normal mode에서 화살표는 CSI 형식이다
- **Given**: `app_cursor = false`, 수정자 없음
- **When**: ArrowUp/Down/Right/Left 에 대해 `encode_key` 를 호출한다
- **Then**: 각각 `ESC[A`, `ESC[B`, `ESC[C`, `ESC[D`

#### R3.2: application mode에서 화살표는 SS3 형식이다
- **Given**: `app_cursor = true`, 수정자 없음
- **When**: ArrowUp/Down/Right/Left 에 대해 `encode_key` 를 호출한다
- **Then**: 각각 `ESC O A`, `ESC O B`, `ESC O C`, `ESC O D`

---

### R4: 네비게이션 키와 F 키가 인코딩된다 *(D11)*

#### R4.1: Home/End는 tilde 형식이다
- **Given**: 수정자 없음
- **When**: Home, End 에 대해 `encode_key` 를 호출한다
- **Then**: `ESC[1~`, `ESC[4~` (zsh가 `beginning-of-line`/`end-of-line` 로 바인드한
  유일한 형식 — 실측)

#### R4.2: PageUp/PageDown
- **Given**: 수정자 없음
- **When**: PageUp, PageDown 에 대해 `encode_key` 를 호출한다
- **Then**: `ESC[5~`, `ESC[6~`

#### R4.3: Insert/Delete
- **Given**: 수정자 없음
- **When**: Insert, Delete 에 대해 `encode_key` 를 호출한다
- **Then**: `ESC[2~`, `ESC[3~` (macOS의 Fn+Delete가 `NamedKey::Delete` 로 온다;
  라벨이 "delete"인 키는 Backspace라 기존 `0x7f` arm이 계속 담당한다)

#### R4.4: F1–F4는 SS3 형식이다
- **Given**: 수정자 없음
- **When**: F1..F4 에 대해 `encode_key` 를 호출한다
- **Then**: `ESC O P`, `ESC O Q`, `ESC O R`, `ESC O S`

#### R4.5: F5–F12는 tilde 형식이고 번호에 구멍이 있다
- **Given**: 수정자 없음
- **When**: F5..F12 에 대해 `encode_key` 를 호출한다
- **Then**: `ESC[15~` `17~` `18~` `19~` `20~` `21~` `23~` `24~` — 16과 22는
  건너뛴다 (terminfo `kf5..kf12` 와 일치)

#### R4.6: tilde 계열은 DECCKM의 영향을 받지 않는다
- **Given**: `app_cursor = true`
- **When**: Home, PageUp, Delete, F5 에 대해 `encode_key` 를 호출한다
- **Then**: R4.1/R4.2/R4.3/R4.5와 **동일한** 바이트 — DECCKM은 커서 키에만 적용된다

---

### R5: 수정자 조합이 xterm 확장 형식으로 인코딩된다 *(D4, D13)*

#### R5.1: Ctrl+화살표
- **Given**: `mods = CONTROL`, `app_cursor = false`
- **When**: ArrowRight 에 대해 `encode_key` 를 호출한다
- **Then**: `ESC[1;5C`

#### R5.2: 수정자 값이 `m = 1 + Shift·1 + Alt·2 + Ctrl·4` 로 계산된다
- **Given**: ArrowLeft
- **When**: `SHIFT` / `ALT` / `CONTROL` / `CONTROL|SHIFT` 로 각각 호출한다
- **Then**: `ESC[1;2D` / `ESC[1;3D` / `ESC[1;5D` / `ESC[1;6D`

#### R5.3: tilde 계열은 수정자를 **두 번째** 파라미터에 싣는다
- **Given**: `mods = CONTROL`
- **When**: PageUp, Home, Delete 에 대해 호출한다
- **Then**: `ESC[5;5~`, `ESC[1;5~`, `ESC[3;5~` — `ESC[1;5H` 같은 CSI-final 형식이
  **아니다**

#### R5.4: 수정자가 붙으면 application mode여도 CSI 형식이다
- **Given**: `app_cursor = true`, `mods = CONTROL`
- **When**: ArrowUp 에 대해 호출한다
- **Then**: `ESC[1;5A` — `ESC O A` 도 아니고 `ESC O 1;5 A` 도 아니다

#### R5.5: 수정자가 없으면 파라미터를 생략한다
- **Given**: `mods = ModifiersState::empty()`
- **When**: ArrowRight, PageUp 에 대해 호출한다
- **Then**: `ESC[C`, `ESC[5~` — `ESC[1;1C` 나 `ESC[5;1~` 를 만들지 않는다

#### R5.6: Shift+Tab은 `ESC[Z` 다 (Cmd가 함께 눌리지 않은 경우)
- **Given**: `mods = SHIFT`
- **When**: `Key::Named(NamedKey::Tab)` 에 대해 호출한다
- **Then**: `ESC[Z` (CBT). 수정자 없는 Tab은 기존대로 `\t` 이고,
  `mods = SUPER|SHIFT` 도 `\t` 다 (R6.2와 일치 — D13 주석 참조)

---

### R6: 기존 입력 경로가 보존되고 Cmd는 삼켜진다 *(D6, D15)*

#### R6.1: Cmd+화살표는 셸에 아무것도 보내지 않는다
- **Given**: `mods = SUPER`
- **When**: ArrowLeft 에 대해 `encode_key` 를 호출한다
- **Then**: `None` — PTY 쓰기가 일어나지 않는다

#### R6.2: Cmd+문자는 기존 동작 그대로다
- **Given**: `mods = SUPER`, `Key::Character("a")`, `text = Some("a")`
- **When**: `encode_key` 를 호출한다
- **Then**: `Some(b"a".to_vec())` — 삼킴 범위는 D15가 열거한 `Key::Named` 키뿐이고
  Cmd+Shift+Tab도 삼켜지지 않는다 (`\t`)

#### R6.3: 문자/IME 폴백이 살아 있다
- **Given**: 한글 조합으로 winit이 `text = Some("한")` 을 준 이벤트
- **When**: `encode_key` 를 호출한다
- **Then**: `Some("한".as_bytes().to_vec())` — `_ => text.map(...)` arm이 그대로다

#### R6.4: 레거시 5개 arm은 (Shift+Tab 외) 수정자를 무시한다
- **Given**: `mods = CONTROL`
- **When**: Enter, Backspace, Escape, Space 에 대해 호출한다
- **Then**: `\r`, `0x7f`, `0x1b`, `' '` — 기존 바이트 그대로

---

### R7: `app`이 활성 탭의 DECCKM을 읽어 인코딩에 넘긴다 *(D14, D7 — 크레이트 경계)*

#### R7.1: `encode_key`가 수정자와 모드를 받는다
- **Given**: `crates/app/src/input.rs`
- **When**: 시그니처를 확인한다
- **Then**: `encode_key(logical: &Key, text: Option<&str>, mods: ModifiersState,
  app_cursor: bool) -> Option<Vec<u8>>` 이고, 기존 테스트 함수
  `encode_named_and_text`(`input.rs:109-120`)의 호출 3곳이 함께 갱신돼 있다

#### R7.2: 호출부가 활성 탭의 DECCKM을 전달한다
- **Given**: 탭 2개, 1번 탭에서만 TUI가 `ESC[?1h` 를 보냈다
- **When**: 1번 탭에서 ↑ 를 누르고, 2번 탭으로 전환해 ↑ 를 누른다
- **Then**: 1번 탭의 PTY는 `ESC O A` 를, 2번 탭의 PTY는 `ESC[A` 를 받는다 —
  `terminal_app.rs:286` 이 `self.tabs.active().terminal.app_cursor_keys()` 를 읽는다

#### R7.3: 디스패치 순서가 바뀌지 않는다
- **Given**: `on_key` (`terminal_app.rs:254-292`)
- **When**: 변경 후의 순서를 확인한다
- **Then**: 오버레이 → 등록 핸들러 → 탭 코드 → `encode_key` 그대로이고, 4단계만
  수정됐다

---

### R8: 검증 게이트 *(D12, D8, Constraints)*

#### R8.1: zsh 1차 게이트
- **Given**: `/run-term` 으로 띄운 터미널의 zsh 프롬프트
- **When**: ←/→, ↑/↓, Home/End, Delete 를 누른다
- **Then**: 각각 커서 이동, 히스토리 이동, 줄 처음/끝 이동, 앞 문자 삭제가 일어난다
  (zsh가 바인드한 것으로 실측 확인된 키들)

#### R8.2: 바이트 게이트 — zsh가 검증해 주지 못하는 행
- **Given**: `cat -v` 를 실행한 상태
- **When**: PageUp, PageDown, Insert, F1, F5, Ctrl+→, Ctrl+PageUp, Shift+Tab 을
  누르고, 이어서 application mode 진입 후 ↑ 를 누른다
- **Then**: 화면에 `^[[5~` `^[[6~` `^[[2~` `^[OP` `^[[15~` `^[[1;5C` `^[[5;5~`
  `^[[Z` 가 순서대로 보이고, application mode에서는 `^[OA` 가 보인다

#### R8.3: 빌드/테스트/린트가 그린이다
- **Given**: 변경이 전부 적용된 트리
- **When**: `cargo build`, `cargo test --workspace`,
  `cargo clippy --workspace --all-targets -- -D warnings`
- **Then**: 셋 다 성공하고 새 의존성이 추가되지 않았으며 `term-core`는 여전히
  wgpu/winit을 참조하지 않는다 (테스트 밖 `unwrap()` 없음)

#### R8.4: `claude`는 2차 관찰이다
- **Given**: `/run-term` 터미널에서 `claude` 를 실행해 신뢰 프롬프트가 떠 있다
- **When**: ↑/↓ 를 누른다
- **Then**: 선택이 움직이면 통과. **움직이지 않아도 이 스펙은 실패가 아니다** —
  R8.1–R8.3이 그린이면 Kitty keyboard protocol 별도 스펙의 신호로 기록한다 (D12)

## Tasks

### T1: `term-core`에 DECCKM 추적을 추가한다 [infra]
- **Fulfills**: R1
- **Depends on**: (none)
- 손대는 파일: `crates/term-core/src/grid.rs` (`app_cursor_keys: bool` 필드 +
  `set_app_cursor_keys` / `app_cursor_keys` 접근자 — `alt_screen`
  (`grid.rs:152, 379-384`) 바로 옆에 같은 모양으로),
  `crates/term-core/src/parser.rs` (`csi_dispatch`의 기존 프라이빗 모드 블록
  `parser.rs:261-266` 안에 `?1` arm 추가 — `1049|1047|47` arm과 형제),
  `crates/term-core/src/lib.rs` (`Terminal::app_cursor_keys()` 위임 —
  `alt_screen()` 옆),
  `crates/term-core/tests/` (새 headless 테스트 파일; `tests/scrollback.rs:129-148`
  의 alt-screen 토글 테스트가 형태 표본)
- 주의: `GridSnapshot`은 건드리지 않는다 (R1.6). R1.5(resize/clear/alt-screen 보존)는
  현재 코드에서 자동으로 성립하지만 **테스트로 못박는 것이 이 태스크의 산출물**이다.

### T2: `esc_dispatch`를 신설하고 최소 RIS를 구현한다 [infra]
- **Fulfills**: R2
- **Depends on**: T1  *(← T3과 병렬 가능)*
- 손대는 파일: `crates/term-core/src/parser.rs` (`impl Perform for Performer`에
  `esc_dispatch` 추가 — 현재 `print`/`execute`/`csi_dispatch` 셋뿐),
  T1이 만든 term-core 테스트 파일
- arm은 `intermediates.is_empty() && byte == b'c'` 로만 발화하고 본문은
  `self.grid.set_app_cursor_keys(false)` **한 줄**이다. `Grid::clear()`를 부르지
  않는다 — 이미지가 지워진다 (`grid.rs:401`, Constraints).
- T1에 의존하는 이유: 같은 `parser.rs`를 편집하고, `set_app_cursor_keys`가 T1의
  산출물이다.

### T3: `encode_key`를 확장하고 활성 탭 DECCKM을 배선한다 [vertical]
- **Fulfills**: R0, R3, R4, R5, R6, R7, R8
- **Depends on**: T1  *(← T2와 병렬 가능)*
- 손대는 파일: `crates/app/src/input.rs` (`encode_key` 시그니처 D14 + 전체 키
  테이블 + 유닛테스트; 기존 `encode_named_and_text`의 호출 3곳 갱신),
  `crates/app/src/terminal_app.rs` (호출부 `:286` 한 줄 —
  `self.tabs.active().terminal.app_cursor_keys()` 를 읽어 넘긴다)
- **이것이 사용자에게 보이는 슬라이스다**: 키 입력 → 바이트 → PTY. 따라서 R8의
  관측 게이트(zsh 동작 + `cat -v` 바이트 + `claude` 2차 관찰)가 이 태스크의 완료
  조건에 포함된다 — 별도의 final-verify 태스크를 두지 않는 이유다.
- 새 arm은 기존 `_ => text.map(...)` 폴백 **앞에** 얹는다. 폴백은 삭제하지 않는다
  (R6.3, IME 회귀 방지).
- T2와 병렬 가능한 근거: 파일 겹침 없음(`app/` vs `term-core/`), 인터페이스 의존
  없음(T3이 소비하는 것은 T1의 접근자이지 T2의 RIS가 아니다).

## External Dependencies

### Pre-work
- (none) — 전부 로컬 코드 변경이고 새 크레이트 의존성이 없다 (D8). 검증에 쓰는
  `zsh`, `cat -v`, `infocmp`는 이미 시스템에 있다.

### Post-work
- R8.4(`claude` 신뢰 프롬프트)가 실패하면 **스펙 실패로 처리하지 말고**, Kitty
  keyboard protocol 지원을 별도 `/specify`로 올린다 (D12, Known Gaps).
- 스냅샷이 움직이면(R1.6 위반 신호) `/insta-review`로 확인한다 — 정상 경로에서는
  변경 0건이어야 한다.
