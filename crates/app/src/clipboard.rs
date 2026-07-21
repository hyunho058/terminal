//! Clipboard image → path paste (image-paste D4/R3), plus the inline-image
//! presenter registration.
//!
//! `ClipboardHandler` registers a Cmd+V `InputHandler` through the SD4 seam: if
//! the pasteboard holds an image, it writes a uniquely-named PNG under the cache
//! dir and types that path into the active shell (so external AI CLIs get a real
//! file path, D4); otherwise it pastes the pasteboard's **text** straight into the
//! active shell (R3.2), consuming the key so the literal `v` isn't also typed.
//!
//! `ImagePresenter` is a passthrough overlay whose draw layer is the renderer's
//! `image_layer` — it registers the inline-image `DrawLayer` (D3) through the same
//! seam without editing `terminal_app.rs`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSDictionary, NSString};
use renderer::image_layer::ImageLayer;
use renderer::DrawLayer;
use winit::keyboard::{Key, KeyCode};

use crate::seam::{AppCtx, Handled, InputHandler, KeyInput, Overlay};

/// Cmd+V clipboard-image handler (R3).
#[derive(Default)]
pub struct ClipboardHandler {
    /// Per-process counter so same-second pastes never collide (R3.3).
    seq: AtomicU64,
}

impl ClipboardHandler {
    pub fn new() -> Self {
        Self::default()
    }
}

impl InputHandler for ClipboardHandler {
    fn on_key(&mut self, input: &KeyInput, ctx: &mut AppCtx) -> Handled {
        if !is_paste(input) {
            return Handled::Passthrough;
        }
        // An image on the pasteboard is written to a file and its path typed
        // (image-paste D4). No image → fall through to a plain text paste.
        if self.paste_image(ctx) == Handled::Consumed {
            return Handled::Consumed;
        }
        // Text on the pasteboard → write it straight into the active shell. This
        // is the ordinary "paste copied text into the input line" case (R3.2).
        if let Some(text) = read_clipboard_text() {
            if !text.is_empty() {
                let _ = ctx.writer.write_all(&normalize_paste(&text));
                return Handled::Consumed;
            }
        }
        // Nothing usable on the pasteboard, but this WAS Cmd+V — swallow it anyway so
        // the shortcut never types a stray character (the shell's `Key::Character`
        // fallback would otherwise send `v` / the IME-mapped `ㅍ`).
        Handled::Consumed
    }
}

impl ClipboardHandler {
    /// Write a clipboard image (if any) to a unique file and type its path + a
    /// space into the shell (R3.1/R3.3/D4). Returns `Consumed` when an image was
    /// written; `Passthrough` when there is no image (or the write fails) so the
    /// caller can try a text paste instead.
    fn paste_image(&self, ctx: &mut AppCtx) -> Handled {
        let Some(png) = read_clipboard_image_png() else {
            return Handled::Passthrough;
        };
        let Some(dir) = pasted_dir() else {
            return Handled::Passthrough;
        };
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(unique_name(secs, seq));
        // Never overwrite an existing file (C4).
        if path.exists() || std::fs::write(&path, &png).is_err() {
            return Handled::Passthrough;
        }
        // Type the absolute path + a space into the active shell (R3.1/D4).
        let mut bytes = path.to_string_lossy().into_owned().into_bytes();
        bytes.push(b' ');
        let _ = ctx.writer.write_all(&bytes);
        Handled::Consumed
    }
}

/// Is this Cmd+V? Detected by the **physical** V key so paste works under a
/// non-Latin IME (Korean maps V→`ㅍ`, so the logical char is not `v`), with a
/// logical-`v` fallback for platforms that report the physical key unidentified.
fn is_paste(input: &KeyInput) -> bool {
    if !input.mods.super_key() {
        return false;
    }
    input.physical == Some(KeyCode::KeyV)
        || matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("v"))
}

/// `~/.cache/mac-terminal/pasted`, created if missing (C4). `None` if `$HOME`
/// is unset or the dir can't be made.
fn pasted_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home)
        .join(".cache")
        .join("mac-terminal")
        .join("pasted");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Unique filename for a paste (timestamp + monotonic sequence → collision-free
/// even within the same second, R3.3).
fn unique_name(secs: u64, seq: u64) -> String {
    format!("paste-{secs}-{seq}.png")
}

/// Read a PNG from the general pasteboard (converting a TIFF screenshot if
/// needed). `None` when the pasteboard holds no image. Runs on the UI thread.
fn read_clipboard_image_png() -> Option<Vec<u8>> {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        if let Some(data) = pb.dataForType(NSPasteboardTypePNG) {
            return Some(data.to_vec());
        }
        // macOS screenshots land as TIFF — convert to PNG for the CLI.
        let tiff = pb.dataForType(NSPasteboardTypeTIFF)?;
        let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
        let empty = NSDictionary::new();
        let png = rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &empty)?;
        Some(png.to_vec())
    }
}

