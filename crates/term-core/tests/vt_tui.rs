//! Headless integration tests for full-screen TUI rendering
//! (feature: vt-tui-completeness).
//!
//! The per-sequence semantics live next to their implementations in
//! `src/parser.rs`. What lives *here* is the class of failure those cannot catch:
//! sequences that are each individually correct but desync **in combination**, and
//! the robustness sweep over the whole newly-reachable surface.
//!
//! Assertion-based rather than insta, for the same reason `scrollback.rs` is: these
//! invariants are positional and stateful (cursor, pending-wrap, history length),
//! not visual.
//!
//! Requirement anchors: R0 (composite frame + no dropped sequences), R8 (no panic,
//! no silent corruption).

use term_core::{Color, Terminal};

const FG: Color = Color(0xE0E0E0);
const BG: Color = Color(0x101010);

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, FG, BG)
}

/// One visible row, trimmed.
fn row(t: &Terminal, r: u16) -> String {
    let snap = t.snapshot();
    let mut s = String::new();
    for col in 0..snap.cols {
        s.push(snap.cell(col, r).map(|c| c.ch).unwrap_or(' '));
    }
    s.trim_end().to_string()
}

/// Every visible row, trimmed.
fn rows(t: &Terminal) -> Vec<String> {
    let snap = t.snapshot();
    (0..snap.rows).map(|r| row(t, r)).collect()
}

// --- R0.1: a realistic composite frame -------------------------------------------

#[test]
fn realistic_tui_frame_lands_correctly() {
    // R0.1 / D16. The inversion probe named this as the most likely way every
    // requirement passes while a real TUI still renders wrong: each sequence is
    // unit-correct, but their *interaction* desyncs. So this replays one frame the
    // way a full-screen app actually emits it — region, absolute positioning,
    // in-line edits, a reverse index, and a mode toggle — then asserts the grid,
    // the cursor, the pending-wrap flag and the history length TOGETHER.
    let mut t = term(10, 6);

    // A status line at the top and a footer at the bottom, outside the pane.
    t.advance(b"\x1b[1;1HHEADER");
    t.advance(b"\x1b[6;1HFOOTER");

    // Claim rows 2..=5 (1-based) as the scrolling message pane. This homes the
    // cursor, which is exactly why the app repositions immediately afterwards.
    t.advance(b"\x1b[2;5r");
    assert_eq!(t.snapshot().cursor.row, 0, "DECSTBM homed the cursor");

    // Fill the pane.
    t.advance(b"\x1b[2;1Hline one");
    t.advance(b"\x1b[3;1Hline two");
    t.advance(b"\x1b[4;1Hline tre");
    assert_eq!(
        rows(&t),
        ["HEADER", "line one", "line two", "line tre", "", "FOOTER"]
    );

    // Edit in place: drop "line " from row 2 (DCH), then insert two cells (ICH).
    t.advance(b"\x1b[3;1H\x1b[5P");
    assert_eq!(row(&t, 2), "two", "DCH pulled the tail left");
    t.advance(b"\x1b[2@");
    assert_eq!(row(&t, 2), "  two", "ICH shifted it back right");

    // Scroll the pane by feeding past its bottom margin — the header and footer
    // must not move, and none of this may reach scrollback.
    t.advance(b"\x1b[5;1Hline for\n");
    assert_eq!(
        rows(&t),
        ["HEADER", "  two", "line tre", "line for", "", "FOOTER"],
        "the pane scrolled; header and footer stayed put"
    );
    assert_eq!(
        t.history_len(),
        0,
        "a partial region never feeds scrollback"
    );

    // Reverse-index at the pane top scrolls it back down.
    t.advance(b"\x1b[2;1H\x1bM");
    assert_eq!(
        rows(&t),
        ["HEADER", "", "  two", "line tre", "line for", "FOOTER"],
        "RI scrolled the pane down without touching the chrome"
    );

    // Erase the middle of a row with ECH (no shift), then the whole row with 2K.
    t.advance(b"\x1b[4;1Hline tre\x1b[4;5H\x1b[2X");
    assert_eq!(
        row(&t, 3),
        "line  re",
        "ECH blanked cols 4-5 in place — the tail did NOT shift left"
    );
    t.advance(b"\x1b[4;1H\x1b[2K");
    assert_eq!(row(&t, 3), "", "2K cleared the whole row");

    // Autowrap off, print past the margin, then back on — the pane must not scroll.
    let before = rows(&t);
    t.advance(b"\x1b[5;1H\x1b[?7labcdefghijkl\x1b[?7h");
    assert_eq!(row(&t, 4), "abcdefghil", "chars piled onto the last column");
    assert_eq!(row(&t, 5), before[5], "FOOTER survived: nothing scrolled");
    assert!(
        !t.pending_wrap(),
        "autowrap-off never arms the deferred wrap"
    );

    // Hand the screen back. The region resets and the cursor homes.
    t.advance(b"\x1b[r");
    assert_eq!(t.snapshot().cursor.row, 0);
    assert_eq!(t.snapshot().cursor.col, 0);

    // And now that the region is full-screen again, scrolling DOES retain history.
    t.advance(b"\x1b[6;1H\n");
    assert_eq!(
        t.history_len(),
        1,
        "the reset restored real scrollback behavior"
    );
}

