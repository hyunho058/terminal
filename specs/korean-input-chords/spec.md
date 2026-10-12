# Spec: korean-input-chords

## Meta
- **Created**: 2026-10-09
- **Type**: dev
- **Status**: approved
- **Approved by**: user
- **Approved at**: 2026-10-10

## Goal
Korean-input chord fix. Under the 2-Set Korean input source, AppKit reports `ㅅ`/`ㅊ`/`ㅡ`
for Cmd+T / Cmd+C / Cmd+Shift+M; winit derives the logical key from that string, so the
app's chords don't match. The always-Roman Command-layer string is the right substitute.
Keep the probe source (`imeprobe.swift`) next to the spec.

## Non-goals
- **Ctrl chords**, including the open question that no code path in `crates/app` appears to
  turn Ctrl+C into the 0x03 interrupt byte on *any* input source. That is a separate bug and
  gets a sibling spec if it is real.
- **Option chords.** `Cmd+Opt+<digit>` already matches by physical key (`input::monitor_jump`)
  and is unaffected.
- **Verifying input sources other than 2-Set Korean** (3-Set Korean, Japanese, Chinese,
  Cyrillic, Dvorak…). The fix is expected to help any source whose Cmd-layer is Roman, but
  this spec measures and asserts only 2-Set Korean and ABC.
- Re-mapping or adding chords. The chord table stays exactly as it is.

## Confirmed Goal
Every Cmd chord the app matches behaves identically under the 2-Set Korean input source as
under ABC: Cmd+T, Cmd+W, Cmd+[ / Cmd+], Cmd+1–9 (tabs), Cmd+C (copy), Cmd+V (paste), Cmd+A
(rename field select-all), Cmd+Shift+S (sidebar), Cmd+Shift+M (ai-monitor), Cmd+Shift+R
(rename), Cmd+Shift+G (region capture) — not only the three that were measured.

- **Cause (measured, earlier session):** with Cmd held, winit 0.30.13 takes `logical_key`
  from `NSEvent.charactersIgnoringModifiers`, which under 2-Set Korean is the jamo (`ㅅ`);
  the same event's `NSEvent.characters` — exposed by winit as
  `KeyEventExtModifierSupplement::text_with_all_modifiers` — is the Roman Command-layer
  string (`t`). Substituting the latter is the fix.
- **Both idle and mid-composition** chords are in scope: a Cmd chord pressed while a Hangul
  syllable is still being composed must also work, and its behaviour is to be **measured
  with a real preedit** before it is decided (the earlier probe never reached a composing
  state — typing `h` committed `ㅗ` at once).
- *(inferred)* A Cmd+letter that no chord claims and that falls through to the shell (the
  `encode_key` text fallback) sends the same byte it sends on ABC, never a jamo.
- The measurement method travels with the spec: the probes (`imeprobe.swift`, the idle-only
  original whose output is quoted in the Research table's KO-idle rows; `composeprobe.swift`,
  `digitprobe.swift`, `optprobe.swift`) sit in this directory with their outputs
  (`composeprobe.out`, `composeprobe.deadkey.out`, `digitprobe.run1-3.out`, `optprobe.out`).

**Done when:** in the real app under 2-Set Korean, Cmd+T opens a tab and Cmd+Shift+M toggles
the ai-monitor (idle and mid-composition); ABC behaviour is byte-for-byte unchanged; the pure
helper is unit-tested and `cargo mutants` on it leaves 0 survivors.

## Research

### Measured (macOS 26.6.2, 2026-10-09; `composeprobe.swift` → `composeprobe.out` in this directory)

The probe feeds synthetic key events through `NSTextInputContext.handleEvent` inside a real
`NSApplication`, so the Korean input method actually sees them (a bare `CGEvent` does not —
that is how this bug was falsely cleared once), and replays winit 0.30.13's `keyDown` state
machine (`view.rs:442-498`, `insert_text` 393-416, `do_command_by_selector` 421-437) to report
whether the app would receive a `KeyboardInput`. `imeprobe.swift` is the earlier, idle-only
probe that first established the diagnosis.

| state | key | `characters` | `charactersIgnoringModifiers` | `key_without_modifiers` (UCKeyTranslate) | `KeyboardInput` reaches app |
|---|---|---|---|---|---|
| ABC | Cmd+T | `t` | `t` | `t` | yes |
| ABC | Cmd+Shift+M | `m` | `M` | `m` | yes |
| ABC | Cmd+Shift+] | `]` | `}` | `]` | yes |
| KO idle | Cmd+T | `t` | **`ㅅ`** | **`ㅅ`** | yes |
| KO idle | Cmd+Shift+M | `m` | **`ㅡ`** | **`ㅡ`** | yes |
| KO idle | Cmd+A / Cmd+Shift+G | `a` / `g` | **`ㅁ` / `ㅎ`** | **`ㅁ` / `ㅎ`** | yes |
| KO idle | Cmd+Shift+] / Cmd+1 | `]` / `1` | `}` / `1` | `]` / `1` | yes |
| KO composing `하` | Cmd+Shift+M | `m` | `ㅡ` | `ㅡ` | **no** |
| KO composing `ㄱ`/`ㅎ` | Cmd+C, Cmd+V | `c`/`v` | `ㅊ`/`ㅍ` | — | **no** |
| KO composing `하` (`digitprobe.run1.out`) | Cmd+1, Cmd+Shift+], Cmd+T | `1`/`]`/`t` | `1`/`}`/`ㅅ` | — | **no** |
| ABC (`optprobe.out`) | Cmd+Opt+S / Cmd+Opt+Shift+M / Cmd+Opt+C / Cmd+Opt+1 | `ß`/`Â`/`ç`/`¡` | `s`/`M`/`c`/`1` | — | yes |
| KO idle (`optprobe.out`) | Cmd+Opt+S / Cmd+Opt+Shift+M / Cmd+Opt+C / Cmd+Opt+1 | `s`/`m`/`c`/`1` | **`ㄴ`/`ㅡ`/`ㅊ`**/`1` | — | yes |
| ABC, dead key pending (Opt+e) (`composeprobe.deadkey.out`) | Cmd+T | `´t` | `t` | `t` | yes — no marked text, no commit |

- **Idle:** only *letter* chords break; digits and brackets are ASCII in every column.
  `characters` (winit: `KeyEventExtModifierSupplement::text_with_all_modifiers`,
  `winit-0.30.13/src/platform/modifier_supplement.rs:6-22`) is Roman for every letter.
- **`key_without_modifiers` is NOT a substitute:** under 2-Set Korean
  `TISCopyCurrentKeyboardLayoutInputSource` returns `com.apple.keylayout.2SetHangul`, so
  winit's `get_modifierless_char` (`platform_impl/macos/event.rs:24-77`) yields the jamo too.
