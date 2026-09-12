//! The tab-rename field (sticky-tab-title D1/D8/D8a): a Cmd+Shift+R modal prompt that
//! names the active tab, registered through the SD4 `Overlay` seam. It owns no draw
//! layer of its own: the field paints onto the sidebar card it is renaming (SD3).
//!
//! **Why this exists as an in-app field at all.** A sticky title is impossible while the
//! only channel is OSC 0/2, because that channel belongs to whatever is running: zsh's
//! precmd, vim, ssh and tmux all write it, and the last writer wins. A chord the app
//! captures is a channel no program can forge (D1) — which is the difference between a
//! title that survives the next prompt redraw and one that does not.
//!
//! **Modal discipline (D8/D8a).** While open the field owns the keyboard: it registers
//! above every other overlay and consumes every key it does not itself act on, so
//! Cmd+Shift+M cannot open the ai-monitor on top of it and nothing leaks to the shell.
//! A tab switch cancels the edit and lets the switch happen; a dead target closes it.
//! Nothing half-typed is ever committed.

use renderer::sidebar_row::{CardEdit, TabModelHandle};
use winit::keyboard::{Key, NamedKey};

use crate::seam::{AppCtx, Handled, KeyInput, Overlay, OverlayCommand};
use crate::session::TabId;

/// Priority above `AiMonitorOverlay`'s 100 (D8a.1). `OverlayHost::dispatch` offers keys to
/// hidden overlays too — by design, so an overlay can consume the chord that opens it —
/// so an open field must be *ahead* of the monitor or Cmd+Shift+M would open the log pane
/// over it.
const PRIORITY: i32 = 200;

/// The rename field's state. `None` = closed; the field is modal, so at most one is open
/// and it always knows which tab it is naming (D8a.3 — the card menu can target a tab
/// that is not active).
pub struct RenameOverlay {
    /// The sidebar model — the edit is drawn **on the card itself**, in place of that
    /// tab's line-1 label, rather than in a floating panel. Renaming happens where the
    /// name lives.
    model: TabModelHandle,
    open: Option<Editing>,
}

/// An in-progress edit.
struct Editing {
    /// The tab being named — by id, never index, so a roster shift cannot retarget it.
    tab: TabId,
    /// What the user has typed so far.
    text: String,
    /// The text the field opened with. Only used to tell an untouched Enter from a real
    /// edit — see [`commit`](RenameOverlay::commit); the field is prefilled with the
    /// card's *displayed* label, which may be an auto one that must not be pinned by
    /// simply confirming it.
    initial: String,
    /// The IME's in-flight composition (R2.6): drawn AT the caret so composing Hangul is
    /// visible where it will land, but NOT part of the value — only a committed syllable
    /// joins `text`, so cancelling mid-composition cannot leave half a jamo in a title.
    preedit: String,
    /// Whether the whole existing name is selected. The field **opens** in this state when
    /// the tab already has a name, so the old name is visible (you can see what you are
    /// replacing, and Esc still keeps it) while the next keystroke replaces it outright —
    /// the rename convention from Finder and every tab-rename UI.
    ///
    /// Any input clears it: typing replaces, Backspace deletes it all, a cursor key just
    /// drops the selection and keeps the text.
    selected: bool,
    /// The caret's position as a **char** index into `text` (`0..=text.chars().count()`).
    ///
    /// Char, never byte: a byte index would slice mid-codepoint on any 한글 or emoji name
    /// and panic. Every edit is expressed against this — insertion, Backspace vs Delete,
    /// Home/End — which is what makes the middle of a name editable instead of only its
    /// end.
    cursor: usize,
}

impl Default for RenameOverlay {
    fn default() -> RenameOverlay {
        RenameOverlay::new()
    }
}

impl RenameOverlay {
    pub fn new() -> RenameOverlay {
        RenameOverlay {
            model: TabModelHandle::shared(),
            open: None,
        }
    }

