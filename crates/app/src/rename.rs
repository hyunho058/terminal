//! The tab-rename field (sticky-tab-title D1/D8/D8a): a Cmd+Shift+R modal prompt that
//! names the active tab, registered through the SD4 `Overlay` seam and drawn via the
//! renderer's `overlay_layer` in its centred-prompt placement (SD3).
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

use renderer::overlay_layer::{OverlayHandle, OverlayLayer, OverlayModel, Placement};
use renderer::DrawLayer;
use winit::keyboard::{Key, NamedKey};

use crate::seam::{AppCtx, Handled, KeyInput, Overlay, OverlayCommand};
use crate::session::TabId;

/// Priority above `AiMonitorOverlay`'s 100 (D8a.1). `OverlayHost::dispatch` offers keys to
/// hidden overlays too — by design, so an overlay can consume the chord that opens it —
/// so an open field must be *ahead* of the monitor or Cmd+Shift+M would open the log pane
/// over it.
const PRIORITY: i32 = 200;

/// The caret drawn after the text. A block, so it reads as a text cursor at any font size.
const CARET: char = '▏';

/// The rename field's state. `None` = closed; the field is modal, so at most one is open
/// and it always knows which tab it is naming (D8a.3 — the card menu can target a tab
/// that is not active).
pub struct RenameOverlay {
    handle: OverlayHandle,
    open: Option<Editing>,
}

/// An in-progress edit.
struct Editing {
    /// The tab being named — by id, never index, so a roster shift cannot retarget it.
    tab: TabId,
    /// What the user has typed so far.
    text: String,
    /// The IME's in-flight composition (R2.6): shown after `text` so composing Hangul is
    /// visible, but NOT part of the value — only a committed syllable joins `text`, so
    /// cancelling mid-composition cannot leave half a jamo in a title.
    preedit: String,
}