- **Case:** `characters` does not heed Shift under Cmd (`m` for Cmd+Shift+M, even on ABC),
  while ABC's `charactersIgnoringModifiers` does (`M`).
- **Mid-composition, the chord never reaches the app at all** — digits and brackets included
  (Cmd+1, Cmd+Shift+], which work idle; measured in `digitprobe.run1.out`, the one run whose
  every step had marked text). The Korean IM answers a Cmd keyDown with marked text by `setMarkedText` →
  `insertText(<syllable>)` → `doCommandBySelector(noop:)`. winit turns the `insertText` into
  `Ime::Preedit("")` + `Ime::Commit`, sets `Committed`, and `do_command_by_selector` returns
  early on `Committed` (view.rs:426-428, deliberately: "we'll end up sending it twice with some
  IMEs like Korean"), so `forward_key_to_app` stays false and `had_ime_input` suppresses the
  `KeyboardInput` (view.rs:479-497). This is a **different defect** from the idle one: no
  remapping of `logical_key` can fix an event that is never emitted.
- **`NSApp.currentEvent` during that commit is the Cmd keyDown itself** (measured: inside
  `insertText` it reported `keyDown kc=46 chars=m cmd=true` for Cmd+Shift+M, and the same for
  Cmd+C and Cmd+V). Combined with the synchronous dispatch below, the app can read the lost
  chord from inside its own `Ime::Commit` handler.
- **Probe caveat:** with synthetic events the first jamo typed right after a Cmd chord is
  sometimes inserted directly instead of composed (`insertText 'ㅎ' hadMarked=false`), so some
  "composing" steps in `composeprobe.out` had no marked text and say nothing about
  composition. The table above uses only the steps whose `marked-before` is non-empty.
  `composeprobe.out` is the long run (its later digit step had no marked text); the digit and
  bracket rows come from the focused `digitprobe.swift`.
- **A second mode, seen in 2 of 3 focused runs** (`digitprobe.run2.out`, `run3.out`): the IM
  makes **no** call at all for the Cmd keyDown — the marked `하` stays, no commit, no
  `doCommandBySelector` — so winit emits nothing and there is no `Ime::Commit` to hang a replay
  on. Both runs are visibly corrupted (`chars=g ignoring=g` while Korean is selected, i.e. the
  source flipped under the IM), so it is most likely an artifact of synthetic events posted
  faster than a person types. Not ruled out — see Known Gaps; checklist steps 4–6 detect it.
- **Opt does not open a hole in D2** (`optprobe.out`): on ABC the logical key under Cmd+Opt
  is ASCII (`s`, `M`, `c`, `1`), so D2(b) fails and nothing is substituted — ABC stays
  byte-for-byte. Under Korean the logical key is the jamo and `characters` is the Roman letter,
  so D2 substitutes and Cmd+Opt+<letter> yields exactly the key ABC yields. Opt chords therefore
  behave as on ABC — which today includes the letter matchers not rejecting alt; that is
  pre-existing ABC behaviour, not something this spec adds.
- **ABC dead keys do not reach the replay path** (`composeprobe.deadkey.out`): Opt+e then
  Cmd+T produced no marked text and no commit, and the `KeyboardInput` was emitted
  (`characters` = `´t`, logical `t`, ASCII → D2 does not substitute).

### Code (winit 0.30.13)
- With Cmd or Ctrl held, `logical_key` = `charactersIgnoringModifiers`
  (`platform_impl/macos/event.rs:121-149`, `get_logical_key_char` 80-90); `text` =
  `logical_key.to_text()` (event.rs:153), so under Korean `text` is the jamo too.
- `queue_event` dispatches **synchronously** when the handler is idle
  (`app_state.rs:306-320`), which it is inside AppKit's `keyDown`. So `Ime::Commit` and
  `KeyboardInput` are handled inside the `keyDown` call, in queue order. `EventLoopProxy`
  events are delivered later via a run-loop source (`event_loop.rs:513-521`).

### Code (this repo)
- The one place a winit `KeyEvent` becomes a `KeyInput`:
  `crates/app/src/terminal_app.rs:2756-2774`. It runs `suppress_key(inner.composing,
  &logical_key)` first (`input.rs:912-914`).
- `KeyInput { key, text, mods, physical }` (`crates/app/src/seam.rs:40-54`) — no field for the
  Command-layer string today.
- Letter-chord matchers, all `Key::Character` + case-fold: `tab_action` (`input.rs:30-53`, T/W
  plus `[ ] { }` and digits), `is_sidebar_toggle_chord` (`input.rs:231-236`, S), copy
  (`copy.rs:23`, C), paste (`clipboard.rs:83`, V), monitor toggle (`overlay.rs:425-430`, M),
  rename toggle (`rename.rs:87-92`, R), rename select-all (`rename.rs:220-222`, A), capture
  (`capture.rs:548-553`, G). `monitor_jump` (`input.rs:631-651`) matches by **physical** key and
  is unaffected.
- `encode_key` (`input.rs:866-897`) sends `text` to the shell for an unclaimed Cmd+letter (the
  "Cmd+A still reaches the shell through the text fallback" comment) — so under Korean today
  Cmd+<unclaimed letter> types a jamo.
- `on_ime(Commit)` (`terminal_app.rs:530-…`) routes the syllable to the tab the composition
  started in (`composing_tab`), and offers it to overlays first (rename field).
- `specs/hangul-ime-and-font-fallback/spec.md:320` claims Cmd+T passes the D8 composing gate
  because it is a *named* key. It is a `Key::Character` — and, per the table above, it never
  reaches that gate mid-composition anyway.
- No `NSEvent` monitor exists in `crates/app`. `objc2 0.6.4`, `objc2-app-kit 0.3.2`,
  `objc2-foundation 0.3.2` are direct deps; `block2 0.6.2` is already in `Cargo.lock`
  transitively.
- Gap list convention for external-tool facts: `docs/architecture.md` G5–G8.
- Build/test/lint: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D
  warnings`, `cargo mutants -f <file> -j 4` (CLAUDE.md).

## Decisions

### D1: The idle substitute is `text_with_all_modifiers` (`NSEvent.characters`)
- **Status**: resolved (measured)
- **Rationale**: It is the only source that is Roman under 2-Set Korean for every letter chord
  (Research table). It is also what AppKit menus match key equivalents against, so it follows
  the user's own Command-layer setting (e.g. "Dvorak – QWERTY ⌘") instead of hard-coding one.
  Rejected: `key_without_modifiers` (measured jamo: the current *layout* source is
  `2SetHangul`); `charactersIgnoringModifiers` (that is the bug); a physical-key → letter
  table (pins a US-QWERTY mapping and breaks every non-QWERTY user's chords silently).

### D2: Substitute only when all three hold
- **Status**: resolved
- **Rationale**: (a) Cmd (`super`) is held; (b) `logical_key` is a `Key::Character` that
  contains a non-ASCII char; (c) `text_with_all_modifiers` is exactly one ASCII-graphic char
  (`is_ascii_graphic`). Otherwise the event passes through untouched. So ABC, digits, brackets,
  plain Hangul typing (no Cmd), named keys and a Cmd+Ctrl chord whose `characters` is a control
  char are all byte-for-byte unchanged **by construction**, not by test luck. Rejected:
  "substitute whenever Cmd is held" (would change ABC's `M` to `m` and `}` to `]`, breaking
  the `{`/`}` arm of `tab_action` for no gain).

### D3: Shift upper-cases a substituted letter
- **Status**: assumed
- **Rationale**: `characters` ignores Shift under Cmd (`m` for Cmd+Shift+M) while ABC's
  `logical_key` heeds it (`M`). Upper-casing an ASCII letter when Shift is held makes the
  Korean `KeyInput` identical to the ABC one, which is what keeps the unclaimed-letter text
  fallback identical too. Every chord matcher case-folds already, so it is neutral for them.
  Rejected: leaving it lowercase (chords would still match, but Cmd+Shift+<unclaimed letter>
  would send `x` to the shell where ABC sends `X`).

### D4: `text` follows the swap
- **Status**: resolved (confirmed in the L0 mirror)
- **Rationale**: winit sets `text = logical_key.to_text()`, so under Korean an unclaimed
  Cmd+letter reaches `encode_key`'s text fallback as a jamo. When D2 substitutes, `text`
  becomes the substituted string, so the shell receives what it receives on ABC.

### D5: One pure helper, applied at the one construction site; `KeyInput` keeps its shape
- **Status**: assumed
- **Rationale**: a pure `input::chord_key(logical, text, text_with_all_modifiers, mods)`
  returning the `(Key, text)` to use (pass-through returns `logical`/`text` untouched), called where `terminal_app.rs` builds `KeyInput` (and by D7's
  replay). Every current and future letter chord benefits with no matcher edited. Rejected:
  a new `KeyInput` field (all eight matchers would have to learn to read it, and the next
  chord added would forget); normalising inside each matcher (eight copies of the rule).
  `suppress_key` still reads winit's raw `logical_key` — it runs only while composing, and
  per Research a Cmd chord never arrives in that state.

### D6: Mid-composition: commit the syllable, then run the chord
- **Status**: resolved (user)
- **Rationale**: native macOS behaviour (TextEdit, Terminal.app). The IM already commits;
  the existing `Ime::Commit` path places the syllable in the tab — or the rename field — the
  composition belongs to (`composing_tab` routing). The chord then runs exactly as if pressed
  idle, so Cmd+T opens a new tab *after* `하` is in the old tab's shell, and Cmd+V pastes
  *after* the syllable. Rejected: dropping the syllable (loses typed text).

### D7: Recover the lost chord from `NSApp.currentEvent` inside `Ime::Commit`
- **Status**: resolved (user; premise measured in the probe, re-confirmed in-app by D9)
- **Rationale**: winit never emits the `KeyboardInput` (Research), but `Ime::Commit` is
  dispatched synchronously inside the very `keyDown` (`app_state.rs:306-320`) and
  `NSApplication::currentEvent` is that keyDown (measured in the probe's `insertText`). So
  once the syllable is placed, if `currentEvent` is a `KeyDown` with the Command flag, build a
  `KeyInput` from it and dispatch it through `on_key`, the path a `KeyboardInput` takes. The
  `KeyInput` is built like this: mods from its `modifierFlags`, physical via winit's public
  `PhysicalKeyExtScancode::from_scancode(keyCode)`, key/text per D11.
  - **Placement — one tail, both exits.** The Commit arm has two exits: the overlay-claimed
    early return (`terminal_app.rs:557-567`, the rename field) and the normal fall-through
    (`:617-620`). The replay runs **after both**. It goes in one tail that both reach, after
    `set_preedit("")` and `composing_tab = None`. So when the replayed `on_key` runs,
    `composing == false` and ownership is already released. Mid-composition Cmd+A /
    Cmd+Shift+R in the rename field therefore replay too.
  - **Exit propagation.** `on_key` returns `false` for "exit" (Cmd+W on the last tab), and
    only the `KeyboardInput` arm acts on it today (`:2772`). `on_ime` returns the replay's
    `bool` (`true` when nothing was replayed), and the `WindowEvent::Ime` arm calls
    `event_loop.exit()` on `false`. Without this, Cmd+W mid-composition on the last tab would
    close it without exiting — the state #46 just fixed.
  - The AppKit read is a thin shim; "replay or not, and as what" is a pure function over a
    small plain struct (`keyCode`, `characters`, `charactersIgnoringModifiers`, flags,
    timestamp, is-key-down), so it is unit-tested without AppKit.
  - **Steelman, answered.** The probe measured `currentEvent` inside its own `insertText`,
    not inside winit's handler. And synchronous dispatch holds only while winit's handler is
    idle. If a Commit were ever *queued* (a nested run loop, or re-entry during `on_key`), it
    would run later, and `currentEvent` could be some other Cmd keyDown, which would replay
    the wrong chord. Three answers:
    (1) the queued case logs winit's own `had to queue event` debug line;
    (2) every `chord-replay` log line prints the event's `keyCode` (and `timestamp`, for
    after-the-fact correlation), and the D9 checklist confirms in the real app that the
    `keyCode` is the key just pressed;
    (3) if the other keyDown is later delivered as a `KeyboardInput` itself, D8's guard drops
    it, so a chord does not run twice — at worst it runs early. This relies on winit draining
    its pending queue first-in, first-out (`run_loop.queue_closure`), which is part of fact 4.
    This dependency is recorded as G9's fourth external fact (D10).
  - **Rejected**:
    - an `NSEvent` local monitor + stash. It was approved first and then superseded by the
      user once this option was found. It needs the `block2` feature, a stash, and
      staleness-clearing in `KeyboardInput` and `about_to_wait`, all to recover a value AppKit
      already holds.
    - a `[patch]`ed winit fork: a fork to rebase on every winit upgrade.

### D8: Replay only for a non-empty Commit, and never twice
- **Status**: assumed
- **Rationale**:
  - **When it replays.** Only when (a) the Commit's text is non-empty and (b) `currentEvent`
    is a Cmd `KeyDown`.
  - **(a) needs no `composing_tab` check.** winit emits `Ime::Commit` only when marked text
    exists (`view.rs:411`), so a non-empty Commit *is* the end of a composition. The handler
    clears `composing_tab` before the tail anyway. The replay still runs when the
    composition's tab has closed (`CommitTarget::Drop`): the syllable has no owner, but the
    chord was still pressed.
  - **The one-shot guard.** The replayed event's `NSEvent.timestamp` is stored as
    `replayed_ts: Option<f64>`. The next `KeyboardInput` takes it (one-shot). If that
    event's `currentEvent` carries the same timestamp, the event is dropped as the duplicate;
    otherwise it is handled normally. The take happens at the very top of the `KeyboardInput`
    arm body, before `suppress_key`. The arm's match guard (`state == Pressed`,
    `terminal_app.rs:2756`) already filters out releases, so a release cannot use it up. Every non-empty Commit sets `replayed_ts` to `Some(ts)` if it replayed, else `None`.
  - **Why the guard exists.** Today it never fires: winit does not emit that event
    (measured). It stops a future winit that *does* forward the key after a commit (or an IM
    that calls `doCommandBySelector` first) from running Cmd+T twice and opening two tabs.
  - **Key repeat.** Holding Cmd+T mid-composition: the first keyDown commits and replays.
    The repeats find no marked text and arrive as normal `KeyboardInput`s (`repeat: true`,
    new timestamps), exactly as an idle hold does.
  - **Rejected**: trusting today's winit behaviour silently; a guard with no clearing, which
    would compare against a stale timestamp forever.

### D9: Verification = pure-function tests + mutants + debug log + manual checklist
- **Status**: resolved (user)
- **Rationale**: synthetic events cannot drive the Korean IM inside the running winit app
  (they bypass the input method). So the AppKit half is verified by the probe, kept in this
  directory, and by hand.
  - **Unit tests.** `chord_key` and the replay decision are pure, covered by unit tests, and
    `cargo mutants` leaves 0 survivors on them.
  - **Debug log.** One `tracing::debug!` line per substitution and per replay. Names:
    `chord-substitute` / `chord-replay`, read with `RUST_LOG=app=debug,winit=debug` on stdout (the `winit` target carries winit's own `had to queue event` line, D7). The
    replay line prints `keyCode`, `timestamp`, `characters` and the resulting key. These lines
    are the oracle the checklist reads.
  - **Manual checklist** (2-Set Korean, `RUST_LOG=app=debug,winit=debug`). Each step names its expected
    log line:
    1. Idle, Cmd+T → a new tab opens; one `chord-substitute` line (`ㅅ`→`t`).
    2. Idle, Cmd+Shift+M → the ai-monitor toggles; Cmd+Shift+S → the sidebar toggles;
       Cmd+Shift+R → the rename field opens; Cmd+Shift+G → the capture chord fires: either
       the region-selection crosshair or macOS's "Screenshots are not allowed" dialog appears
       (every rebuild voids the Screen Recording grant, G7) — both prove the chord reached the
       app; pressing Esc / dismissing then logs `capture: no file at …`.
    3. Select text, Cmd+C → copied; Cmd+V → pasted.
    4. Type `하` (still composing), Cmd+T → `하` lands in the **old** tab's shell, then a new
       tab opens. One `chord-replay` line whose `keyCode` is 17.
    5. Compose `ㅎ`, Cmd+V → `ㅎ` then the clipboard text, in that order.
    6. Compose `ㅎ`, Cmd+1 → `ㅎ` commits, then tab 1 is active.
    7. In the rename field (Cmd+Shift+R): compose `하`, Cmd+A → `하` is in the field and the
       name is selected. Compose `하`, Cmd+← → `하` is in the field and the cursor is at the
       start.
    8. Only one tab open: compose `하`, Cmd+W → the tab closes **and the app exits**.
    9. Hold Cmd+T while composing → one replay, then normal repeats; no doubled first tab.
    10. Switch to ABC and repeat steps 1–3 → identical behaviour, with **no**
        `chord-substitute` lines.
    11. ABC: Opt+e (dead acute pending), then Cmd+T → a new tab opens, and **no**
        `chord-substitute` or `chord-replay` line appears (R0.3's dead-key clause).
  - **Rejected**: an `#[ignore]`d in-process AppKit test — more code, cannot run on CI, and
    the probe already measures the same facts.