// --- R0.2: every named sequence reaches the grid ---------------------------------

#[test]
fn every_named_sequence_produces_an_observable_change() {
    // R0.2. Before this feature all of these hit `_ => {}` and vanished. Each case
    // is fed to a grid whose state would visibly change if handled, so a silent
    // regression back to the catch-all fails loudly here rather than as a rendering
    // mystery months later.
    struct Case {
        name: &'static str,
        setup: &'static [u8],
        seq: &'static [u8],
    }
    let cases = [
        Case {
            // Observable via its cursor homing, so the setup parks the cursor
            // somewhere else first.
            name: "CSI r (DECSTBM)",
            setup: b"\x1b[2;2H",
            seq: b"\x1b[2;4r",
        },
        Case {
            name: "CSI S (SU)",
            setup: b"aaa\r\nbbb\r\nccc\x1b[1;1H",
            seq: b"\x1b[1S",
        },
        Case {
            name: "CSI T (SD)",
            setup: b"aaa\r\nbbb\r\nccc\x1b[1;1H",
            seq: b"\x1b[1T",
        },
        Case {
            name: "CSI L (IL)",
            setup: b"aaa\r\nbbb\x1b[1;1H",
            seq: b"\x1b[1L",
        },
        Case {
            name: "CSI M (DL)",
            setup: b"aaa\r\nbbb\x1b[1;1H",
            seq: b"\x1b[1M",
        },
        Case {
            name: "CSI @ (ICH)",
            setup: b"abcd\x1b[1;1H",
            seq: b"\x1b[2@",
        },
        Case {
            name: "CSI P (DCH)",
            setup: b"abcd\x1b[1;1H",
            seq: b"\x1b[2P",
        },
        Case {
            name: "CSI X (ECH)",
            setup: b"abcd\x1b[1;1H",
            seq: b"\x1b[2X",
        },
        Case {
            name: "CSI 1K (EL to cursor)",
            setup: b"abcd\x1b[1;3H",
            seq: b"\x1b[1K",
        },
        Case {
            name: "CSI 2K (EL whole line)",
            setup: b"abcd\x1b[1;3H",
            seq: b"\x1b[2K",
        },
        Case {
            name: "ESC D (IND)",
            setup: b"aaa\r\nbbb\r\nccc\x1b[3;1H",
            seq: b"\x1bD",
        },
        Case {
            name: "ESC M (RI)",
            setup: b"aaa\r\nbbb\r\nccc\x1b[1;1H",
            seq: b"\x1bM",
        },
    ];

    for c in cases {
        let mut t = term(8, 3);
        t.advance(c.setup);
        let before = (rows(&t), t.snapshot().cursor);
        t.advance(c.seq);
        let after = (rows(&t), t.snapshot().cursor);
        assert_ne!(
            before, after,
            "{} was silently dropped — it changed nothing",
            c.name
        );
    }

    // DECSTBM's real effect is the region itself, not the incidental cursor homing
    // the loop detects — assert it directly so the case cannot pass vacuously.
    let mut t = term(8, 6);
    assert_eq!(t.scroll_region(), (0, 5));
    t.advance(b"\x1b[2;4r");
    assert_eq!(t.scroll_region(), (1, 3), "CSI r actually set the region");

    // The two mode sequences change no cell, so they are checked by behavior.
    let mut t = term(4, 2);
    t.advance(b"\x1b[?7l");
    t.advance(b"abcdef"); // 6 chars into 4 columns
    assert_eq!(row(&t, 1), "", "?7l was dropped — the line still wrapped");
    t.advance(b"\x1b[?7h");
    t.advance(b"\x1b[1;1Habcdef");
    assert_eq!(
        row(&t, 1),
        "ef",
        "?7h was dropped — wrapping did not come back"
    );
}