impl RenameOverlay {
    pub fn new() -> RenameOverlay {
        RenameOverlay {
            handle: OverlayHandle::default(),
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

    /// Open the field on `tab`, prefilled with its current user title (R2.1). Prefilling
    /// means Cmd+Shift+R on a named tab shows the name you are about to replace.
    fn begin(&mut self, tab: TabId, current: Option<String>) {
        self.open = Some(Editing {
            tab,
            text: current.unwrap_or_default(),
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
    fn commit(&mut self, ctx: &mut AppCtx) {
        let Some(editing) = self.open.take() else {
            return;
        };
        ctx.set_user_title(editing.tab, Some(editing.text));
        self.refresh_model();
    }

    /// Push the current visibility + rendered lines into the draw layer.
    fn refresh_model(&self) {
        let lines = match &self.open {
            Some(editing) => vec![
                "Rename tab  —  Enter to save, Esc to cancel, empty to reset".to_string(),
                format!("{}{}{CARET}", editing.text, editing.preedit),
            ],
            None => Vec::new(),
        };
        self.handle.set(OverlayModel {
            visible: self.open.is_some(),
            lines,
            placement: Placement::CenteredPrompt,
        });
    }

    /// Apply one keystroke to the open field. Returns whether the edit is finished
    /// (committed or cancelled) so the caller can close.
    ///
    /// Split from `on_key` so the whole edit loop is testable without an `AppCtx`, which
    /// needs a live PTY writer.
    fn edit(text: &mut String, input: &KeyInput) -> EditStep {
        match &input.key {
            Key::Named(NamedKey::Enter) => EditStep::Commit,
            Key::Named(NamedKey::Escape) => EditStep::Cancel,
            Key::Named(NamedKey::Backspace) => {
                text.pop();
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
                if let Some(s) = input.text.as_deref() {
                    text.extend(s.chars().filter(|c| !c.is_control()));
                }
                EditStep::Continue
            }
        }
    }
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
        match Self::edit(&mut editing.text, input) {
            EditStep::Continue => self.refresh_model(),
            EditStep::Commit => self.commit(ctx),
            EditStep::Cancel => self.cancel(),
        }
        ctx.request_redraw();
        // Consume EVERYTHING while open (D8a.1): no keystroke reaches the shell, and no
        // lower-priority overlay sees its hotkey.
        Handled::Consumed
    }

    fn layer(&self) -> Box<dyn DrawLayer> {
        Box::new(OverlayLayer::from_handle(self.handle.clone()))
    }

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
                editing
                    .text
                    .extend(text.chars().filter(|c| !c.is_control()));
                self.refresh_model();
                Handled::Consumed
            }
            // The in-flight composition is shown but not committed (R2.6).
            OverlayCommand::ImePreedit(text) => {
                let Some(editing) = self.open.as_mut() else {
                    return Handled::Passthrough;
                };
                editing.preedit = text.clone();
                self.refresh_model();
                Handled::Consumed
            }
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
        }
    }

    fn ch(c: &str) -> KeyInput {
        key(Key::Character(c.into()), Some(c), ModifiersState::empty())
    }

    fn named(n: NamedKey) -> KeyInput {
        key(Key::Named(n), None, ModifiersState::empty())
    }

    #[test]
    fn typing_accumulates_and_backspace_removes() {
        // R2.2 — the basic edit loop.
        let mut t = String::new();
        for c in ["F", "r", "o", "n"] {
            assert_eq!(RenameOverlay::edit(&mut t, &ch(c)), EditStep::Continue);
        }
        assert_eq!(t, "Fron");
        RenameOverlay::edit(&mut t, &named(NamedKey::Backspace));
        assert_eq!(t, "Fro");
    }

    #[test]
    fn backspace_on_empty_is_harmless() {
        let mut t = String::new();
        assert_eq!(
            RenameOverlay::edit(&mut t, &named(NamedKey::Backspace)),
            EditStep::Continue
        );
        assert_eq!(t, "");
    }

    #[test]
    fn enter_commits_and_escape_cancels() {
        // R2.2 / R2.4 — the two ways out, and only one of them writes a title.
        let mut t = "Frontend".to_string();
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
        let mut t = String::new();
        RenameOverlay::edit(&mut t, &ch("프"));
        RenameOverlay::edit(&mut t, &ch("론"));
        assert_eq!(t, "프론");
    }

    #[test]
    fn modifier_chords_do_not_type_into_the_field() {
        // A Cmd/Ctrl chord is a command, not typing — but winit still reports `text` for
        // it. Without the guard, Cmd+V seeds a literal "v" into the name the user is
        // about to commit.
        let mut t = "Front".to_string();
        for (m, c) in [
            (ModifiersState::SUPER, "v"),
            (ModifiersState::SUPER, "c"),
            (ModifiersState::CONTROL, "a"),
            (ModifiersState::SUPER | ModifiersState::SHIFT, "R"),
        ] {
            RenameOverlay::edit(&mut t, &key(Key::Character(c.into()), Some(c), m));
        }
        assert_eq!(t, "Front", "no chord left a glyph behind");
        // Plain Shift is ordinary typing and must still work.
        RenameOverlay::edit(
            &mut t,
            &key(Key::Character("E".into()), Some("E"), ModifiersState::SHIFT),
        );
        assert_eq!(t, "FrontE");
    }

    #[test]
    fn control_characters_never_enter_a_title() {
        // A stray Ctrl-chord that still carries text must not inject a control glyph into
        // a user-facing label.
        let mut t = "ok".to_string();
        RenameOverlay::edit(
            &mut t,
            &key(
                Key::Character("c".into()),
                Some("\u{3}"),
                ModifiersState::CONTROL,
            ),
        );
        assert_eq!(t, "ok");
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

    #[test]
    fn preedit_is_displayed_but_never_committed() {
        // R2.6 — an in-flight composition shows after the text so typing looks live, but
        // it is not part of the value: cancelling mid-composition must not leave half a
        // jamo in a title.
        let mut ov = RenameOverlay::new();
        ov.begin(TabId(1), Some("Fr".into()));
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