### D10: Record the external facts as a gap; correct the stale claim
- **Status**: assumed
- **Rationale**: the fix rests on four facts this repo cannot pin:
  1. the Korean IM answers a Cmd keyDown with `insertText`, then `doCommandBySelector`;
  2. winit returns early on `Committed`;
  3. `currentEvent` is the keyDown while the Commit is handled;
  4. winit dispatches that Commit synchronously (its handler is idle inside `keyDown`), and
     drains its pending queue first-in, first-out.

  They go into `docs/architecture.md` as **G9**, beside G5–G8. The oracles are the probe and
  the `chord-replay` log line. `specs/hangul-ime-and-font-fallback/spec.md:320` gets a
  one-line correction note pointing here, without rewriting that spec.

### D11: What the replayed key is
- **Status**: assumed
- **Rationale**: the replay must produce the same `KeyInput` an idle press would have
  produced.
  - **A printable key.** If `characters` is one ASCII-graphic char, the key comes from
    `chord_key` (D1–D4) called with `logical = Character(charactersIgnoringModifiers)`,
    `text = Some(charactersIgnoringModifiers)` (what winit's `to_text()` gives idle,
    `event.rs:153`) and `text_with_all_modifiers = Some(characters)`, so replay and idle share
    one normalisation — a pass-through (Cmd+1, Cmd+Shift+]) keeps winit's idle key and text.
  - **A function key.** If `charactersIgnoringModifiers` is in AppKit's function-key
    private-use range (U+F700–U+F8FF, e.g. U+F702 for ←), the key comes from a small table
    keyed on the **physical** code. The table covers ArrowLeft/Right/Up/Down, Home and End,
    the named keys some Cmd matcher reads (`rename.rs:203-204`; `encode_key` swallows
    Cmd+nav). `text = None`, as winit gives for these named keys idle. A key in that range
    but **not** in the table (F1–F12, PageUp/PageDown, ForwardDelete…) is **not replayed**,
    and a `chord-replay skipped` line says so. Building it would give
    `Key::Character("\u{F72C}")`, which no matcher recognises and which the text fallback
    could write to the PTY.
  - **Any other non-ASCII `characters`.** For example Cmd+Opt+1, where Opt changes the
    character. The key is `Key::Character(charactersIgnoringModifiers)` and
    `text = charactersIgnoringModifiers`, which is what winit builds idle (`to_text()`).
    Physical-key matchers (`monitor_jump`) still fire on `from_scancode`.
  - **Anything else** — Enter, Backspace, Tab, Escape, an empty string, a control char — is
    **not replayed**. A `chord-replay skipped` debug line says so. Those keys behave as they
    do today: lost mid-composition. That is recorded in Known Gaps.
  - **Rejected**: replaying everything as `Character` (garbage keys); a full `KeyCode` →
    `NamedKey` table (copies winit's private `code_to_key` for keys no Cmd matcher reads).

## Constraints
- **No new crates.** `objc2-app-kit`'s already-enabled `NSApplication`/`NSEvent` are enough; no
  `block2`, no winit fork.
- **ABC behaviour is byte-for-byte unchanged**: no `KeyInput` built from an ABC `KeyboardInput`
  may differ from today's (D2 is the mechanism; tests assert it). The replay path cannot change
  a working ABC chord either: it fires only on a Commit, and a Cmd keyDown that causes a Commit
  is one winit already drops today — so at most it turns a lost chord into a working one.
- The `NSEvent` is only **read** — never consumed, re-posted or modified.
- `term-core` and `renderer` are untouched (dependency direction).
- No `#[cfg(target_os)]` branches; no `unwrap()` outside tests.
- The chord table itself (which keys do what) does not change.

## Known Gaps
- **External facts (→ G9):** if the Korean IM stops committing on a Cmd keyDown, or winit stops
  swallowing the key after a commit, the replay path goes quiet (or would double-fire, which D8
  guards). Re-run `composeprobe.swift` to tell which.
- **Input sources other than 2-Set Korean are unmeasured** (3-Set Korean, Japanese, Chinese,
  Russian…). D1/D2 are generic, but only 2-Set Korean and ABC are asserted.
- **Ctrl+C / control bytes:** no code path in `crates/app` appears to turn Ctrl+<letter> into a
  C0 byte on any source, and under Korean `characters` is empty for Ctrl+C. Out of scope; a
  sibling spec if confirmed.
- **Cmd+Enter / Cmd+Backspace / Cmd+Tab / Cmd+Escape / Cmd+F1–F12 / Cmd+PageUp/PageDown /
  Cmd+ForwardDelete mid-composition stay lost** (D11). No
  Cmd matcher reads them today. The Korean IM's behaviour for Cmd+named keys mid-composition
  is also unmeasured — checklist step 7's Cmd+← is the first real reading.
- **The no-commit IM mode** (Research, `digitprobe.run2/3.out`): if the Korean IM ever
  swallows a Cmd keyDown mid-composition *without* committing, there is no `Ime::Commit`, D7
  has nothing to replay, and the chord stays lost with `하` still composing. Believed to be a
  synthetic-event artifact; checklist steps 4–6 would show it as "no `chord-replay` line and
  the syllable still underlined".
- **Probe flakiness:** see the Research caveat — synthetic events sometimes commit the first jamo
  after a Cmd chord instead of composing it.
- **End-to-end confirmation is manual** (D9) and depends on the user running the checklist.

## Requirements

### R0: Every Cmd chord behaves the same under 2-Set Korean as under ABC, idle and mid-composition

#### R0.1: Idle chord fires under Korean
- **Given**: the 2-Set Korean input source is active and nothing is being composed
- **When**: the user presses Cmd+T (AppKit: `charactersIgnoringModifiers` = `ㅅ`, `characters` = `t`)
- **Then**: a new tab opens, exactly as under ABC

#### R0.2: Mid-composition chord fires after the syllable commits
- **Given**: 2-Set Korean, `하` is being composed in tab A
- **When**: the user presses Cmd+T
- **Then**: `하` is written to tab A's shell, and after that a new tab opens

#### R0.3: ABC is untouched
- **Given**: the ABC input source is active
- **When**: any key is pressed
- **Then**: the `KeyInput` the `KeyboardInput` arm builds is identical to the one it builds today, and no `chord-substitute` or `chord-replay` line is logged — including with an Opt+e dead key pending, which produces no commit (`composeprobe.deadkey.out`)

### R1: `chord_key` — the pure substitution (D1, D2, D3, D4)

#### R1.1: Korean Cmd+letter becomes the Roman letter
- **Given**: logical `Key::Character("ㅅ")`, `text_with_all_modifiers` = `Some("t")`, mods = super
- **When**: `chord_key` is called
- **Then**: it returns key `Key::Character("t")` and text `Some("t")`

#### R1.2: Shift upper-cases the substituted letter
- **Given**: logical `Key::Character("ㅡ")`, `text_with_all_modifiers` = `Some("m")`, mods = super+shift
- **When**: `chord_key` is called
- **Then**: it returns key `Key::Character("M")` and text `Some("M")`

#### R1.3: An ASCII logical key passes through unchanged
- **Given**: logical `Key::Character("}")`, `text_with_all_modifiers` = `Some("]")`, mods = super+shift (ABC or Korean Cmd+Shift+])
- **When**: `chord_key` is called
- **Then**: it returns the input key and text unchanged (`"}"`)