// --- R1.6 / R3.5: contract and guard invariants ----------------------------------

#[test]
fn the_scroll_region_is_invisible_to_the_renderer_contract() {
    // R1.6 / spec C-2. The region is terminal *mode* state, like alt_screen and
    // app_cursor_keys — it must not leak into `GridSnapshot`, which is the
    // *renderer* contract. Proven behaviorally rather than structurally: two grids
    // that differ ONLY by their scroll region must produce equal snapshots. If a
    // region field were ever added to the snapshot, this comparison would diverge.
    let mut plain = term(8, 6);
    let mut banded = term(8, 6);
    for t in [&mut plain, &mut banded] {
        t.advance(b"\x1b[1;1Halpha\x1b[3;1Hbravo\x1b[5;1Hcharlie");
    }
    banded.advance(b"\x1b[2;5r"); // a region…
    banded.advance(b"\x1b[5;8H"); // …then undo its cursor homing side effect
    plain.advance(b"\x1b[5;8H");

    assert_ne!(
        banded.scroll_region(),
        plain.scroll_region(),
        "precondition: the two terminals really do differ by region"
    );
    assert_eq!(
        banded.snapshot().cells(),
        plain.snapshot().cells(),
        "the region must not change what the renderer sees"
    );
    assert_eq!(banded.snapshot().cursor, plain.snapshot().cursor);
    assert_eq!(banded.snapshot().cols, plain.snapshot().cols);
    assert_eq!(banded.snapshot().rows, plain.snapshot().rows);
    assert_eq!(banded.snapshot().view_offset, plain.snapshot().view_offset);
    assert_eq!(banded.snapshot().at_bottom, plain.snapshot().at_bottom);
    assert_eq!(banded.snapshot().at_top, plain.snapshot().at_top);
}

#[test]
fn the_alt_screen_history_guard_survives_the_region_change() {
    // R3.5. D1 added `region_is_full_screen()` as a second precondition on a history
    // push that previously depended on `!alt_screen` alone. This pins the ORIGINAL
    // guard so the new one cannot have quietly replaced it: on the alt screen,
    // history must stay frozen whether the region is full-screen or partial.
    let mut t = term(4, 3);
    t.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5"); // build real primary history first
    let before = t.history_len();
    assert!(before > 0, "precondition: primary history exists");

    // Alt screen, full-screen region (the state ?1049h leaves behind per D2).
    t.advance(b"\x1b[?1049h");
    assert_eq!(
        t.scroll_region(),
        (0, 2),
        "alt-screen enter reset the region"
    );
    for _ in 0..20 {
        t.advance(b"\r\nchurn");
    }
    assert_eq!(
        t.history_len(),
        before,
        "alt-screen + FULL-screen region must not retain (the original guard)"
    );

    // Alt screen with a partial region — both guards active at once.
    t.advance(b"\x1b[1;2r");
    for _ in 0..20 {
        t.advance(b"\x1b[1S\r\nchurn");
    }
    assert_eq!(
        t.history_len(),
        before,
        "alt-screen + partial region must not retain either"
    );

    // Back to the primary screen: retention resumes.
    t.advance(b"\x1b[?1049l");
    t.advance(b"\x1b[3;1H\n");
    assert_eq!(
        t.history_len(),
        before + 1,
        "leaving the alt screen restores real scrollback behavior"
    );
}

// --- R8: robustness ---------------------------------------------------------------

#[test]
fn extreme_and_malformed_parameters_never_panic() {
    // R8.1 / C-4. Every new arm is fed absurd, zero, inverted, and truncated
    // parameters; the grid must stay internally consistent and in bounds.
    let mut t = term(4, 2);
    let battery: &[&[u8]] = &[
        b"\x1b[65535;65535r",
        b"\x1b[0;0r",
        b"\x1b[9;2r", // inverted
        b"\x1b[65535S",
        b"\x1b[65535T",
        b"\x1b[65535L",
        b"\x1b[65535M",
        b"\x1b[65535@",
        b"\x1b[65535P",
        b"\x1b[65535X",
        b"\x1b[65535K",
        b"\x1b[0S",
        b"\x1b[0T",
        b"\x1b[0L",
        b"\x1b[0M",
        b"\x1b[0@",
        b"\x1b[0P",
        b"\x1b[0X",
        b"\x1b[;;;;r", // empty params
        b"\x1b[;S",
        b"\x1b[999999999S", // overflows u16 in the wire format
        b"\x1b[-1S",        // not valid VT; vte drops the sign
        b"\x1b[r",
        b"\x1bM",
        b"\x1bD",
        b"\x1b[?7l",
        b"\x1b[?7h",
    ];
    for seq in battery {
        t.advance(seq);
        let snap = t.snapshot();
        assert_eq!(snap.cols, 4, "dimensions intact after {seq:?}");
        assert_eq!(snap.rows, 2);
        assert!(snap.cursor.col < 4, "cursor in bounds after {seq:?}");
        assert!(snap.cursor.row < 2, "cursor in bounds after {seq:?}");
        // Reading every cell proves the backing store is still the right length.
        for r in 0..snap.rows {
            for c in 0..snap.cols {
                assert!(snap.cell(c, r).is_some(), "cell({c},{r}) after {seq:?}");
            }
        }
    }

    // The same battery against a 1x1 grid, where every off-by-one is fatal.
    let mut tiny = term(1, 1);
    for seq in battery {
        tiny.advance(seq);
        tiny.advance(b"x");
    }
    assert_eq!(tiny.snapshot().cols, 1);
}

