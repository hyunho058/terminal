//! The font set — an ordered N-font fallback chain, memory-mapped (D1/D3/D4).
//!
//! The renderer owns exactly one [`FontSet`]. `faces[0]` is the primary Latin
//! monospace face; every later face is a fallback, consulted in order when an
//! earlier face's charmap does not cover a character. This module only *opens*
//! the chain — per-character face resolution is a separate concern (D2).
//!
//! Two properties are load-bearing:
//!
//! * **mmap, not `std::fs::read` (D1/R1.2).** `AppleSDGothicNeo.ttc` measures
//!   55,373,848 bytes — 26× Menlo. Reading it eagerly would put a 55 MB blocking
//!   read on the startup path and keep 55 MB resident for *every* user, including
//!   those who never type a non-Latin character, in a project whose budget is
//!   `input→screen ≤ 8 ms`. A mapping pages in only what is touched, and swash's
//!   `FontRef::from_index(&[u8], _)` accepts any slice, so it costs no copy.
//! * **The TTC face index is probed, never assumed (D3/R1.3).** A `.ttc` is a
//!   collection of faces; the one covering Hangul is not guaranteed to be index 0
//!   and the ordering can change across OS updates. We ask the charmap instead.
//!
//! Failing to open a *fallback* is not fatal: the set stays primary-only and says
//! so via [`FontSet::is_degraded`] (D4/R1.6). Only a total *primary* failure is a
//! [`RendererError::NoFont`] (R1.7) — a terminal that will not launch because one
//! font moved is a worse regression than tofu.

use std::fs::File;
use std::sync::Arc;

use memmap2::Mmap;
use swash::{FontDataRef, FontRef};

use crate::{RendererError, Result};

/// Candidate macOS monospace fonts for the **primary** face (macOS-only — SD1).
/// Family→path resolution is a later enhancement; we use the first that loads.
pub(crate) const FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Menlo.ttc",
    "/System/Library/Fonts/SFNSMono.ttf",
];

/// Candidate **fallback** faces with Hangul coverage (D1). Two entries because
/// Apple has relocated fonts into `Supplemental/` before, so a single hardcoded
/// path is one OS update away from a silent regression (R1.5).
///
/// `Supplemental/NotoSansGothic-Regular.ttf` is deliberately *not* here: despite
/// the name it is the ancient **Gothic** script (U+10330) in 6,152 bytes, not
/// Korean. The probe below would reject it anyway (D3/R1.4).
pub(crate) const FALLBACK_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
];

/// Probe character for the primary face — plain Latin capital A.
const PRIMARY_PROBE: char = 'A';
/// Probe character for the fallback chain — U+AC00 HANGUL SYLLABLE GA.
const FALLBACK_PROBE: char = '가';

/// Memory-mapped font-file bytes, shared by the shaper and the rasterizers.
///
/// `Mmap` derefs to `[u8]`, so this hands straight to `FontRef::from_index`
/// without a copy, and `Arc` keeps a single owner of the mapping (D1).
pub type FontData = Arc<Mmap>;

/// One face inside a font file: the mapped bytes plus the face index selected
/// within them (a `.ttc` holds several — D3).
#[derive(Clone, Debug)]
pub struct FontFace {
    pub data: FontData,
    pub index: usize,
}

/// The ordered fallback chain. Index 0 is the primary face; the rest are
/// fallbacks in priority order (D1).
#[derive(Clone, Debug)]
pub struct FontSet {
    faces: Vec<FontFace>,
}

impl FontSet {
    /// Open the chain: the primary face, then the first fallback candidate that
    /// covers Hangul.
    ///
    /// Returns [`RendererError::NoFont`] **only** when no primary candidate is
    /// usable (R1.7). A fallback that cannot be opened logs a warning and leaves
    /// the set [degraded](FontSet::is_degraded) (D4/R1.6).
    pub fn load() -> Result<FontSet> {
        let primary = load_candidate(FONT_CANDIDATES, PRIMARY_PROBE)
            .ok_or_else(|| RendererError::NoFont(FONT_CANDIDATES.join(", ")))?;
        let mut faces = vec![primary];

        // D4/R1.6: a missing fallback degrades rendering; it never blocks startup.
        match load_candidate(FALLBACK_CANDIDATES, FALLBACK_PROBE) {
            Some(face) => faces.push(face),
            None => tracing::warn!(
                candidates = FALLBACK_CANDIDATES.join(", "),
                "no fallback font with Hangul coverage could be opened; \
                 running degraded — non-Latin text will draw .notdef (D4/R1.6)"
            ),
        }

        Ok(FontSet { faces })
    }

    /// True when no fallback face loaded, i.e. the chain is primary-only (D4).
    /// Callers use this to keep drawing `.notdef` for uncovered characters, so a
    /// missing font stays *visible* instead of failing silently (D5).
    pub fn is_degraded(&self) -> bool {
        self.faces.len() == 1
    }

    /// The chain in priority order; `faces()[0]` is the primary face.
    pub fn faces(&self) -> &[FontFace] {
        &self.faces
    }
}