#### R1.4: No Cmd, no substitution
- **Given**: logical `Key::Character("ㅅ")`, `text_with_all_modifiers` = `Some("ㅅ")`, mods = empty (plain Hangul typing) — and separately mods = ctrl
- **When**: `chord_key` is called
- **Then**: it returns the input key and text unchanged

#### R1.5: A Command-layer string that is not one ASCII-graphic char blocks substitution
- **Given**: logical `Key::Character("ㅊ")`, mods = super+ctrl, and `text_with_all_modifiers` is in turn `None`, `Some("\u{3}")`, `Some(" ")`, `Some("tt")`, `Some("ㅊ")`
- **When**: `chord_key` is called for each
- **Then**: every call returns the input key and text unchanged

#### R1.6: Named keys pass through
- **Given**: logical `Key::Named(ArrowLeft)`, mods = super, `text_with_all_modifiers` = `Some("\u{F702}")`
- **When**: `chord_key` is called
- **Then**: it returns `Key::Named(ArrowLeft)` and the input text unchanged

### R2: The `KeyboardInput` arm builds its `KeyInput` through `chord_key` (D5, D4, D9)

#### R2.1: The one construction site uses the substitution
- **Given**: the `WindowEvent::KeyboardInput` press arm in `terminal_app.rs`
- **When**: it builds the `KeyInput` it hands to `on_key`
- **Then**: `key` and `text` are `chord_key(&key_event.logical_key, key_event.text.as_deref(), key_event.text_with_all_modifiers(), mods)` and `mods`/`physical` are unchanged from today; `suppress_key` still reads winit's raw `logical_key`

