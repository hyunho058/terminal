//! Recovering a Cmd chord that the Korean input method consumed mid-composition
//! (korean-input-chords D7, D8, D11).
//!
//! With a syllable still marked, the 2-Set Korean IM answers a Cmd keyDown by committing
//! the syllable and then calling `doCommandBySelector:`. winit turns the commit into
//! `Ime::Commit`, enters its `Committed` state, and `do_command_by_selector` returns early
//! on `Committed` (winit-0.30.13 `view.rs:426-428`), so no `KeyboardInput` is ever emitted:
//! Cmd+T while `하` is composing does nothing at all. No remapping of a logical key can fix
//! an event that never arrives.
//!
//! What the app *does* get is the `Ime::Commit`, dispatched synchronously inside the very
//! `keyDown` (`app_state.rs:306-320`), at a moment when `NSApp.currentEvent` is that keyDown
//! (measured in the probe's `insertText`, `specs/korean-input-chords/composeprobe.out`; the
//! in-app confirmation is the spec's manual checklist, step 4). So the commit handler reads the
//! event into [`CmdEventFacts`] and this module decides, AppKit-free, whether that event is
//! a chord to replay and as what. The AppKit read stays a thin shim in `terminal_app.rs`.

use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::platform::scancode::PhysicalKeyExtScancode;

use crate::input::chord_key;
use crate::seam::KeyInput;

/// The parts of an `NSEvent` the replay decision reads — plain data, so the decision is
/// unit-tested without AppKit. Built for *any* current event, not only Cmd keyDowns: the
/// filtering is [`replay_decision`]'s job, and the duplicate guard reads `timestamp` of
/// whatever event is current.
#[derive(Clone, Debug, PartialEq)]
pub struct CmdEventFacts {
    pub is_key_down: bool,
    pub command: bool,
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    /// The macOS virtual key code (`NSEvent.keyCode`).
    pub key_code: u16,
    /// `NSEvent.characters` — the Command-layer string under Cmd, Roman under Korean.
    pub characters: String,
    /// `NSEvent.charactersIgnoringModifiers` — what winit builds the logical key from.
    pub characters_ignoring_modifiers: String,
    /// `NSEvent.timestamp`, the event's identity for the duplicate guard (D8).
    pub timestamp: f64,
}

impl CmdEventFacts {
    fn mods(&self) -> ModifiersState {
        let mut mods = ModifiersState::empty();
        mods.set(ModifiersState::SUPER, self.command);
        mods.set(ModifiersState::SHIFT, self.shift);
        mods.set(ModifiersState::CONTROL, self.control);
        mods.set(ModifiersState::ALT, self.option);
        mods
    }
}