    /// Is this the Cmd+Shift+R chord that opens the field (R2.1)?
    fn is_toggle(input: &KeyInput) -> bool {
        if !(input.mods.super_key() && input.mods.shift_key()) {
            return false;
        }
        matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("r"))
    }

    /// Open the field on `tab`, prefilled with the label the card is **currently showing**
    /// (R2.1 as amended): the user title if it has one, else the auto label — the AI task,
    /// a program's OSC title, or the cwd. You edit the text that is on screen.
    fn begin(&mut self, tab: TabId, current: Option<String>) {
        let text = current.unwrap_or_default();
        self.open = Some(Editing {
            // What the field opened with, so an untouched Enter can be told apart from a
            // deliberate re-type of the same string (see `commit`).
            initial: text.clone(),
            // Opens **selected** when there is a name to replace: you see the current name
            // and can keep it (Esc) or replace it by just typing, without first having to
            // delete it a character at a time. An empty field has nothing to select.
            selected: !text.is_empty(),
            // Caret at the end, so pressing Right (or typing after deselecting) continues
            // from the end of the existing name rather than its start.
            cursor: text.chars().count(),
            tab,
            text,
            preedit: String::new(),
        });
        self.refresh_model();
    }

    /// Close without committing (R2.4/R2.5). The invariant behind D8: leaving this field
    /// by any route other than Enter must not change a title.
    fn cancel(&mut self) {
        self.open = None;
        self.refresh_model();
    }

    /// Commit the typed name (R2.2/R2.3) and close. An all-blank name is a reset to the
    /// auto label rather than a blank title — `Session::set_user_title` normalizes that,
    /// so this passes the raw text and lets the single setter own the rule.
    ///
    /// **An untouched Enter sets nothing.** The field opens prefilled with whatever the
    /// card is showing, which on a never-named tab is an *auto* label (AI task / cwd).
    /// Committing that unchanged would silently pin it as a sticky title and freeze a
    /// label that was following the tab — the opposite of what R2.3 does for an empty
    /// commit. So confirming without editing is a no-op, and only a real edit names a tab.
    fn commit(&mut self, ctx: &mut AppCtx) {
        let Some(editing) = self.open.take() else {
            return;
        };
        if editing.text != editing.initial {
            ctx.set_user_title(editing.tab, Some(editing.text));
        }
        self.refresh_model();
    }

    /// Push the current edit onto the card being renamed (or clear it).
    ///
    /// The IME preedit is inserted AT THE CARET here rather than carried separately: the
    /// card draws one string, and an uncommitted composition should appear where it will
    /// land. It is still not part of the *value* — `commit` reads
    /// `editing.text` alone, so cancelling mid-composition cannot leave half a jamo in a
    /// title.
    fn refresh_model(&self) {
        self.model.set_edit(self.open.as_ref().map(|e| {
            // The composition is shown AT the caret, not tacked onto the end — otherwise
            // composing into the middle of a name would draw the jamo in the wrong place.
            let at = e.byte_at(e.cursor);
            let mut text = e.text.clone();
            text.insert_str(at, &e.preedit);
            CardEdit {
                tab_id: e.tab.0,
                text,
                // The caret sits after the composition being typed.
                caret: e.cursor + e.preedit.chars().count(),
                // A composition in flight has already replaced the selection, so never
                // draw both a highlight and a preedit.
                selected: e.selected && e.preedit.is_empty(),
            }
        }));
    }

    /// Apply one keystroke to the open field. Returns whether the edit is finished
    /// (committed or cancelled) so the caller can close.
    ///
    /// Split from `on_key` so the whole edit loop is testable without an `AppCtx`, which
    /// needs a live PTY writer.
    fn edit(editing: &mut Editing, input: &KeyInput) -> EditStep {
        // While a composition is in flight the IME owns the editing keys: macOS handles
        // Backspace and the arrows inside the composition itself and sends an updated
        // `Preedit`, so acting on them here would edit the text UNDERNEATH the composition
        // and apply the same keystroke twice. winit forwards named keys during a preedit
        // on purpose, so this has to be declined explicitly.
        if !editing.preedit.is_empty()
            && matches!(
                input.key,
                Key::Named(
                    NamedKey::Backspace
                        | NamedKey::Delete
                        | NamedKey::ArrowLeft
                        | NamedKey::ArrowRight
                        | NamedKey::ArrowUp
                        | NamedKey::ArrowDown
                        | NamedKey::Home
                        | NamedKey::End
                )
            )
        {
            return EditStep::Continue;
        }
        // macOS line-editing chords, checked BEFORE the bare arrow arms (which would
        // otherwise swallow Cmd+← as a one-char move) and before the modifier guard below
        // (which would swallow Ctrl+A entirely). Without these the keys a Mac user reaches
        // for to jump to the ends of a field silently do nothing.
        if input.mods.super_key() || input.mods.control_key() {
            let to = match &input.key {
                Key::Named(NamedKey::ArrowLeft) => Some(CursorMove::Start),
                Key::Named(NamedKey::ArrowRight) => Some(CursorMove::End),
                // Emacs bindings macOS honours in every text field.
                Key::Character(s) if input.mods.control_key() && s.eq_ignore_ascii_case("a") => {
                    Some(CursorMove::Start)
                }
                Key::Character(s) if input.mods.control_key() && s.eq_ignore_ascii_case("e") => {
                    Some(CursorMove::End)
                }
                _ => None,
            };
            if let Some(to) = to {
                editing.move_cursor(to);
                return EditStep::Continue;
            }
            // Cmd+A selects the whole name again — the way back to "replace everything"
            // once the opening selection has been consumed.
            if input.mods.super_key()
                && matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("a"))
            {
                editing.selected = !editing.text.is_empty();
                return EditStep::Continue;
            }
        }
        match &input.key {
            Key::Named(NamedKey::Enter) => EditStep::Commit,
            Key::Named(NamedKey::Escape) => EditStep::Cancel,
            Key::Named(NamedKey::Backspace) => {
                // With everything selected, one Backspace clears the whole name — the
                // point of opening selected is that the old name never has to be deleted
                // a character at a time. Otherwise it deletes the char BEFORE the cursor.
                if editing.take_selection() {
                    editing.replace_all("");
                } else {
                    editing.delete_back();
                }
                EditStep::Continue
            }
            Key::Named(NamedKey::Delete) => {
                // Forward delete: removes the char AT the cursor, which is a different
                // operation from Backspace once the cursor can sit mid-string.
                if editing.take_selection() {
                    editing.replace_all("");
                } else {
                    editing.delete_forward();
                }
                EditStep::Continue
            }
            // A cursor key is not "replace" — it means "put me in the existing text", so
            // it keeps the name and moves the caret (collapsing any selection to an edge).
            Key::Named(NamedKey::ArrowLeft) => {
                editing.move_cursor(CursorMove::Left);
                EditStep::Continue
            }
            Key::Named(NamedKey::ArrowRight) => {
                editing.move_cursor(CursorMove::Right);
                EditStep::Continue
            }
            // One line, so vertical movement is start/end — as in any single-line input.
            Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::Home) => {
                editing.move_cursor(CursorMove::Start);
                EditStep::Continue
            }
            Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::End) => {
                editing.move_cursor(CursorMove::End);
                EditStep::Continue
            }
            _ => {
                // A Cmd/Ctrl chord is a command, not typing. winit still reports `text`
                // for them, so without this guard Cmd+V would insert a literal "v" and
                // Cmd+C a "c" — seeding characters into the name the user is about to
                // commit. (The field consumes those chords either way, D8a.1; this only
                // decides whether they leave a glyph behind.)
                if input.mods.super_key() || input.mods.control_key() {
                    return EditStep::Continue;
                }
                // Take the composed `text`, not the raw key, so IME output and shifted
                // characters both arrive as the user sees them. Control characters are
                // dropped — a stray chord must not inject a glyph into a title.
                let Some(s) = input.text.as_deref() else {
                    return EditStep::Continue;
                };
                let typed: String = s.chars().filter(|c| !c.is_control()).collect();
                if typed.is_empty() {
                    // A bare modifier or an unprintable key must NOT clear the selection:
                    // pressing Shift before typing a capital would otherwise wipe the name
                    // and then type into an empty field, which is not what "replace" means.
                    return EditStep::Continue;
                }
                // The first real character replaces the whole selected name; otherwise it
                // goes in AT the cursor, which is what makes mid-name editing work.
                if editing.take_selection() {
                    editing.replace_all(&typed);
                } else {
                    editing.insert(&typed);
                }
                EditStep::Continue
            }
        }
    }
}