#[test]
fn extreme_parameters_survive_a_region_and_a_resize() {
    // R8.1, the stateful half: absurd params while a region is active, interleaved
    // with resizes that invalidate it.
    let mut t = term(6, 4);
    t.advance(b"\x1b[2;3r");
    for _ in 0..3 {
        t.advance(b"\x1b[65535S\x1b[65535L\x1b[65535M\x1b[65535@");
        t.resize(2, 2);
        t.advance(b"\x1b[65535T\x1b[65535P\x1b[65535X");
        t.resize(9, 5);
        t.advance(b"\x1b[3;4r\x1bM\x1bD");
    }
    let snap = t.snapshot();
    assert_eq!((snap.cols, snap.rows), (9, 5));
    assert!(snap.cursor.col < 9 && snap.cursor.row < 5);
}

#[test]
fn still_unimplemented_sequences_remain_harmless_no_ops() {
    // R8.3. Sequences deliberately out of scope (Known Gaps G-D/G-E) must stay
    // inert — and critically, the `>`/`<`/`=` private-marker guard must still
    // intercept BEFORE the new arms, or `CSI > 4 ; 2 m`-style sequences would now
    // be misparsed as real commands.
    let mut t = term(8, 3);
    t.advance(b"abcdefg\x1b[1;3H");
    // Compare full cells, not just `.ch`. The `CSI > 4 ; 2 m` case exists precisely
    // because a private-marker CSI was once misparsed as SGR 4;2 and latched
    // UNDERLINE + DIM — a change a glyph-only comparison structurally cannot see.
    let cells_of = |t: &Terminal| t.snapshot().cells().to_vec();
    let before = (cells_of(&t), t.snapshot().cursor);

    for seq in [
        b"\x1b[?25h".as_slice(), // DECTCEM — G-D
        b"\x1b[?25l".as_slice(),
        b"\x1b7".as_slice(), // DECSC — G-E
        b"\x1b8".as_slice(), // DECRC
        b"\x1b[s".as_slice(),
        b"\x1b[u".as_slice(),
        b"\x1b[>4;2m".as_slice(), // XTMODKEYS — the private-marker guard
        b"\x1b[<1S".as_slice(),   // a private-marker form of a NEW arm
        b"\x1b[=2K".as_slice(),
        b"\x1b[>3r".as_slice(),
    ] {
        t.advance(seq);
        assert_eq!(
            (cells_of(&t), t.snapshot().cursor),
            before,
            "{seq:?} must be a no-op (cells compared in full, attributes included)"
        );
    }

    // `\x1b[>3r` cannot fail via the loop above (with the guard removed it would
    // become an inverted region and be rejected anyway), so pin the guard directly:
    // a private-marker DECSTBM must not reach `set_scroll_region` at all.
    let mut t = term(8, 6);
    t.advance(b"\x1b[>2;4r");
    assert_eq!(
        t.scroll_region(),
        (0, 5),
        "a `>`-marked DECSTBM must be ignored, not set as a region"
    );
    // And the positive control: the same sequence without the marker DOES apply.
    t.advance(b"\x1b[2;4r");
    assert_eq!(t.scroll_region(), (1, 3), "unmarked DECSTBM still works");
}

// --- Mouse tracking modes (feature: alt-screen-scroll) --------------------------