/// What a commit should do about the event that caused it.
#[derive(Debug)]
pub enum Replay {
    /// Not a Cmd chord (or nothing was committed): nothing was lost, nothing to do.
    None,
    /// A Cmd chord that was lost but is deliberately not replayed; the reason is logged.
    Skip(&'static str),
    /// Dispatch this through `on_key`; the `f64` is the event's timestamp (D8).
    Key(KeyInput, f64),
}

/// AppKit's function-key characters (`NSUpArrowFunctionKey` = U+F700 … U+F8FF): what
/// `charactersIgnoringModifiers` holds for arrows, F-keys, Home/End and the like.
fn is_function_key(c: char) -> bool {
    ('\u{F700}'..='\u{F8FF}').contains(&c)
}

/// The named keys a replay reconstructs (D11) — exactly the ones a Cmd matcher reads:
/// `rename.rs`'s Cmd+←/→, and the arrows/Home/End `encode_key` swallows under Cmd. Keyed on
/// the physical code because the private-use character says nothing portable.
fn replayable_named(physical: PhysicalKey) -> Option<NamedKey> {
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    Some(match code {
        KeyCode::ArrowLeft => NamedKey::ArrowLeft,
        KeyCode::ArrowRight => NamedKey::ArrowRight,
        KeyCode::ArrowUp => NamedKey::ArrowUp,
        KeyCode::ArrowDown => NamedKey::ArrowDown,
        KeyCode::Home => NamedKey::Home,
        KeyCode::End => NamedKey::End,
        _ => return None,
    })
}

/// Decide whether the event current during an `Ime::Commit` is a Cmd chord the IM
/// swallowed, and rebuild the `KeyInput` an idle press would have produced (D7, D11).
///
/// Only a non-empty commit counts: winit emits `Ime::Commit` only when marked text existed
/// (`view.rs:411`), so a non-empty one *is* the end of a composition (D8).
pub fn replay_decision(facts: Option<&CmdEventFacts>, commit_text: &str) -> Replay {
    if commit_text.is_empty() {
        return Replay::None;
    }
    let Some(facts) = facts else {
        return Replay::None;
    };
    if !(facts.is_key_down && facts.command) {
        return Replay::None;
    }
    let physical = PhysicalKey::from_scancode(u32::from(facts.key_code));
    let mods = facts.mods();
    let ignoring = facts.characters_ignoring_modifiers.as_str();
    // Enter, Backspace, Tab, Escape, or nothing at all: no Cmd matcher reads these, and a
    // `Key::Character` built from a control char would only reach the shell as garbage.
    if ignoring.is_empty() || ignoring.chars().any(char::is_control) {
        return Replay::Skip("not a printable key");
    }
    let (key, text) = if ignoring.chars().any(is_function_key) {
        // Building `Key::Character("\u{F702}")` would match nothing and could be written to
        // the PTY by the text fallback; only the table's keys come back, as named keys.
        match replayable_named(physical) {
            Some(named) => (Key::Named(named), None),
            None => return Replay::Skip("function key outside the replay table"),
        }
    } else {
        // The same normalisation an idle press gets: winit's logical key and text are
        // `charactersIgnoringModifiers` (`event.rs:121-153`), the Command-layer string is
        // `characters`. A pass-through (Cmd+1, Cmd+Shift+]) keeps winit's idle key.
        chord_key(
            &Key::Character(ignoring.into()),
            Some(ignoring),
            Some(&facts.characters),
            mods,
        )
    };
    Replay::Key(
        KeyInput {
            key,
            text,
            mods,
            physical,
        },
        facts.timestamp,
    )
}

/// What the commit handler stores as `replayed_ts` (R5.3): the replayed event's timestamp,
/// or `None` when this commit replayed nothing — every non-empty commit resets the guard.
pub fn guard_after_commit(replay: &Replay) -> Option<f64> {
    match replay {
        Replay::Key(_, ts) => Some(*ts),
        Replay::None | Replay::Skip(_) => None,
    }
}

/// Is this `KeyboardInput` the event a commit already replayed (R5.1/R5.2)? One-shot: the
/// stored timestamp is consumed whatever the answer, so it can never shadow a later press.
///
/// Today it never answers `true` — winit does not emit that event (measured). It exists so
/// a winit that starts forwarding the key after a commit cannot run Cmd+T twice.
pub fn take_duplicate(replayed_ts: &mut Option<f64>, current_ts: Option<f64>) -> bool {
    let replayed = replayed_ts.take();
    replayed.is_some() && replayed == current_ts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{font_action, monitor_jump, tab_action, FontAction, TabAction};

    fn facts(key_code: u16, characters: &str, ignoring: &str) -> CmdEventFacts {
        CmdEventFacts {
            is_key_down: true,
            command: true,
            shift: false,
            control: false,
            option: false,
            key_code,
            characters: characters.into(),
            characters_ignoring_modifiers: ignoring.into(),
            timestamp: 42.5,
        }
    }

    fn replayed(f: &CmdEventFacts) -> (KeyInput, f64) {
        match replay_decision(Some(f), "하") {
            Replay::Key(input, ts) => (input, ts),
            other => panic!("expected a replay, got {other:?}"),
        }
    }

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    // R3.1 — measured: composing `하`, Cmd+Shift+M reports `m` / `ㅡ`, key code 46.
    #[test]
    fn a_korean_cmd_letter_is_replayed_as_the_idle_chord() {
        let f = CmdEventFacts {
            shift: true,
            ..facts(46, "m", "ㅡ")
        };
        let (input, ts) = replayed(&f);
        assert_eq!(input.key, ch("M"));
        assert_eq!(input.text.as_deref(), Some("M"));
        assert_eq!(input.mods, ModifiersState::SUPER | ModifiersState::SHIFT);
        assert_eq!(input.physical, PhysicalKey::Code(KeyCode::KeyM));
        assert_eq!(ts, 42.5);
    }

    // The modifier flags map one to one; nothing is dropped or invented.
    #[test]
    fn every_modifier_flag_reaches_the_replayed_input() {
        let f = CmdEventFacts {
            control: true,
            option: true,
            ..facts(17, "t", "ㅅ")
        };
        assert_eq!(
            replayed(&f).0.mods,
            ModifiersState::SUPER | ModifiersState::CONTROL | ModifiersState::ALT
        );
    }

    // R3.2 — not a Cmd keyDown: nothing was lost.
    #[test]
    fn only_a_cmd_key_down_is_replayed() {
        let key_up = CmdEventFacts {
            is_key_down: false,
            ..facts(17, "t", "ㅅ")
        };
        let no_cmd = CmdEventFacts {
            command: false,
            ..facts(17, "t", "ㅅ")
        };
        assert!(matches!(replay_decision(Some(&key_up), "하"), Replay::None));
        assert!(matches!(replay_decision(Some(&no_cmd), "하"), Replay::None));
        assert!(matches!(replay_decision(None, "하"), Replay::None));
    }

    // R3.3 — an empty commit is not the end of a composition.
    #[test]
    fn an_empty_commit_replays_nothing() {
        assert!(matches!(
            replay_decision(Some(&facts(17, "t", "ㅅ")), ""),
            Replay::None
        ));
    }

    // R3.4 — table function keys come back as their named keys, with no text.
    #[test]
    fn table_function_keys_are_replayed_as_named_keys() {
        for (code, c, named) in [
            (123, '\u{F702}', NamedKey::ArrowLeft),
            (124, '\u{F703}', NamedKey::ArrowRight),
            (126, '\u{F700}', NamedKey::ArrowUp),
            (125, '\u{F701}', NamedKey::ArrowDown),
            (115, '\u{F729}', NamedKey::Home),
            (119, '\u{F72B}', NamedKey::End),
        ] {
            let s = c.to_string();
            let (input, _) = replayed(&facts(code, &s, &s));
            assert_eq!(input.key, Key::Named(named), "{named:?}");
            assert_eq!(input.text, None, "{named:?}");
        }
    }

    // R3.5 — function keys outside the table are skipped, never built as characters.
    #[test]
    fn function_keys_outside_the_table_are_skipped() {
        for (code, c) in [(122, '\u{F704}'), (116, '\u{F72C}'), (117, '\u{F728}')] {
            let s = c.to_string();
            assert!(
                matches!(
                    replay_decision(Some(&facts(code, &s, &s)), "하"),
                    Replay::Skip(_)
                ),
                "{c:?}"
            );
        }
    }

    // R3.6 — the spec's fixture: a non-ASCII character `chord_key` cannot substitute is
    // replayed unchanged, and a physical matcher still recognises it. (Not a measured event;
    // the measured Korean shape is the next test.)
    #[test]
    fn an_option_altered_character_is_replayed_as_winit_builds_it() {
        let f = CmdEventFacts {
            option: true,
            ..facts(18, "¡", "¡")
        };
        let (input, _) = replayed(&f);
        assert_eq!(input.key, ch("¡"));
        assert_eq!(input.text.as_deref(), Some("¡"));
        assert_eq!(input.physical, PhysicalKey::Code(KeyCode::Digit1));
        assert_eq!(monitor_jump(&input), Some(0));
    }

    // R3.6, measured shape (`optprobe.out`): under 2-Set Korean Cmd+Opt+1 reports `1` / `1`,
    // so the replay is the plain digit with alt held, and the ai-monitor jump still fires.
    #[test]
    fn a_measured_korean_cmd_opt_digit_reaches_the_section_jump() {
        let f = CmdEventFacts {
            option: true,
            ..facts(18, "1", "1")
        };
        let (input, _) = replayed(&f);
        assert_eq!(input.key, ch("1"));
        assert_eq!(input.mods, ModifiersState::SUPER | ModifiersState::ALT);
        assert_eq!(monitor_jump(&input), Some(0));
        assert_eq!(
            tab_action(&input.key, &input.mods),
            None,
            "alt keeps it off the tab jump"
        );
    }

    // R3.7 — other non-printables are skipped.
    #[test]
    fn non_printables_are_skipped() {
        for (code, s) in [
            (36, "\r"),
            (51, "\u{7f}"),
            (48, "\t"),
            (53, "\u{1b}"),
            (0, ""),
        ] {
            assert!(
                matches!(
                    replay_decision(Some(&facts(code, s, s)), "하"),
                    Replay::Skip(_)
                ),
                "{s:?}"
            );
        }
    }

    // R3.8 — measured: composing `하`, Cmd+1 reports `1`/`1` and Cmd+Shift+] `]`/`}`.
    // `chord_key` passes both through, so the replay keeps winit's idle key and text.
    #[test]
    fn a_passed_through_printable_chord_keeps_winits_idle_key() {
        let (one, _) = replayed(&facts(18, "1", "1"));
        assert_eq!(one.key, ch("1"));
        assert_eq!(one.text.as_deref(), Some("1"));
        assert_eq!(one.physical, PhysicalKey::Code(KeyCode::Digit1));
        assert_eq!(tab_action(&one.key, &one.mods), Some(TabAction::Jump(0)));

        let bracket = CmdEventFacts {
            shift: true,
            ..facts(30, "]", "}")
        };
        let (next, _) = replayed(&bracket);
        assert_eq!(next.key, ch("}"));
        assert_eq!(next.text.as_deref(), Some("}"));
        assert_eq!(tab_action(&next.key, &next.mods), Some(TabAction::Next));
    }

    // terminal-font-size R3.4 — mid-composition replay preserves `=`, `+`, `-`, `0` so
    // `font_action` matches `Increase`, `Decrease`, and `Reset` when composing Hangul.
    #[test]
    fn r3_4_mid_composition_replay_preserves_font_zoom_chords() {
        let (equal, _) = replayed(&facts(24, "=", "="));
        assert_eq!(equal.key, ch("="));
        assert_eq!(equal.text.as_deref(), Some("="));
        assert_eq!(
            font_action(&equal.key, &equal.mods),
            Some(FontAction::Increase)
        );

        let plus_facts = CmdEventFacts {
            shift: true,
            ..facts(24, "=", "+")
        };
        let (plus, _) = replayed(&plus_facts);
        assert_eq!(plus.key, ch("+"));
        assert_eq!(plus.text.as_deref(), Some("+"));
        assert_eq!(
            font_action(&plus.key, &plus.mods),
            Some(FontAction::Increase)
        );

        let (minus, _) = replayed(&facts(27, "-", "-"));
        assert_eq!(minus.key, ch("-"));
        assert_eq!(minus.text.as_deref(), Some("-"));
        assert_eq!(
            font_action(&minus.key, &minus.mods),
            Some(FontAction::Decrease)
        );

        let (zero, _) = replayed(&facts(29, "0", "0"));
        assert_eq!(zero.key, ch("0"));
        assert_eq!(zero.text.as_deref(), Some("0"));
        assert_eq!(
            font_action(&zero.key, &zero.mods),
            Some(FontAction::Reset)
        );
    }


    // R5.3 — every non-empty commit resets the guard to what it replayed.
    #[test]
    fn the_guard_after_a_commit_is_the_replayed_timestamp_or_nothing() {
        assert_eq!(
            guard_after_commit(&replay_decision(Some(&facts(17, "t", "ㅅ")), "하")),
            Some(42.5)
        );
        assert_eq!(guard_after_commit(&Replay::Skip("x")), None);
        assert_eq!(guard_after_commit(&Replay::None), None);
    }

    // R5.1 — the same event again is the duplicate; the guard is consumed.
    #[test]
    fn the_replayed_event_is_dropped_once() {
        let mut guard = Some(7.0);
        assert!(take_duplicate(&mut guard, Some(7.0)));
        assert_eq!(guard, None);
        assert!(!take_duplicate(&mut guard, Some(7.0)), "one-shot");
    }

    // R5.2 — a different event (a key repeat, the next press) is handled, and still clears.
    #[test]
    fn a_different_event_is_handled_and_clears_the_guard() {
        let mut guard = Some(7.0);
        assert!(!take_duplicate(&mut guard, Some(8.0)));
        assert_eq!(guard, None);
        let mut guard = Some(7.0);
        assert!(!take_duplicate(&mut guard, None));
        assert_eq!(guard, None);
        let mut empty = None;
        assert!(
            !take_duplicate(&mut empty, None),
            "no guard, no current event"
        );
    }
}