#### R2.2: Every letter-chord matcher accepts the substituted input
- **Given**: Korean-shaped raw inputs (logical jamo + Roman Command-layer string + mods) for Cmd+T, Cmd+W, Cmd+C, Cmd+V, Cmd+Shift+S, Cmd+Shift+M, Cmd+Shift+R, Cmd+Shift+G, and Cmd+A with the rename field open
- **When**: each is passed through `chord_key` and then to its matcher (`tab_action`, `is_sidebar_toggle_chord`, copy, paste, the monitor `is_toggle`, the rename `is_toggle`, `is_capture_chord`, rename select-all)
- **Then**: each matcher reports its chord (and each one still reports no chord for the raw, unsubstituted jamo input — proving the test exercises the fix)

#### R2.3: An unclaimed Cmd+letter reaches the shell as on ABC
- **Given**: Korean Cmd+Shift+X (`ㅌ` / `x`), which no chord claims
- **When**: the substituted input reaches `encode_key`
- **Then**: the bytes are `b"X"`, the same as ABC's, never the jamo's UTF-8

#### R2.4: Substitution is logged
- **Given**: `RUST_LOG=app=debug`
- **When**: `chord_key` substitutes
- **Then**: one `chord-substitute` debug line is printed naming the original and substituted key; nothing is logged when it does not substitute