/// Try each candidate path in order and return the first that both maps *and*
/// has a face covering `probe`.
///
/// A candidate whose faces all miss the probe is rejected outright and the search
/// moves on — that is what keeps a same-named-but-wrong font (the ancient-Gothic
/// `NotoSansGothic`) out of the chain (D3/R1.4).
fn load_candidate(candidates: &[&str], probe: char) -> Option<FontFace> {
    for path in candidates {
        let Some(data) = map_font(path) else {
            continue;
        };
        match probe_face(&data, probe) {
            Some(index) => {
                tracing::debug!(path = *path, index, "font face selected");
                return Some(FontFace { data, index });
            }
            None => tracing::debug!(
                path = *path,
                "no face covers the probe character; rejecting candidate (R1.4)"
            ),
        }
    }
    None
}

/// Memory-map a font file (D1/R1.2). **The only `unsafe` in this crate.**
fn map_font(path: &str) -> Option<FontData> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) => {
            tracing::debug!(path, %err, "font candidate could not be opened");
            return None;
        }
    };

    // SAFETY: `Mmap::map` is unsafe because the mapping aliases the file's bytes —
    // if the file is replaced or truncated while it stays mapped, reads through it
    // fault (SIGBUS) and the `&[u8]` handed to swash could observe torn data. We
    // assume macOS *system* font files are not replaced while this process runs
    // (an OS update stages a new system volume and requires a reboot). Recorded as
    // an accepted risk in the spec's Known Gaps.
    let mapped = unsafe { Mmap::map(&file) };

    match mapped {
        Ok(mapped) => Some(Arc::new(mapped)),
        Err(err) => {
            tracing::debug!(path, %err, "font candidate could not be mapped");
            None
        }
    }
}

/// The index of the first face in a (possibly `.ttc`) font file whose charmap
/// maps `probe` to a non-zero glyph id, or `None` if no face does (D3/R1.3).
fn probe_face(data: &[u8], probe: char) -> Option<usize> {
    let count = FontDataRef::new(data)?.len();
    first_covering(count, |index| {
        FontRef::from_index(data, index).is_some_and(|font| font.charmap().map(probe) != 0)
    })
}

/// The first of `count` face indices satisfying `covers`.
///
/// Split out from [`probe_face`] so the selection rule is unit-testable with a
/// stub — no font file, no GPU (mirrors D2's injectable `covers`).
fn first_covering(count: usize, covers: impl Fn(usize) -> bool) -> Option<usize> {
    (0..count).find(|&index| covers(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_covering_picks_the_first_match() {
        // D3/R1.3: not index 0 — the probe finds whichever face actually covers.
        assert_eq!(first_covering(4, |i| i >= 2), Some(2));
        assert_eq!(first_covering(4, |i| i == 0), Some(0));
    }

    #[test]
    fn first_covering_rejects_a_candidate_that_covers_nothing() {
        // R1.4: no face covers the probe → the whole candidate is rejected.
        assert_eq!(first_covering(8, |_| false), None);
        assert_eq!(first_covering(0, |_| true), None);
    }

    #[test]
    fn probe_face_rejects_non_font_bytes() {
        assert_eq!(probe_face(&[0, 1, 2, 3], PRIMARY_PROBE), None);
    }

    #[test]
    fn map_font_returns_none_for_a_missing_path() {
        assert!(map_font("/System/Library/Fonts/definitely-not-a-font.ttc").is_none());
    }

    #[test]
    fn candidate_lists_exclude_the_ancient_gothic_font() {
        // D3 side effect: "Gothic" in the name does not mean Korean.
        assert!(!FALLBACK_CANDIDATES
            .iter()
            .any(|p| p.contains("NotoSansGothic")));
    }

    /// A real [`FontData`] mapping that needs no system font: this crate's own
    /// manifest. The bytes are never handed to swash — only the chain *shape*
    /// (how many faces) is under test here.
    fn dummy_face() -> FontFace {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        FontFace {
            data: map_font(path).expect("the crate manifest is mappable"),
            index: 0,
        }
    }

    #[test]
    fn degraded_means_no_fallback_face() {
        // D4/R1.6: primary-only == degraded; one fallback is enough to leave it.
        let primary_only = FontSet {
            faces: vec![dummy_face()],
        };
        assert!(primary_only.is_degraded());
        let with_fallback = FontSet {
            faces: vec![dummy_face(), dummy_face()],
        };
        assert!(!with_fallback.is_degraded());
        assert_eq!(with_fallback.faces().len(), 2);
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn loads_primary_and_hangul_fallback() {
        // R1.1: an ordered chain — primary first, Hangul face second, not degraded.
        let set = FontSet::load().expect("a primary system font");
        assert!(!set.is_degraded(), "a Hangul fallback should be available");
        assert_eq!(set.faces().len(), 2);
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn each_face_covers_its_probe() {
        // R1.3: the selected `FontFace.index` is a face that really covers the probe.
        let set = FontSet::load().expect("a primary system font");
        let covers = |face: &FontFace, ch: char| {
            FontRef::from_index(&face.data, face.index)
                .expect("a valid face at the probed index")
                .charmap()
                .map(ch)
                != 0
        };
        let faces = set.faces();
        assert!(covers(&faces[0], PRIMARY_PROBE));
        assert!(covers(&faces[1], FALLBACK_PROBE));
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn ancient_gothic_font_fails_the_hangul_probe() {
        // R1.4, made concrete: NotoSansGothic maps U+10330, not U+AC00.
        let path = "/System/Library/Fonts/Supplemental/NotoSansGothic-Regular.ttf";
        let Some(data) = map_font(path) else {
            eprintln!("skipping: {path} not present");
            return;
        };
        assert_eq!(probe_face(&data, FALLBACK_PROBE), None);
        assert!(probe_face(&data, '\u{10330}').is_some());
    }
}