/// Write `text` to the general pasteboard as a plain string (Cmd+C copy). Clears
/// the pasteboard first, per the standard AppKit `clearContents` + `setString`
/// sequence. Runs on the UI thread. No-op for empty text.
pub fn write_clipboard_text(text: &str) {
    if text.is_empty() {
        return;
    }
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let s = NSString::from_str(text);
        pb.setString_forType(&s, NSPasteboardTypeString);
    }
}

/// Read a plain-text string from the general pasteboard. `None` when the
/// pasteboard holds no text. Runs on the UI thread.
fn read_clipboard_text() -> Option<String> {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        let s = pb.stringForType(NSPasteboardTypeString)?;
        Some(s.to_string())
    }
}

/// Turn clipboard text into the bytes written to the shell on paste. Newlines are
/// normalised to CR — what the shell's line editor treats as Enter — so a CRLF or
/// a lone LF collapses to a single CR and a pasted line never double-accepts (a
/// bare CR then an LF would accept the line and then an empty one). No
/// bracketed-paste wrapping: this terminal doesn't track DEC private mode `?2004`
/// yet, so wrapping would leak literal `ESC[200~` into programs that don't expect it.
fn normalize_paste(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").replace('\n', "\r").into_bytes()
}

/// A passthrough overlay that registers the inline-image `DrawLayer` (D3) through
/// the SD4 overlay seam. Always "visible" (images draw whenever present) and never
/// consumes input.
#[derive(Default)]
pub struct ImagePresenter;

impl Overlay for ImagePresenter {
    fn priority(&self) -> i32 {
        0
    }
    fn visible(&self) -> bool {
        // Not an input-capturing overlay; only its draw layer matters.
        false
    }
    fn on_key(&mut self, _input: &KeyInput, _ctx: &mut AppCtx) -> Handled {
        Handled::Passthrough
    }
    fn layer(&self) -> Box<dyn DrawLayer> {
        Box::new(ImageLayer::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_never_collide_within_a_second() {
        let a = unique_name(1000, 0);
        let b = unique_name(1000, 1);
        assert_ne!(a, b, "same second, different seq → distinct (R3.3)");
        assert!(a.ends_with(".png"));
    }

    #[test]
    fn is_paste_matches_physical_v_under_non_latin_ime() {
        use winit::keyboard::ModifiersState;

        let ki = |logical: &str, mods, physical| KeyInput {
            key: Key::Character(logical.into()),
            text: Some(logical.into()),
            mods,
            physical,
        };
        // Korean 2-beolsik: Cmd+V yields the logical char `ㅍ`, but the physical key
        // is V — the regression this guards. Must still be a paste.
        assert!(
            is_paste(&ki("ㅍ", ModifiersState::SUPER, Some(KeyCode::KeyV))),
            "Cmd+V must paste even when the IME maps V→ㅍ"
        );
        // US layout: logical `v`, physical V — matches too.
        assert!(is_paste(&ki(
            "v",
            ModifiersState::SUPER,
            Some(KeyCode::KeyV)
        )));
        // Logical-`v` fallback (physical unidentified) still works.
        assert!(is_paste(&ki("v", ModifiersState::SUPER, None)));
        // Without Cmd, plain `ㅍ`/`v` types normally — not a paste.
        assert!(!is_paste(&ki(
            "ㅍ",
            ModifiersState::empty(),
            Some(KeyCode::KeyV)
        )));
        // Cmd + a different physical key (B→ㅠ) is not a paste.
        assert!(!is_paste(&ki(
            "ㅠ",
            ModifiersState::SUPER,
            Some(KeyCode::KeyB)
        )));
    }

    #[test]
    fn paste_of_plain_text_is_unchanged() {
        // The common case — pasting a command/URL with no newline goes through
        // verbatim so it lands in the input line as typed.
        assert_eq!(normalize_paste("echo hello"), b"echo hello");
        assert_eq!(normalize_paste(""), b"");
    }

    #[test]
    fn paste_normalizes_newlines_to_cr() {
        // Lone LF and CRLF both become a single CR (Enter for the line editor)…
        assert_eq!(normalize_paste("a\nb"), b"a\rb");
        assert_eq!(normalize_paste("a\r\nb"), b"a\rb");
        // …and a bare CR is preserved (no doubling).
        assert_eq!(normalize_paste("a\rb"), b"a\rb");
    }

    #[test]
    fn paste_crlf_never_double_accepts() {
        // Two CRLF-separated lines → exactly two CRs, not CR-LF-CR-LF (which would
        // insert blank accepts between the lines).
        assert_eq!(normalize_paste("a\r\n\r\nb"), b"a\r\rb");
    }

    #[test]
    fn sequential_pastes_do_not_overwrite() {
        let dir = std::env::temp_dir().join(format!("paste-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p1 = dir.join(unique_name(42, 0));
        let p2 = dir.join(unique_name(42, 1));
        std::fs::write(&p1, b"a").unwrap();
        std::fs::write(&p2, b"b").unwrap();
        assert_ne!(p1, p2);
        assert!(
            p1.exists() && p2.exists(),
            "both pastes persist, no overwrite"
        );
    }
}