### R3: The replay decision — a pure function over a plain event struct (D7, D8, D11)

#### R3.1: A Cmd letter keyDown during a non-empty commit is replayed as the idle chord would be
- **Given**: event facts `{ is_key_down: true, flags: cmd+shift, key_code: 46, characters: "m", characters_ignoring_modifiers: "ㅡ", timestamp: t }` and a non-empty commit
- **When**: the replay decision runs
- **Then**: it returns `Replay(KeyInput { key: Character("M"), text: Some("M"), mods: super+shift, physical: Code(KeyM) })` with timestamp `t`

#### R3.2: Not a Cmd keyDown → no replay
- **Given**: event facts with `is_key_down: false`, and separately a keyDown without the Command flag
- **When**: the replay decision runs with a non-empty commit
- **Then**: it returns `None` for both

#### R3.3: An empty commit → no replay
- **Given**: Cmd+T event facts
- **When**: the replay decision runs with an empty commit text
- **Then**: it returns `None`

#### R3.4: A table function key is replayed as its named key
- **Given**: Cmd keyDown facts with `characters_ignoring_modifiers` = `"\u{F702}"`, `key_code` = 123 (ArrowLeft); and the same for Right/Up/Down/Home/End
- **When**: the replay decision runs with a non-empty commit
- **Then**: it returns `Replay` with `key: Named(ArrowLeft)` (etc.) and `text: None`

#### R3.5: A function key outside the table is skipped
- **Given**: Cmd keyDown facts with `characters_ignoring_modifiers` = `"\u{F704}"` (F1), and separately PageUp and ForwardDelete
- **When**: the replay decision runs with a non-empty commit
- **Then**: it returns `Skip`, and the caller logs one `chord-replay skipped` line

#### R3.6: An Option-altered character is replayed as winit would build it
- **Given**: Cmd+Opt keyDown facts with `key_code` = 18, `characters_ignoring_modifiers` = `"¡"`, `characters` = `"¡"`
- **When**: the replay decision runs with a non-empty commit
- **Then**: it returns `Replay` with `key: Character("¡")`, `text: Some("¡")`, `physical: Code(Digit1)`, and `monitor_jump` on that `KeyInput` returns `Some(0)`

#### R3.7: Other non-printables are skipped
- **Given**: Cmd keyDown facts whose `characters_ignoring_modifiers` is `"\r"` (Enter), `"\u{7f}"` (Backspace), `"\t"`, `"\u{1b}"`, or empty
- **When**: the replay decision runs with a non-empty commit
- **Then**: each returns `Skip`

#### R3.8: A printable chord that `chord_key` passes through is replayed as winit builds it idle
- **Given**: Cmd keyDown facts with `key_code` = 18, `characters` = `"1"`, `characters_ignoring_modifiers` = `"1"`; and separately Cmd+Shift with `key_code` = 30, `characters` = `"]"`, `characters_ignoring_modifiers` = `"}"`
- **When**: the replay decision runs with a non-empty commit
- **Then**: it returns `Replay` with `key: Character("1")`, `text: Some("1")`, `physical: Code(Digit1)`, and `tab_action` on it returns `Jump(0)`; and for the second, `key: Character("}")`, `text: Some("}")`, and `tab_action` returns `Next`

### R4: The `Ime::Commit` handler replays the lost chord (D6, D7)

#### R4.1: The syllable is placed before the chord runs
- **Given**: `하` is composing in tab A and Cmd+T is pressed (the Commit arrives with `currentEvent` = the Cmd+T keyDown)
- **When**: the Commit handler runs
- **Then**: `하` reaches tab A's shell (visible at tab A's prompt) before the new tab exists, and the new tab's prompt shows no `하` — observable as checklist step 4. (Implementation note, D7: the tail runs after `set_preedit("")` and `composing_tab = None`.)

#### R4.2: A commit claimed by the rename field also replays
- **Given**: the rename field is open and `하` is composing in it
- **When**: Cmd+A is pressed (Commit claimed by the overlay via `OverlayCommand::ImeCommit`)
- **Then**: `하` is in the field and the replayed Cmd+A then selects the whole name — the overlay early-return does not skip the replay

#### R4.3: A replayed exit request exits the app
- **Given**: one tab is open and `하` is composing in it
- **When**: Cmd+W is pressed and the replayed `on_key` returns `false`
- **Then**: `on_ime` returns `false` and the `WindowEvent::Ime` arm calls `event_loop.exit()`

#### R4.4: The replay is logged with the event's identity
- **Given**: `RUST_LOG=app=debug`
- **When**: a chord is replayed
- **Then**: one `chord-replay` debug line prints `keyCode`, `timestamp`, `characters` and the resulting key

### R5: A replayed chord never runs twice (D8)

#### R5.1: The duplicate `KeyboardInput` is dropped
- **Given**: a Commit replayed the event with timestamp `t` (`replayed_ts = Some(t)`)
- **When**: the next `KeyboardInput` press arrives while `currentEvent.timestamp == t`
- **Then**: it is dropped before `suppress_key`/`on_key`, and `replayed_ts` becomes `None`

#### R5.2: A different event clears the guard and is handled
- **Given**: `replayed_ts = Some(t)`
- **When**: the next `KeyboardInput` press arrives with `currentEvent.timestamp == u != t` (e.g. a key repeat)
- **Then**: it is handled normally, and `replayed_ts` becomes `None`

#### R5.3: Every non-empty commit resets the guard
- **Given**: `replayed_ts = Some(t)`
- **When**: a non-empty Commit runs that does not replay
- **Then**: `replayed_ts` becomes `None` (and becomes `Some(t')` when it does replay)