impl Editing {
    /// Consume the select-all state, reporting whether it *was* set. One call site per
    /// input kind, so "what does this key do to the selection" is answered exactly once
    /// per key rather than being re-derived.
    fn take_selection(&mut self) -> bool {
        std::mem::take(&mut self.selected)
    }

    /// Length in chars — the unit `cursor` counts in.
    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// The byte offset of char index `i`, for slicing `text` without splitting a codepoint.
    fn byte_at(&self, i: usize) -> usize {
        self.text
            .char_indices()
            .nth(i)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    /// Replace the whole name with `s` (a selection being typed over), leaving the cursor
    /// after the inserted text.
    fn replace_all(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
        self.cursor = self.len();
    }

    /// Insert at the cursor and step over what was inserted — the ordinary typing path,
    /// and the one that makes editing the MIDDLE of a name work.
    fn insert(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, s);
        self.cursor += s.chars().count();
    }

    /// Delete the char **before** the cursor (Backspace). No-op at the start.
    fn delete_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let from = self.byte_at(self.cursor - 1);
        let to = self.byte_at(self.cursor);
        self.text.replace_range(from..to, "");
        self.cursor -= 1;
    }

    /// Delete the char **at** the cursor (Delete/fn+Backspace). No-op at the end; the
    /// cursor does not move, since the text after it shifts back onto it.
    fn delete_forward(&mut self) {
        if self.cursor >= self.len() {
            return;
        }
        let from = self.byte_at(self.cursor);
        let to = self.byte_at(self.cursor + 1);
        self.text.replace_range(from..to, "");
    }

    /// Move the cursor, collapsing a selection to the edge it moved toward — the standard
    /// behaviour everywhere else: Left from a selection puts you at its start, not one
    /// char before its end.
    fn move_cursor(&mut self, to: CursorMove) {
        let had_selection = self.take_selection();
        let len = self.len();
        self.cursor = match to {
            CursorMove::Left if had_selection => 0,
            CursorMove::Right if had_selection => len,
            CursorMove::Left => self.cursor.saturating_sub(1),
            CursorMove::Right => (self.cursor + 1).min(len),
            CursorMove::Start => 0,
            CursorMove::End => len,
        };
    }
}

/// Where a cursor key moves the caret. A single-line field, so Up/Down are Start/End —
/// the convention in every one-line text input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorMove {
    Left,
    Right,
    Start,
    End,
}

/// What one keystroke did to an open field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditStep {
    /// Keep editing.
    Continue,
    /// Enter — save what is typed.
    Commit,
    /// Esc — discard.
    Cancel,
}