#[test]
fn mouse_tracking_modes_are_parsed_and_cleared() {
    use term_core::{MouseEncoding, MouseProtocol};
    let mut t = term(10, 3);
    assert_eq!(t.mouse_protocol(), MouseProtocol::Off, "off by default");
    assert_eq!(
        t.mouse_encoding(),
        MouseEncoding::Legacy,
        "legacy by default"
    );

    for (seq, want) in [
        (b"\x1b[?9h".as_slice(), MouseProtocol::X10),
        (b"\x1b[?1000h".as_slice(), MouseProtocol::Normal),
        (b"\x1b[?1002h".as_slice(), MouseProtocol::ButtonEvent),
        (b"\x1b[?1003h".as_slice(), MouseProtocol::AnyEvent),
    ] {
        let mut t = term(10, 3);
        t.advance(seq);
        assert_eq!(t.mouse_protocol(), want, "{seq:?}");
        assert!(t.mouse_protocol().is_on());
    }

    t.advance(b"\x1b[?1006h");
    assert_eq!(t.mouse_encoding(), MouseEncoding::Sgr, "?1006 selects SGR");
    t.advance(b"\x1b[?1006l");
    assert_eq!(t.mouse_encoding(), MouseEncoding::Legacy, "?1006l reverts");

    // Each mode is INDEPENDENT, as in xterm: disabling the top one falls back to
    // the ones still enabled. Collapsing to Off here killed the wheel mid-session.
    t.advance(b"\x1b[?1000h\x1b[?1002h\x1b[?1003h");
    assert_eq!(t.mouse_protocol(), MouseProtocol::AnyEvent);
    t.advance(b"\x1b[?1003l");
    assert_eq!(
        t.mouse_protocol(),
        MouseProtocol::ButtonEvent,
        "?1003l leaves ?1000/?1002 on"
    );
    t.advance(b"\x1b[?1002l");
    assert_eq!(t.mouse_protocol(), MouseProtocol::Normal, "?1000 still on");
    t.advance(b"\x1b[?1000l");
    assert_eq!(
        t.mouse_protocol(),
        MouseProtocol::Off,
        "now everything is off"
    );
}

#[test]
fn ris_clears_mouse_tracking() {
    // A TUI killed mid-run leaves tracking on; without this the wheel would keep
    // being reported to a dead app instead of scrolling scrollback.
    use term_core::{MouseEncoding, MouseProtocol};
    let mut t = term(10, 3);
    t.advance(b"\x1b[?1003h\x1b[?1006h");
    assert!(t.mouse_protocol().is_on());
    t.advance(b"\x1bc");
    assert_eq!(t.mouse_protocol(), MouseProtocol::Off);
    assert_eq!(t.mouse_encoding(), MouseEncoding::Legacy);
}

#[test]
fn claude_codes_real_mode_set_turns_tracking_on() {
    // The measured sequence Claude Code emits at startup. This is the whole reason
    // the wheel was dead inside it: it asks for mouse events, and we reported none.
    use term_core::{MouseEncoding, MouseProtocol};
    let mut t = term(65, 31);
    t.advance(b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h");
    assert!(t.alt_screen());
    assert_eq!(t.mouse_protocol(), MouseProtocol::AnyEvent);
    assert_eq!(t.mouse_encoding(), MouseEncoding::Sgr);
}

#[test]
fn a_batched_private_mode_applies_every_parameter() {
    // `CSI ? Pm ; Pm ; ... h` is legal and apps do batch it. Reading only the first
    // parameter silently dropped the rest — and losing `?1006` downgraded reports to
    // the legacy encoding and its 223-column ceiling.
    use term_core::{MouseEncoding, MouseProtocol};
    let mut t = term(65, 31);
    t.advance(b"\x1b[?1000;1002;1003;1006h");
    assert_eq!(
        t.mouse_protocol(),
        MouseProtocol::AnyEvent,
        "all modes applied"
    );
    assert_eq!(t.mouse_encoding(), MouseEncoding::Sgr, "?1006 not dropped");

    // The same batching turns them back off.
    t.advance(b"\x1b[?1000;1002;1003;1006l");
    assert_eq!(t.mouse_protocol(), MouseProtocol::Off);
    assert_eq!(t.mouse_encoding(), MouseEncoding::Legacy);

    // And it works for the non-mouse modes sharing this arm.
    let mut t = term(65, 31);
    t.advance(b"\x1b[?1049;2004;7h");
    assert!(t.alt_screen(), "?1049 applied");
    assert!(t.bracketed_paste(), "?2004 applied");
    assert!(t.autowrap(), "?7 applied");
    t.advance(b"\x1b[?2004;7l");
    assert!(!t.bracketed_paste());
    assert!(!t.autowrap());
    assert!(t.alt_screen(), "untouched modes stay put");
}