### R6: The external facts are documented (D10)

#### R6.1: Gap G9 exists
- **Given**: `docs/architecture.md`'s gap list
- **When**: the change lands
- **Then**: a **G9** entry names the four external facts (the IM's insertText→doCommandBySelector order; winit's `Committed` early-return; `currentEvent` = the keyDown during the Commit; synchronous + FIFO dispatch), what breaks if each changes, and the oracles (`composeprobe.swift`, the `chord-replay` line)

#### R6.2: The stale Hangul-spec claim is corrected
- **Given**: `specs/hangul-ime-and-font-fallback/spec.md:320` says Cmd+T passes the composing gate as a named key
- **When**: the change lands
- **Then**: a one-line note beside it says Cmd+T is a `Key::Character` and points to `specs/korean-input-chords`

### R7: Verification (D9)

#### R7.1: The pure functions are mutation-clean
- **Given**: `chord_key`, the replay decision and the duplicate-guard function
- **When**: `cargo mutants -f <their files> -j 4` (scoped to them) runs
- **Then**: 0 survivors (an equivalent mutant is rewritten away, not excluded)

#### R7.2: The workspace gates are green
- **Given**: the finished change
- **When**: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` run
- **Then**: all pass

#### R7.3: The manual checklist passes in the real app
- **Given**: the release app under 2-Set Korean with `RUST_LOG=app=debug,winit=debug`
- **When**: the user runs the 11 steps of D9's checklist
- **Then**: every step shows its expected behaviour and log line, and the result of each step is recorded in this spec

## Tasks

### T1: `chord_key` — the pure substitution, with its tests [infra]
- **Fulfills**: R1 (R1.1–R1.6), R2.3, R2.4 (emits the line), R7.1 (for `chord_key`)
- **Depends on**: (none) ← parallel with T5
- **Files**: `crates/app/src/input.rs` (new `pub fn chord_key(logical: &Key, text: Option<&str>, text_with_all_modifiers: Option<&str>, mods: ModifiersState) -> (Key, Option<String>)` beside `tab_action`; on pass-through it returns `logical.clone()` and `text` untouched, which is what makes ABC byte-for-byte identical, plus its unit tests and an `encode_key` test for R2.3). The `chord-substitute` `tracing::debug!` lives inside it, so the one rule logs at one place.
- **Done when**: R1.x/R2.3 tests green; `cargo mutants -f crates/app/src/input.rs -F chord_key -j 4` → 0 survivors.

### T2: Route the `KeyboardInput` arm through `chord_key`; prove every matcher [vertical]
- **Fulfills**: R0.1, R0.3, R2.1, R2.2
- **Depends on**: T1
- **Files**: `crates/app/src/terminal_app.rs` (the `KeyInput` construction at the `WindowEvent::KeyboardInput` press arm, ~:2766, importing `winit::platform::modifier_supplement::KeyEventExtModifierSupplement`); test modules of `input.rs` (`tab_action`, `is_sidebar_toggle_chord`), `copy.rs`, `clipboard.rs`, `overlay.rs` (monitor `is_toggle`), `rename.rs` (`is_toggle`, select-all), `capture.rs` (`is_capture_chord`). Each test asserts the raw jamo input does **not** match and the `chord_key`-normalised input **does**.
- **Done when**: R2.2's nine matcher tests green; `suppress_key` still receives `&key_event.logical_key`.

### T3: The replay decision and the duplicate guard — pure, AppKit-free [infra]
- **Fulfills**: R3 (R3.1–R3.8), R5 (R5.1–R5.3, the pure halves: `take_duplicate`, `guard_after_commit`), R7.1 (for these)
- **Depends on**: T1 ← parallel with T2 (no shared files)
- **Files**: new `crates/app/src/ime_chord.rs` (+ `pub(crate) mod ime_chord;` in `crates/app/src/lib.rs`, alphabetical with the others):
  - `struct CmdEventFacts { is_key_down, command, shift, control, option, key_code: u16, characters: String, characters_ignoring_modifiers: String, timestamp: f64 }`;
  - `enum Replay { None, Skip(&'static str), Key(KeyInput, f64) }`;
  - `fn replay_decision(facts: Option<&CmdEventFacts>, commit_text: &str) -> Replay` — uses `input::chord_key` for printable keys (D11), a six-entry `KeyCode` → `NamedKey` table for U+F700–U+F8FF keys (ArrowLeft/Right/Up/Down, Home, End), `PhysicalKeyExtScancode::from_scancode` for `physical`;
  - `fn take_duplicate(replayed_ts: &mut Option<f64>, current_ts: Option<f64>) -> bool` (R5.1/R5.2: always leaves `None`);
  - `fn guard_after_commit(replay: &Replay) -> Option<f64>` (R5.3: `Some(ts)` for `Replay::Key(_, ts)`, else `None`) — the value the Commit tail assigns to `replayed_ts`.
- **Done when**: R3.x, R5.1/R5.2 (`take_duplicate`) and R5.3 (`guard_after_commit`) unit tests green (R3.6 also calls `input::monitor_jump`); `cargo mutants -f crates/app/src/ime_chord.rs -j 4` → 0 survivors.

### T4: Wire the replay into `Ime::Commit` and the guard into `KeyboardInput` [vertical]
- **Fulfills**: R0.2, R4 (R4.1–R4.4), R5 (wiring of R5.1–R5.3)
- **Depends on**: T2, T3 (same `terminal_app.rs` as T2)
- **Files**: `crates/app/src/terminal_app.rs`:
  - a new field `replayed_ts: Option<f64>` on `struct Running` (beside `composing_tab`, ~:317), initialised `None`;
  - a thin shim reading `NSApplication::sharedApplication(mtm).currentEvent()` (objc2-app-kit, read-only) into `Option<CmdEventFacts>` — **unfiltered**: any current event becomes facts (`is_key_down`/`command` are fields), `None` only when there is no current event. The Commit tail passes it to `replay_decision`; the `KeyboardInput` arm passes its `timestamp` as `current_ts` to `take_duplicate`;
  - `on_ime` returns `bool`; the Commit arm's two exits (overlay-claimed early return ~:557-567 and fall-through ~:617-620) converge on one tail that runs **after** `set_preedit("")` / `composing_tab = None`, calls `replay_decision`, sets `replayed_ts` per R5.3, logs `chord-replay` (keyCode, timestamp, characters, key) or `chord-replay skipped`, and dispatches a `Key` through `on_key`, returning its bool;
  - the `WindowEvent::Ime` arm calls `event_loop.exit()` when `on_ime` returns `false`;
  - the `KeyboardInput` press arm calls `take_duplicate` first (before `suppress_key`) and returns early on `true`.
- **Done when**: workspace test/clippy/fmt green (R7.2). R4.1–R4.3 cannot be driven without a window and the Korean IM (D9), so they are verified by T6's checklist steps 4, 5, 7, 8; the code review checks the tail placement against R4.1/R4.2.

### T5: Document the external facts [infra]
- **Fulfills**: R6 (R6.1, R6.2)
- **Depends on**: (none) ← parallel with T1–T4 (docs only)
- **Files**: `docs/architecture.md` (new **G9** after G8: the four facts, what breaks if each changes, oracles `specs/korean-input-chords/composeprobe.swift` and the `chord-replay` line); `specs/hangul-ime-and-font-fallback/spec.md` (a one-line note beside line 320).
- **Done when**: both edits present; G9 names all four facts.

### T6: Run the manual checklist with the user and record the result [verification]
- **Fulfills**: R2.4 (checklist steps 1 and 10 read the presence and absence of `chord-substitute`), R7.2 (final run), R7.3
- **Depends on**: T4, T5
- **Files**: `specs/korean-input-chords/spec.md` (append `## Manual Verification` with each D9 step's observed result and log line).
- **Done when**: `cargo test --workspace`, clippy `-D warnings`, `cargo fmt --check` green on the final tree; the release app is built with `cargo build --release -p app`, and the user launches it **from their own terminal** (not from Claude Code, whose env leaks into tabs) as `RUST_LOG=app=debug,winit=debug ./target/release/app | tee /tmp/korean-chords.log` — `/run-term`'s defaults (debug build, `RUST_LOG=info`) would hide every line the checklist reads. The user runs D9's eleven steps under 2-Set Korean, and every step's outcome is recorded, plus whether any `had to queue event` line appeared. A failing step reopens T2 or T4, not this task.

## External Dependencies

### Pre-work
- (none) — the probes (`imeprobe.swift`, `composeprobe.swift`) and `composeprobe.out` are already in `specs/korean-input-chords/`.

### Post-work
- The user runs D9's manual checklist (T6) with the **2-Set Korean** input source enabled in System Settings › Keyboard › Input Sources. Synthetic events cannot stand in for this (they bypass the input method).
- Commit / PR only when the user asks.

## Spec Review
- **Verdict**: NEEDS_FIX on review 3 (the last allowed); every issue it raised was fixed
  afterwards, but the fixed file was **not** re-reviewed (limit reached).
- **Reviews**: 3 (plus 3 L2-reviewer rounds on Decisions: NEEDS_FIX → NEEDS_FIX → PASS)
- **Fixed**:
  - [Readiness] T1/R2.1 — `chord_key` takes winit's `text` and returns it untouched on pass-through (ABC identity is explicit); D5 updated to the same four-parameter signature.
  - [Testability] D7/D9/R7.3/T6 — checklist runs with `RUST_LOG=app=debug,winit=debug` so winit's `had to queue event` line is visible; T6 gives the exact release build + launch command instead of `/run-term`'s defaults.
  - [Testability] D7 — the checklist confirms the replay's `keyCode`, not its timestamp.
  - [Testability] R4.1 — Then reworded to observable order (`하` at tab A's prompt before the new tab exists); the private-state detail kept as an implementation note.
  - [Coverage] R2.4 — added to T6 (checklist steps 1 and 10).
  - [Grounding] Research — the "not even Cmd+1" claim rested on a run with no marked text; re-measured with `digitprobe.swift` (run 1: Cmd+1, Cmd+Shift+], Cmd+T all lost mid-composition). Runs 2–3 showed a no-commit IM mode, recorded in Research and Known Gaps.
  - [Readiness] D11/T3 — the printable replay branch's `text` is `Some(charactersIgnoringModifiers)`; new R3.8 (Cmd+1 → `Jump(0)`, Cmd+Shift+] → `Next`).
  - [Consistency] R0.3/Constraint — ABC dead key measured (`composeprobe.deadkey.out`: no marked text, no commit); Constraint now argues why the replay path cannot change a working ABC chord; checklist step 11 asserts it.
  - [Readiness] T4 — `replayed_ts` field on `struct Running`; the `currentEvent` shim is unfiltered and also feeds `take_duplicate`.
  - [Testability] D9 step 2 — Cmd+Shift+G judged by the crosshair or the refusal dialog, since a rebuild voids the Screen Recording grant (G7).
  - [Consistency] D2 × Opt — measured with `optprobe.swift`: on ABC the Cmd+Opt logical key is ASCII, so D2 never substitutes; under Korean it yields ABC's key. Recorded in Research.
  - [Testability] R5.3 — new pure `guard_after_commit` in T3, unit-tested and in T3's mutants run.
  - [Testability] R0.3 dead-key clause — checklist step 11.
  - [Grounding] Confirmed Goal — names the probes and outputs actually present.
- **Open**:
  - Approved items reworded during the review (substance unchanged, flagged for the final gate): R2.1 (four-arg call), R4.1 (observable Then), R0.3 (dead-key clause), R3.8 (new sub-requirement), D5/D7/D11 wording, D9 checklist (RUST_LOG, step 2 wording, step 11).

## Manual Verification (T6) — 2026-10-11, release bundle, 2-Set Korean

Launched via `open -n target/mac-terminal.app --env RUST_LOG=app=debug,winit=debug` (no
Claude Code env in the tab: no `TMUX`/`TERM_PROGRAM`/`CLAUDE_CODE_*`). Evidence is the debug
log; the user reported the behaviour as working.

| Step | Key | Log evidence | Result |
|---|---|---|---|
| 1 | Cmd+T idle | `chord-substitute from=ㅅ to=t`; tab 1 appears | ✅ |
| 2 | Cmd+Shift+M idle (first run) | `chord-substitute from=ㅡ to=M` ×2 | ✅ |
| 4 | compose, Cmd+T | `chord-replay key_code=Some(17) … key=Character("t")` | ✅ replayed |
| 5 | compose, Cmd+V | `chord-replay key_code=Some(9) … key=Character("v")` | ✅ replayed |
| — | Cmd+W idle, last tab | `chord-substitute from=ㅈ to=w`; process exited | ✅ (KeyboardInput path) |
| 3, 6, 7, 9, 10, 11 | — | not in the logs | not exercised |
| 8 | compose, Cmd+W on last tab | the final Cmd+W presses were idle (no `chord-replay`) | not exercised — R4.3's replay-exit path unverified in-app |

**G9 facts confirmed in-app:** fact 3 — both replays carried the key code of the key pressed
(17 = T, 9 = V), so `currentEvent` is the lost keyDown inside winit's handler too, not only in
the probe; fact 4 — no `had to queue event` line outside window creation. No
`chord-replay duplicate dropped` line (fact 2 holds: winit still drops the key).