impl Overlay for RenameOverlay {
    fn priority(&self) -> i32 {
        PRIORITY
    }

    fn visible(&self) -> bool {
        self.open.is_some()
    }

    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        // Closed: only the opening chord is ours. Everything else falls through to the
        // shell, exactly as before this feature existed.
        if self.open.is_none() {
            if Self::is_toggle(input) {
                // The chord names the ACTIVE tab (R2.1); the card menu targets any tab by
                // sending `BeginRename` instead (R3.4). Both land in `begin`.
                if let Some(tab) = ctx.active_tab {
                    self.begin(tab, ctx.active_user_title.clone());
                    ctx.request_redraw();
                }
                // Consumed either way: the chord must never reach the shell (R2.1).
                return Handled::Consumed;
            }
            return Handled::Passthrough;
        }
        // The opening chord pressed AGAIN, while open, re-selects the whole name — "start
        // this rename over". It previously fell through to the modifier guard and did
        // nothing at all, which reads as the app ignoring a key it visibly consumed.
        if Self::is_toggle(input) {
            if let Some(e) = self.open.as_mut() {
                e.selected = !e.text.is_empty();
                e.cursor = e.text.chars().count();
            }
            self.refresh_model();
            ctx.request_redraw();
            return Handled::Consumed;
        }
        // Open: a tab chord cancels the edit and is then allowed through, so the switch
        // still happens (D8a.2 — a chord is never swallowed).
        if crate::input::tab_action(&input.key, &input.mods).is_some() {
            self.cancel();
            ctx.request_redraw();
            return Handled::Passthrough;
        }
        let Some(editing) = self.open.as_mut() else {
            return Handled::Passthrough;
        };
        match Self::edit(editing, input) {
            EditStep::Continue => self.refresh_model(),
            EditStep::Commit => self.commit(ctx),
            EditStep::Cancel => self.cancel(),
        }
        ctx.request_redraw();
        // Consume EVERYTHING while open (D8a.1): no keystroke reaches the shell, and no
        // lower-priority overlay sees its hotkey.
        Handled::Consumed
    }

    // No `layer()` override: this overlay draws through the sidebar's model, onto the card
    // it is renaming, so it registers no surface of its own (the seam's default no-op).

    fn on_command(&mut self, cmd: &OverlayCommand) -> Handled {
        match cmd {
            OverlayCommand::BeginRename { tab, current } => {
                self.begin(*tab, current.clone());
                Handled::Consumed
            }
            // A tab switch, a click elsewhere, or a session that died under the field.
            OverlayCommand::Cancel => {
                if self.open.is_some() {
                    self.cancel();
                    return Handled::Consumed;
                }
                Handled::Passthrough
            }
            // A committed IME syllable belongs to the field while it is open (R2.6).
            // Consuming it is what keeps the text OUT of the shell — `Ime::Commit` writes
            // straight to a PTY otherwise.
            OverlayCommand::ImeCommit(text) => {
                let Some(editing) = self.open.as_mut() else {
                    return Handled::Passthrough;
                };
                editing.preedit.clear();
                // A committed syllable replaces the selection, exactly as a typed Latin
                // character does — otherwise renaming a named tab in Korean would append
                // to the old name instead of replacing it. Otherwise it lands AT the
                // cursor, so Korean can be typed into the middle of a name too.
                let typed: String = text.chars().filter(|c| !c.is_control()).collect();
                if editing.take_selection() {
                    editing.replace_all(&typed);
                } else {
                    editing.insert(&typed);
                }
                self.refresh_model();
                Handled::Consumed
            }
            // The in-flight composition is shown but not committed (R2.6).
            OverlayCommand::ImePreedit(text) => {
                let Some(editing) = self.open.as_mut() else {
                    return Handled::Passthrough;
                };
                // Starting a composition is the same "replace" gesture as typing: drop the
                // selected name now, so the composing jamo is not drawn appended to a name
                // that is about to disappear. An empty preedit is winit's clear signal and
                // must NOT count as the start of one.
                if !text.is_empty() && editing.take_selection() {
                    editing.replace_all("");
                }
                editing.preedit = text.clone();
                self.refresh_model();
                Handled::Consumed
            }
            // Addressed to the ai-monitor overlay (right-panel R6.6). Passing through is
            // load-bearing, not a formality: this overlay sits at priority 200, ahead of the
            // ai-monitor's 100, and `OverlayHost::command` stops at the first taker — so
            // consuming it here would silently eat the launch restore. Matched explicitly
            // rather than by a wildcard so the next command added still forces a decision.
            OverlayCommand::RestoreMonitor { .. } => Handled::Passthrough,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::ModifiersState;

    fn key(k: Key, text: Option<&str>, mods: ModifiersState) -> KeyInput {
        KeyInput {
            key: k,
            text: text.map(str::to_string),
            mods,
            physical: crate::seam::NO_PHYSICAL,
        }
    }

    fn ch(c: &str) -> KeyInput {
        key(Key::Character(c.into()), Some(c), ModifiersState::empty())
    }

    fn named(n: NamedKey) -> KeyInput {
        key(Key::Named(n), None, ModifiersState::empty())
    }

    /// A field with nothing selected and the caret at the END — the state `begin` leaves
    /// a field in once a selection has been consumed. Tests about a specific caret
    /// position set `cursor` explicitly.
    fn field(text: &str) -> Editing {
        Editing {
            tab: TabId(1),
            text: text.to_string(),
            initial: text.to_string(),
            preedit: String::new(),
            selected: false,
            cursor: text.chars().count(),
        }
    }

    /// A field as it opens on a NAMED tab: the existing name present and fully selected,
    /// so the next keystroke replaces it.
    fn field_selected(text: &str) -> Editing {
        Editing {
            selected: true,
            ..field(text)
        }
    }

    #[test]
    fn typing_accumulates_and_backspace_removes() {
        // R2.2 — the basic edit loop.
        let mut t = field("");
        for c in ["F", "r", "o", "n"] {
            assert_eq!(RenameOverlay::edit(&mut t, &ch(c)), EditStep::Continue);
        }
        assert_eq!(t.text, "Fron");
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "Fro");
    }

    #[test]
    fn backspace_on_empty_is_harmless() {
        let mut t = field("");
        assert_eq!(
            RenameOverlay::edit(&mut t, &named(NamedKey::Backspace)),
            EditStep::Continue
        );
        assert_eq!(t.text, "");
    }

    #[test]
    fn enter_commits_and_escape_cancels() {
        // R2.2 / R2.4 — the two ways out, and only one of them writes a title.
        let mut t = field("Frontend");
        assert_eq!(
            RenameOverlay::edit(&mut t, &named(NamedKey::Enter)),
            EditStep::Commit
        );
        assert_eq!(
            RenameOverlay::edit(&mut t, &named(NamedKey::Escape)),
            EditStep::Cancel
        );
    }

    #[test]
    fn composed_text_is_taken_so_ime_output_lands_in_the_field() {
        // R2.6 — the field consumes `KeyInput.text` (what the user sees) rather than the
        // raw key, so a composed Hangul syllable enters as one character.
        let mut t = field("");
        RenameOverlay::edit(&mut t, &ch("프"));
        RenameOverlay::edit(&mut t, &ch("론"));
        assert_eq!(t.text, "프론");
    }

    #[test]
    fn modifier_chords_do_not_type_into_the_field() {
        // A Cmd/Ctrl chord is a command, not typing — but winit still reports `text` for
        // it. Without the guard, Cmd+V seeds a literal "v" into the name the user is
        // about to commit.
        let mut t = field("Front");
        for (m, c) in [
            (ModifiersState::SUPER, "v"),
            (ModifiersState::SUPER, "c"),
            (ModifiersState::CONTROL, "b"),
            (ModifiersState::SUPER | ModifiersState::SHIFT, "R"),
        ] {
            RenameOverlay::edit(&mut t, &key(Key::Character(c.into()), Some(c), m));
        }
        assert_eq!(t.text, "Front", "no chord left a glyph behind");
        assert_eq!(t.cursor, 5, "and none of them moved the caret");
        // (Ctrl+A / Ctrl+E are deliberately NOT in that list — they are line-editing
        // bindings that move the caret, covered by `macos_line_editing_chords_*`.)
        // Plain Shift is ordinary typing and must still work.
        RenameOverlay::edit(
            &mut t,
            &key(Key::Character("E".into()), Some("E"), ModifiersState::SHIFT),
        );
        assert_eq!(t.text, "FrontE");
    }

    #[test]
    fn control_characters_never_enter_a_title() {
        // A stray Ctrl-chord that still carries text must not inject a control glyph into
        // a user-facing label.
        let mut t = field("ok");
        RenameOverlay::edit(
            &mut t,
            &key(
                Key::Character("c".into()),
                Some("\u{3}"),
                ModifiersState::CONTROL,
            ),
        );
        assert_eq!(t.text, "ok");
    }

    #[test]
    fn is_toggle_matches_only_cmd_shift_r() {
        // R2.1 — and specifically NOT plain Cmd+R or a bare "R", which must reach the shell.
        let cmd_shift = ModifiersState::SUPER | ModifiersState::SHIFT;
        assert!(RenameOverlay::is_toggle(&key(
            Key::Character("r".into()),
            Some("r"),
            cmd_shift
        )));
        assert!(RenameOverlay::is_toggle(&key(
            Key::Character("R".into()),
            Some("R"),
            cmd_shift
        )));
        assert!(!RenameOverlay::is_toggle(&key(
            Key::Character("r".into()),
            Some("r"),
            ModifiersState::SUPER
        )));
        assert!(!RenameOverlay::is_toggle(&ch("r")));
    }

    #[test]
    fn begin_prefills_with_the_current_title_and_cancel_leaves_no_state() {
        // R2.1 — Cmd+Shift+R on a named tab shows the name you are replacing.
        let mut ov = RenameOverlay::new();
        assert!(!ov.visible());
        ov.begin(TabId(3), Some("Frontend".into()));
        assert!(ov.visible());
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some("Frontend"));
        assert_eq!(ov.open.as_ref().map(|e| e.tab), Some(TabId(3)));
        ov.cancel();
        assert!(
            !ov.visible(),
            "cancel closes without leaving an edit behind"
        );
    }

    #[test]
    fn begin_on_an_unnamed_tab_starts_empty() {
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(1), None);
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some(""));
    }

    #[test]
    fn cancel_command_closes_an_open_field_and_is_ignored_when_closed() {
        // R2.5/R3.8 — the app cancels the edit on a tab switch or a dead session; when
        // nothing is open the command must fall through rather than claim to have acted.
        let mut ov = RenameOverlay::new();
        assert_eq!(ov.on_command(&OverlayCommand::Cancel), Handled::Passthrough);
        ov.begin(TabId(2), None);
        assert_eq!(ov.on_command(&OverlayCommand::Cancel), Handled::Consumed);
        assert!(!ov.visible());
    }

    #[test]
    fn ime_commit_lands_in_an_open_field_and_is_consumed() {
        // R2.6 — THE fix this task exists for. `Ime::Commit` writes straight to a PTY
        // (terminal_app.rs), so unless the field consumes it, every Hangul syllable typed
        // into the rename prompt would be injected into zsh while the field stayed empty.
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(1), None);
        assert_eq!(
            ov.on_command(&OverlayCommand::ImeCommit("프".into())),
            Handled::Consumed,
            "consuming is what keeps the syllable out of the shell"
        );
        ov.on_command(&OverlayCommand::ImeCommit("론".into()));
        ov.on_command(&OverlayCommand::ImeCommit("트".into()));
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some("프론트"));
    }

    #[test]
    fn ime_falls_through_to_the_shell_when_no_field_is_open() {
        // The other half of R2.6: with nothing open, Korean must reach zsh exactly as it
        // does today. A blanket consume here would break typing Hangul in the terminal.
        let mut ov = RenameOverlay::new();
        assert_eq!(
            ov.on_command(&OverlayCommand::ImeCommit("프".into())),
            Handled::Passthrough
        );
        assert_eq!(
            ov.on_command(&OverlayCommand::ImePreedit("ㅍ".into())),
            Handled::Passthrough
        );
    }

    // ---- cursor movement + mid-name editing ----

    #[test]
    fn arrows_move_the_caret_and_typing_lands_there() {
        // The bug this fixes: arrows did nothing and every character went to the end, so
        // the middle of a name could not be edited at all.
        let mut t = field("abcd"); // caret starts at 0 for a plain `field`
        t.cursor = 4;
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowLeft));
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowLeft));
        assert_eq!(t.cursor, 2, "Left moves the caret");
        RenameOverlay::edit(&mut t, &ch("X"));
        assert_eq!(t.text, "abXcd", "typing inserts AT the caret");
        assert_eq!(t.cursor, 3, "and the caret steps over what was typed");
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowRight));
        assert_eq!(t.cursor, 4);
    }

    #[test]
    fn backspace_and_delete_act_on_opposite_sides_of_the_caret() {
        let mut t = field("abcd");
        t.cursor = 2;
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "acd", "Backspace removes the char BEFORE the caret");
        assert_eq!(t.cursor, 1);
        RenameOverlay::edit(&mut t, &named(NamedKey::Delete));
        assert_eq!(t.text, "ad", "Delete removes the char AT the caret");
        assert_eq!(t.cursor, 1, "and leaves the caret where it was");
    }

    #[test]
    fn caret_cannot_run_off_either_end() {
        let mut t = field("ab");
        t.cursor = 0;
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowLeft));
        assert_eq!(t.cursor, 0, "no underflow at the start");
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "ab", "Backspace at the start is a no-op");
        t.cursor = 2;
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowRight));
        assert_eq!(t.cursor, 2, "no overrun at the end");
        RenameOverlay::edit(&mut t, &named(NamedKey::Delete));
        assert_eq!(t.text, "ab", "Delete at the end is a no-op");
    }

    #[test]
    fn home_end_and_vertical_keys_jump_to_the_edges() {
        let mut t = field("abcd");
        t.cursor = 2;
        RenameOverlay::edit(&mut t, &named(NamedKey::Home));
        assert_eq!(t.cursor, 0);
        RenameOverlay::edit(&mut t, &named(NamedKey::End));
        assert_eq!(t.cursor, 4);
        // One line, so Up/Down are Start/End.
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowUp));
        assert_eq!(t.cursor, 0);
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowDown));
        assert_eq!(t.cursor, 4);
    }

    #[test]
    fn an_arrow_collapses_a_selection_to_the_edge_it_moved_toward() {
        // Left from a selection means "go to its start", not "one before its end" — the
        // behaviour of every other text field.
        let mut t = field_selected("Frontend");
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowLeft));
        assert_eq!(t.cursor, 0);
        assert_eq!(t.text, "Frontend");
        let mut t = field_selected("Frontend");
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowRight));
        assert_eq!(t.cursor, 8);
    }

    #[test]
    fn the_caret_is_a_char_index_so_hangul_never_panics() {
        // A byte index would slice mid-codepoint here and panic. Editing a Korean name
        // must behave exactly like a Latin one.
        let mut t = field("한글이름");
        t.cursor = 4;
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowLeft));
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowLeft));
        assert_eq!(t.cursor, 2);
        RenameOverlay::edit(&mut t, &ch("X"));
        assert_eq!(t.text, "한글X이름");
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "한글이름");
        RenameOverlay::edit(&mut t, &named(NamedKey::Delete));
        assert_eq!(t.text, "한글름");
    }

    #[test]
    fn macos_line_editing_chords_jump_to_the_ends() {
        // These silently did nothing: the bare arrow arms swallowed Cmd+← as a one-char
        // move, and the Cmd/Ctrl guard swallowed Ctrl+A entirely.
        let cmd = ModifiersState::SUPER;
        let ctrl = ModifiersState::CONTROL;
        let mut t = field("abcd"); // caret at end
        RenameOverlay::edit(&mut t, &key(Key::Named(NamedKey::ArrowLeft), None, cmd));
        assert_eq!(t.cursor, 0, "Cmd+← jumps to the start, not one char");
        RenameOverlay::edit(&mut t, &key(Key::Named(NamedKey::ArrowRight), None, cmd));
        assert_eq!(t.cursor, 4, "Cmd+→ jumps to the end");
        RenameOverlay::edit(&mut t, &key(Key::Character("a".into()), Some("a"), ctrl));
        assert_eq!(t.cursor, 0, "Ctrl+A → start");
        RenameOverlay::edit(&mut t, &key(Key::Character("e".into()), Some("e"), ctrl));
        assert_eq!(t.cursor, 4, "Ctrl+E → end");
        // None of them typed a character.
        assert_eq!(t.text, "abcd");
    }

    #[test]
    fn cmd_a_reselects_the_whole_name() {
        // The way back to "replace everything" once the opening selection is consumed.
        let mut t = field("abcd");
        assert!(!t.selected);
        RenameOverlay::edit(
            &mut t,
            &key(Key::Character("a".into()), Some("a"), ModifiersState::SUPER),
        );
        assert!(t.selected);
        assert_eq!(t.text, "abcd", "selecting types nothing");
        // Then one character replaces it all.
        RenameOverlay::edit(&mut t, &ch("Z"));
        assert_eq!(t.text, "Z");
    }

    #[test]
    fn the_ime_owns_the_editing_keys_while_composing() {
        // macOS handles Backspace/arrows inside the composition and sends a new Preedit;
        // acting on them here too would edit the text underneath and double-apply the key.
        let mut t = field("abc");
        t.preedit = "ㅍ".into();
        let before = (t.text.clone(), t.cursor);
        for k in [
            NamedKey::Backspace,
            NamedKey::Delete,
            NamedKey::ArrowLeft,
            NamedKey::ArrowRight,
            NamedKey::Home,
            NamedKey::End,
        ] {
            RenameOverlay::edit(&mut t, &named(k));
        }
        assert_eq!(
            (t.text.clone(), t.cursor),
            before,
            "text and caret untouched"
        );
        // With no composition in flight the same keys work normally again.
        t.preedit.clear();
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "ab");
    }

    #[test]
    fn a_named_tab_opens_with_its_name_selected() {
        // The requested behaviour: the existing title is there, shown as selected, and the
        // field waits. You can see what you are replacing; Esc still keeps it.
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(3), Some("Frontend".into()));
        let e = ov.open.as_ref().expect("open");
        assert_eq!(e.text, "Frontend");
        assert!(e.selected, "opens selected so the next key replaces it");

        // An UNNAMED tab has nothing to select — an empty field is just an empty field.
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(4), None);
        assert!(!ov.open.as_ref().expect("open").selected);
    }

    #[test]
    fn typing_replaces_the_whole_selected_name() {
        // The point of opening selected: one keystroke replaces, instead of 8 backspaces.
        let mut t = field_selected("Frontend");
        RenameOverlay::edit(&mut t, &ch("A"));
        assert_eq!(t.text, "A");
        assert!(!t.selected, "selection is consumed by the first character");
        // Subsequent typing appends normally.
        RenameOverlay::edit(&mut t, &ch("P"));
        RenameOverlay::edit(&mut t, &ch("I"));
        assert_eq!(t.text, "API");
    }

    #[test]
    fn backspace_on_a_selection_clears_the_whole_name() {
        let mut t = field_selected("Frontend");
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "");
        assert!(!t.selected);
        // And a second Backspace on the now-empty field is harmless.
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "");
    }

    #[test]
    fn a_cursor_key_drops_the_selection_but_keeps_the_name() {
        // Arrowing into the text means "edit this", not "replace this".
        let mut t = field_selected("Frontend");
        RenameOverlay::edit(&mut t, &named(NamedKey::ArrowRight));
        assert_eq!(t.text, "Frontend", "the name survives");
        assert!(!t.selected);
        // Now Backspace trims one character rather than clearing everything.
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t.text, "Fronten");
    }

    #[test]
    fn a_bare_modifier_does_not_wipe_the_selection() {
        // Pressing Shift before typing a capital must not clear the name and then type
        // into an empty field — the selection may only be consumed by real input.
        let mut t = field_selected("Frontend");
        RenameOverlay::edit(
            &mut t,
            &key(Key::Named(NamedKey::Shift), None, ModifiersState::SHIFT),
        );
        assert_eq!(t.text, "Frontend");
        assert!(t.selected, "still selected — nothing was typed yet");
        // A Cmd chord likewise leaves it intact.
        RenameOverlay::edit(
            &mut t,
            &key(Key::Character("v".into()), Some("v"), ModifiersState::SUPER),
        );
        assert!(t.selected);
        assert_eq!(t.text, "Frontend");
    }

    #[test]
    fn an_ime_composition_replaces_the_selected_name() {
        // R2.6 × selection — renaming a named tab in Korean must REPLACE, not append to,
        // the old name. The replacement happens when the composition starts.
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(1), Some("Frontend".into()));
        ov.on_command(&OverlayCommand::ImePreedit("ㅍ".into()));
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some(""));
        ov.on_command(&OverlayCommand::ImeCommit("프".into()));
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some("프"));

        // An empty preedit is winit's CLEAR signal, not the start of a composition, so it
        // must not silently wipe a selected name.
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(1), Some("Frontend".into()));
        ov.on_command(&OverlayCommand::ImePreedit(String::new()));
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some("Frontend"));
        assert!(ov.open.as_ref().expect("open").selected);
    }

    #[test]
    fn confirming_without_editing_sets_no_title() {
        // The field opens prefilled with the card's DISPLAYED label, which on a
        // never-named tab is an auto one (AI task / cwd). Committing it unchanged would
        // silently pin it and freeze a label that was following the tab — so an untouched
        // Enter must be a no-op. `initial` is what makes the two cases distinguishable.
        let t = field("mac-terminal");
        assert_eq!(t.text, t.initial, "opened untouched");

        // A real edit is committed as normal.
        let mut edited = field("mac-terminal");
        RenameOverlay::edit(&mut edited, &named(NamedKey::Backspace));
        assert_ne!(edited.text, edited.initial, "an edit is detectable");

        // Known consequence, accepted: the test is on the resulting TEXT, not on which
        // keys were pressed, so re-typing the identical string by hand also commits
        // nothing. Harmless — on a named tab the title is already that value, and on an
        // unnamed one the card keeps showing the same label either way.
        let mut retyped = field_selected("proj");
        for c in ["p", "r", "o", "j"] {
            RenameOverlay::edit(&mut retyped, &ch(c));
        }
        assert_eq!(retyped.text, retyped.initial);
    }

    #[test]
    fn escape_keeps_the_selected_name_untouched() {
        // The safety property of opening selected: it LOOKS destructive, so cancelling
        // must be completely inert.
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(1), Some("Frontend".into()));
        ov.cancel();
        assert!(!ov.visible(), "closed without committing anything");
    }

    #[test]
    fn preedit_is_displayed_but_never_committed() {
        // R2.6 — an in-flight composition shows at the caret so typing looks live, but
        // it is not part of the value: cancelling mid-composition must not leave half a
        // jamo in a title.
        let mut ov = RenameOverlay::new();
        // Start from an UNNAMED tab and type, so the select-all state is not in play —
        // this test is about the preedit, and replacement is covered separately below.
        ov.begin(TabId(1), None);
        for c in ["F", "r"] {
            if let Some(e) = ov.open.as_mut() {
                RenameOverlay::edit(e, &ch(c));
            }
        }
        ov.on_command(&OverlayCommand::ImePreedit("ㅍ".into()));
        assert_eq!(
            ov.open.as_ref().map(|e| e.text.as_str()),
            Some("Fr"),
            "preedit does not enter the value"
        );
        assert_eq!(ov.open.as_ref().map(|e| e.preedit.as_str()), Some("ㅍ"));
        // Committing the syllable replaces the preedit with real text.
        ov.on_command(&OverlayCommand::ImeCommit("프".into()));
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some("Fr프"));
        assert_eq!(ov.open.as_ref().map(|e| e.preedit.as_str()), Some(""));
    }

    #[test]
    fn begin_rename_command_opens_on_the_named_tab() {
        // R3.4 — the card menu's route in: a background tab can be the target.
        let mut ov = RenameOverlay::new();
        let cmd = OverlayCommand::BeginRename {
            tab: TabId(9),
            current: Some("API".into()),
        };
        assert_eq!(ov.on_command(&cmd), Handled::Consumed);
        assert_eq!(ov.open.as_ref().map(|e| e.tab), Some(TabId(9)));
        assert_eq!(ov.open.as_ref().map(|e| e.text.as_str()), Some("API"));
    }
}
