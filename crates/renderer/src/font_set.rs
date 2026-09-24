//! The font set — an ordered N-font fallback chain, memory-mapped (H-D1/H-D3/H-D4).
//!
//! The renderer owns exactly one [`FontSet`]. `families()[0]` is the primary Latin
//! monospace family; every later entry is a fallback, consulted in order when an
//! earlier family's charmap does not cover a character. This module only *opens*
//! the chain — per-character face resolution is a separate concern (H-D2).
//!
//! Each chain entry is a [`FontFamily`]: a regular face plus whatever **real**
//! bold/italic faces the same file ships (D10). Both files ship more than was ever
//! opened — Menlo has Bold/Italic/BoldItalic at faces 1/2/3, and AppleSDGothicNeo
//! carries nine weights including Bold (w700) at face 6. Before this, bold was
//! always *synthesized* by emboldening the regular outline, which on Gothic
//! **erodes** 593 of 11,172 Hangul syllables rather than thickening them.
//!
//! Citations: `D<n>` is `specs/real-bold-faces/spec.md`; `H-D<n>`/`H-R<n>.<n>` is
//! the earlier `specs/hangul-ime-and-font-fallback/spec.md`.
//!
//! Two properties are load-bearing:
//!
//! * **mmap, not `std::fs::read` (H-D1/H-R1.2).** `AppleSDGothicNeo.ttc` measures
//!   55,373,848 bytes — 26× Menlo. Reading it eagerly would put a 55 MB blocking
//!   read on the startup path and keep 55 MB resident for *every* user, including
//!   those who never type a non-Latin character, in a project whose budget is
//!   `input→screen ≤ 8 ms`. A mapping pages in only what is touched, and swash's
//!   `FontRef::from_index(&[u8], _)` accepts any slice, so it costs no copy.
//! * **The TTC face index is probed, never assumed (H-D3/H-R1.3).** A `.ttc` is a
//!   collection of faces; the one covering Hangul is not guaranteed to be index 0
//!   and the ordering can change across OS updates. We ask the charmap instead.
//!
//! Failing to open a *fallback* is not fatal: the set stays primary-only and says
//! so via [`FontSet::is_degraded`] (H-D4/H-R1.6). Only a total *primary* failure is a
//! [`RendererError::NoFont`] (H-R1.7) — a terminal that will not launch because one
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

/// Candidate **fallback** faces with Hangul coverage (H-D1). Two entries because
/// Apple has relocated fonts into `Supplemental/` before, so a single hardcoded
/// path is one OS update away from a silent regression (H-R1.5).
///
/// `Supplemental/NotoSansGothic-Regular.ttf` is deliberately *not* here: despite
/// the name it is the ancient **Gothic** script (U+10330) in 6,152 bytes, not
/// Korean. The probe below would reject it anyway (H-D3/H-R1.4).
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
/// without a copy, and `Arc` keeps a single owner of the mapping (H-D1).
pub type FontData = Arc<Mmap>;

/// One face inside a font file: the mapped bytes plus the face index selected
/// within them (a `.ttc` holds several — H-D3).
#[derive(Clone, Debug)]
pub struct FontFace {
    pub data: FontData,
    pub index: usize,
}

/// Which of a family's four faces a glyph id belongs to.
///
/// This is the **slot**, not the requested style, and the distinction is the whole
/// point (D15): a character the italic face does not cover is drawn from the
/// *regular* face with a synthetic shear, so its glyph id lives in `Regular`'s
/// namespace while the cell's requested style is still italic. Keying a cache on
/// the requested style instead collides those two — measured reachable on Menlo,
/// where regular gid 407 is `Ǟ` and italic gid 407 is `ǟ`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StyleSlot {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl StyleSlot {
    /// The slot a `(bold, italic)` pair asks for, before availability is known.
    pub fn wanted(bold: bool, italic: bool) -> Self {
        match (bold, italic) {
            (false, false) => StyleSlot::Regular,
            (true, false) => StyleSlot::Bold,
            (false, true) => StyleSlot::Italic,
            (true, true) => StyleSlot::BoldItalic,
        }
    }
}

/// The outcome of asking a family for a style: which face actually supplies the
/// outline, plus the synthesis still owed on top of it.
///
/// Each axis resolves independently (D9). AppleSDGothicNeo ships nine weights and
/// **no italic face at all**, so bold-italic Hangul comes back as
/// `{ slot: Bold, synth_italic: true }` — real weight 700 outlines with a shear,
/// never the `embolden` that erodes 593 syllables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StyleResolution {
    pub slot: StyleSlot,
    pub synth_bold: bool,
    pub synth_italic: bool,
}

/// One entry of the fallback chain: a regular face plus whatever real styled
/// faces the same file happens to ship (D10).
///
/// The chain stays **one entry per fallback step**, which is what keeps
/// `GlyphKey.face`, `Atlas::resolve`'s enumerate index and `is_degraded()`
/// meaning exactly what they meant before styles existed.
#[derive(Clone, Debug)]
pub struct FontFamily {
    regular: FontFace,
    bold: Option<FontFace>,
    italic: Option<FontFace>,
    bold_italic: Option<FontFace>,
}

impl FontFamily {
    /// The always-present face. Coverage and chain selection are decided against
    /// this one and never against a styled face (D7), so adding a style attribute
    /// can never change *which font* a character comes from.
    pub fn regular(&self) -> &FontFace {
        &self.regular
    }

    /// The face filling `slot`, or `None` when this file ships no such face.
    pub fn slot(&self, slot: StyleSlot) -> Option<&FontFace> {
        match slot {
            StyleSlot::Regular => Some(&self.regular),
            StyleSlot::Bold => self.bold.as_ref(),
            StyleSlot::Italic => self.italic.as_ref(),
            StyleSlot::BoldItalic => self.bold_italic.as_ref(),
        }
    }

    /// Resolve a requested `(bold, italic)` to a real slot plus leftover synthesis
    /// (D9). Falls back one axis at a time rather than all-or-nothing: a file with
    /// a real bold but no italic still gets its **real bold** for bold-italic.
    pub fn resolve_style(&self, bold: bool, italic: bool) -> StyleResolution {
        resolve_style_over(bold, italic, |slot| self.slot(slot).is_some())
    }

    /// Build a family from explicit parts — **tests only**.
    ///
    /// Lets the atlas exercise D2's three outcomes (a slot the file never
    /// offered, one it offers that will not parse, and a broken *regular* face)
    /// without needing a damaged font file on disk.
    #[cfg(test)]
    pub(crate) fn from_parts(
        regular: FontFace,
        bold: Option<FontFace>,
        italic: Option<FontFace>,
        bold_italic: Option<FontFace>,
    ) -> Self {
        FontFamily {
            regular,
            bold,
            italic,
            bold_italic,
        }
    }

    /// Which slots this family filled with a real face — diagnostics and tests.
    pub fn real_slots(&self) -> Vec<StyleSlot> {
        [
            StyleSlot::Regular,
            StyleSlot::Bold,
            StyleSlot::Italic,
            StyleSlot::BoldItalic,
        ]
        .into_iter()
        .filter(|&s| self.slot(s).is_some())
        .collect()
    }
}

/// The ordered fallback chain. Index 0 is the primary family; the rest are
/// fallbacks in priority order (H-D1).
#[derive(Clone, Debug)]
pub struct FontSet {
    families: Vec<FontFamily>,
}

impl FontSet {
    /// Open the chain: the primary face, then the first fallback candidate that
    /// covers Hangul.
    ///
    /// Returns [`RendererError::NoFont`] **only** when no primary candidate is
    /// usable (H-R1.7). A fallback that cannot be opened logs a warning and leaves
    /// the set [degraded](FontSet::is_degraded) (H-D4/H-R1.6).
    pub fn load() -> Result<FontSet> {
        let primary = load_candidate(FONT_CANDIDATES, PRIMARY_PROBE)
            .ok_or_else(|| RendererError::NoFont(FONT_CANDIDATES.join(", ")))?;
        let mut families = vec![primary];

        // H-D4/H-R1.6: a missing fallback degrades rendering; never blocks startup.
        match load_candidate(FALLBACK_CANDIDATES, FALLBACK_PROBE) {
            Some(family) => families.push(family),
            None => tracing::warn!(
                candidates = FALLBACK_CANDIDATES.join(", "),
                "no fallback font with Hangul coverage could be opened; \
                 running degraded — non-Latin text will draw .notdef (H-D4/H-R1.6)"
            ),
        }

        Ok(FontSet { families })
    }

    /// True when no fallback family loaded, i.e. the chain is primary-only (H-D4).
    ///
    /// This is a statement about **chain entries**, and stays one (D11). A family
    /// that shipped no bold face is not degraded — it simply synthesizes that
    /// style, which is orthogonal to whether Hangul has a font at all.
    pub fn is_degraded(&self) -> bool {
        self.families.len() == 1
    }

    /// The chain in priority order; `families()[0]` is the primary family.
    pub fn families(&self) -> &[FontFamily] {
        &self.families
    }

    /// A chain built from explicit families — **tests only** (see
    /// [`FontFamily::from_parts`]).
    #[cfg(test)]
    pub(crate) fn from_families(families: Vec<FontFamily>) -> Self {
        FontSet { families }
    }
}

/// The style-fallback ladder (D9), over any notion of "is this slot available".
///
/// Shared deliberately. [`FontFamily`] answers from the faces the *file* ships;
/// the atlas answers from the rasterizers it actually *built*, which differ when a
/// face is present but unparseable (D2/R2.2). Two copies of this ladder would let
/// those two views disagree about what synthesis is owed, and the disagreement
/// would surface as a glyph rendered at the wrong weight — so there is one copy
/// and both call it.
///
/// The ordering is not arbitrary: it gives up the **slant** before the **weight**,
/// because on this font set synthesizing weight is the destructive option —
/// `embolden` erodes 593 Hangul syllables, while a 14° shear is merely synthetic.
pub fn resolve_style_over(
    bold: bool,
    italic: bool,
    available: impl Fn(StyleSlot) -> bool,
) -> StyleResolution {
    let exact = StyleSlot::wanted(bold, italic);
    if available(exact) {
        return StyleResolution {
            slot: exact,
            synth_bold: false,
            synth_italic: false,
        };
    }
    let candidates: &[(StyleSlot, bool, bool)] = match (bold, italic) {
        (true, true) => &[
            (StyleSlot::Bold, false, true),
            (StyleSlot::Italic, true, false),
            (StyleSlot::Regular, true, true),
        ],
        (true, false) => &[(StyleSlot::Regular, true, false)],
        (false, true) => &[(StyleSlot::Regular, false, true)],
        (false, false) => &[(StyleSlot::Regular, false, false)],
    };
    for &(slot, synth_bold, synth_italic) in candidates {
        if available(slot) {
            return StyleResolution {
                slot,
                synth_bold,
                synth_italic,
            };
        }
    }
    // `Regular` is always present, so this is unreachable in practice; answer with
    // full synthesis rather than panicking (no `unwrap`, C4).
    StyleResolution {
        slot: StyleSlot::Regular,
        synth_bold: bold,
        synth_italic: italic,
    }
}

/// The vertical metrics that identify a face's *design family*, in font design
/// units, rounded to integers so the comparison is exact (R1.3).
///
/// Needed because AppleSDGothicNeo ships **every** weight twice — `Apple SD Gothic
/// Neo Bold` and `.Apple SD Gothic NeoI Bold` are both weight 700 with identical
/// charmaps, but `900/300` vs `860/140` ascent/descent. Picking by index alone
/// works only while Apple keeps the non-dot face first, which is exactly the
/// ordering assumption this module warns about at the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MetricKey {
    upem: u16,
    ascent: i32,
    descent: i32,
    leading: i32,
}

fn metric_key(data: &[u8], index: usize) -> Option<MetricKey> {
    let m = FontRef::from_index(data, index)?.metrics(&[]);
    Some(MetricKey {
        upem: m.units_per_em,
        ascent: m.ascent.round() as i32,
        descent: m.descent.round() as i32,
        leading: m.leading.round() as i32,
    })
}

/// Whether a face's attributes report an italic (or oblique) slant.
fn is_italic(data: &[u8], index: usize) -> Option<bool> {
    let style = FontRef::from_index(data, index)?.attributes().style();
    Some(!matches!(style, swash::Style::Normal))
}

fn weight_of(data: &[u8], index: usize) -> Option<u16> {
    Some(FontRef::from_index(data, index)?.attributes().weight().0)
}

/// The weight this project calls **bold** (H-D1). Not "one step above regular":
/// Gothic's next step up is Medium 500, which is barely visible as bold, and
/// Menlo has no 500 at all — the two scripts would bold by different amounts.
const BOLD_WEIGHT: u16 = 700;

/// Find the face filling one style slot, or `None` if the file ships none (R1.1).
///
/// Selection is exact on `(weight, italic)`, then **prefers a face whose
/// [`MetricKey`] equals the regular's** (R1.2/R1.3, tolerance 0), and breaks any
/// remaining tie by lowest index. A match that *is* the regular face is refused:
/// a file whose regular is already weight 700 would otherwise report a bold that
/// renders identically to regular, i.e. a silently missing bold.
fn find_styled(
    data: &[u8],
    count: usize,
    want_weight: u16,
    want_italic: bool,
    regular_index: usize,
    regular_metrics: Option<MetricKey>,
) -> Option<usize> {
    let matches: Vec<usize> = (0..count)
        .filter(|&i| i != regular_index)
        .filter(|&i| weight_of(data, i) == Some(want_weight))
        .filter(|&i| is_italic(data, i) == Some(want_italic))
        .collect();
    pick_by_metrics(&matches, |i| {
        same_design_family(metric_key(data, i), regular_metrics)
    })
}

/// Whether a candidate face is the regular face's **design twin** — same `upem`,
/// ascent, descent and leading, compared as integers with no tolerance (R1.3).
///
/// `None` on either side means the metrics could not be read, and that is never a
/// match. Written as an explicit two-`Some` pattern rather than `a == b` because
/// `Option`'s own equality makes `None == None` true, which would declare every
/// *unreadable* face a twin of an unreadable regular — and then prefer it. The
/// earlier form, `regular.is_some() && candidate == regular`, expressed this only
/// by accident of operator order.
fn same_design_family(candidate: Option<MetricKey>, regular: Option<MetricKey>) -> bool {
    matches!((candidate, regular), (Some(c), Some(r)) if c == r)
}

/// The tie-break rule: prefer a candidate whose metrics match the regular's,
/// otherwise take the lowest index (R1.2/R1.3).
///
/// Split out from [`find_styled`] so the rule is unit-testable with a stub — no
/// font file, no GPU — exactly as [`first_covering`] is for the coverage probe.
fn pick_by_metrics(matches: &[usize], is_metric_match: impl Fn(usize) -> bool) -> Option<usize> {
    matches
        .iter()
        .copied()
        .find(|&i| is_metric_match(i))
        .or_else(|| matches.first().copied())
}

/// Fill a family's three styled slots from the same file the regular came from.
///
/// Every slot shares the regular's [`FontData`] handle — the `Arc` is cloned, the
/// 55 MB behind it never is (R1.5, H-D1/H-R1.2).
fn probe_styles(regular: FontFace) -> FontFamily {
    let Some(count) = FontDataRef::new(&regular.data).map(|d| d.len()) else {
        return FontFamily {
            regular,
            bold: None,
            italic: None,
            bold_italic: None,
        };
    };
    let data = regular.data.clone();
    let idx = regular.index;
    let key = metric_key(&data, idx);
    let regular_weight = weight_of(&data, idx).unwrap_or(400);

    let pick = |weight: u16, italic: bool| -> Option<FontFace> {
        find_styled(&data, count, weight, italic, idx, key).map(|index| FontFace {
            // R1.5: the handle, never the bytes.
            data: data.clone(),
            index,
        })
    };

    let family = FontFamily {
        bold: pick(BOLD_WEIGHT, false),
        italic: pick(regular_weight, true),
        bold_italic: pick(BOLD_WEIGHT, true),
        regular,
    };
    tracing::debug!(
        slots = ?family.real_slots(),
        "styled faces probed within the family (R1.1)"
    );
    family
}

/// Try each candidate path in order and return the first that both maps *and*
/// has a face covering `probe`.
///
/// A candidate whose faces all miss the probe is rejected outright and the search
/// moves on — that is what keeps a same-named-but-wrong font (the ancient-Gothic
/// `NotoSansGothic`) out of the chain (H-D3/H-R1.4).
fn load_candidate(candidates: &[&str], probe: char) -> Option<FontFamily> {
    for path in candidates {
        let Some(data) = map_font(path) else {
            continue;
        };
        match probe_face(&data, probe) {
            Some(index) => {
                tracing::debug!(path = *path, index, "font face selected");
                // The regular face is still chosen by *coverage* (H-D3/H-R1.3);
                // only the styled slots beside it are chosen by attributes.
                return Some(probe_styles(FontFace { data, index }));
            }
            None => tracing::debug!(
                path = *path,
                "no face covers the probe character; rejecting candidate (H-R1.4)"
            ),
        }
    }
    None
}

/// Memory-map a font file (H-D1/H-R1.2). **The only `unsafe` in this crate.**
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
/// maps `probe` to a non-zero glyph id, or `None` if no face does (H-D3/H-R1.3).
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

    /// A family with only its regular slot filled — the shape the chain has
    /// when a file ships no styled faces at all.
    fn dummy_family() -> FontFamily {
        FontFamily {
            regular: dummy_face(),
            bold: None,
            italic: None,
            bold_italic: None,
        }
    }

    #[test]
    fn degraded_means_no_fallback_face() {
        // H-D4/H-R1.6: primary-only == degraded; one fallback leaves it.
        // D11: this counts chain ENTRIES, and empty style slots do not affect it.
        let primary_only = FontSet {
            families: vec![dummy_family()],
        };
        assert!(primary_only.is_degraded());
        let with_fallback = FontSet {
            families: vec![dummy_family(), dummy_family()],
        };
        assert!(!with_fallback.is_degraded());
        assert_eq!(with_fallback.families().len(), 2);
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn loads_primary_and_hangul_fallback() {
        // R1.1: an ordered chain — primary first, Hangul face second, not degraded.
        let set = FontSet::load().expect("a primary system font");
        assert!(!set.is_degraded(), "a Hangul fallback should be available");
        assert_eq!(set.families().len(), 2);
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
        let families = set.families();
        assert!(covers(families[0].regular(), PRIMARY_PROBE));
        assert!(covers(families[1].regular(), FALLBACK_PROBE));
    }

    // ---- T1 / R1: styled-face probing ------------------------------------

    #[test]
    fn pick_by_metrics_prefers_a_metric_match_over_a_lower_index() {
        // R1.2: AppleSDGothicNeo ships every weight twice. Index order alone gets
        // the right answer today only because Apple happens to put the non-dot
        // face first — the very assumption this module warns about at the top.
        assert_eq!(pick_by_metrics(&[6, 7], |i| i == 7), Some(7));
        assert_eq!(pick_by_metrics(&[6, 7], |i| i == 6), Some(6));
    }

    fn key(upem: u16, ascent: i32, descent: i32) -> Option<MetricKey> {
        Some(MetricKey {
            upem,
            ascent,
            descent,
            leading: 0,
        })
    }

    #[test]
    fn a_design_twin_needs_every_metric_to_agree() {
        // R1.3: tolerance 0, all four fields. The real case this decides is
        // `Apple SD Gothic Neo Bold` (900/300) vs `.Apple SD Gothic NeoI Bold`
        // (860/140) — same weight, same charmap, different design family.
        let regular = key(1000, 900, 300);
        assert!(same_design_family(key(1000, 900, 300), regular));
        assert!(
            !same_design_family(key(1000, 860, 140), regular),
            "the .NeoI twin"
        );
        assert!(
            !same_design_family(key(1000, 900, 301), regular),
            "no tolerance"
        );
        assert!(
            !same_design_family(key(2048, 900, 300), regular),
            "upem differs"
        );
    }

    #[test]
    fn unreadable_metrics_are_never_a_design_twin() {
        // The trap this function exists to avoid: `Option`'s own equality makes
        // `None == None` true, so a naive `candidate == regular` would call two
        // *unreadable* faces twins and then prefer one of them.
        assert!(!same_design_family(None, None));
        assert!(!same_design_family(None, key(1000, 900, 300)));
        assert!(!same_design_family(key(1000, 900, 300), None));
    }

    #[test]
    fn the_tie_break_prefers_the_twin_over_the_lower_index() {
        // R1.2 as a composition: `pick_by_metrics` + `same_design_family` is the
        // whole rule. Face 6 is the twin and face 7 is not, and 7 is NOT lower —
        // so this also pins that a metric match beats index order, which is the
        // property that survives an OS reshuffling the TTC.
        let regular = key(1000, 900, 300);
        let metrics = |i: usize| {
            if i == 7 {
                key(1000, 900, 300)
            } else {
                key(1000, 860, 140)
            }
        };
        assert_eq!(
            pick_by_metrics(&[6, 7], |i| same_design_family(metrics(i), regular)),
            Some(7),
            "the twin wins even though it is the higher index"
        );
        // And with the metric source blinded — the `metric_key -> None` mutation —
        // the rule degrades to lowest index rather than picking wrongly.
        assert_eq!(
            pick_by_metrics(&[6, 7], |_| same_design_family(None, regular)),
            Some(6)
        );
    }

    #[test]
    fn pick_by_metrics_falls_back_to_lowest_index_when_nothing_matches() {
        // R1.3 + G-C: no metric match at all is not an error; take the lowest.
        assert_eq!(pick_by_metrics(&[4, 9, 2], |_| false), Some(4));
        assert_eq!(pick_by_metrics(&[], |_| true), None);
    }

    #[test]
    fn resolve_style_uses_a_real_face_when_one_exists() {
        // R2.5 (pure half): an exact slot means no synthesis at all.
        let f = FontFamily {
            regular: dummy_face(),
            bold: Some(dummy_face()),
            italic: Some(dummy_face()),
            bold_italic: Some(dummy_face()),
        };
        for (b, i, want) in [
            (false, false, StyleSlot::Regular),
            (true, false, StyleSlot::Bold),
            (false, true, StyleSlot::Italic),
            (true, true, StyleSlot::BoldItalic),
        ] {
            let r = f.resolve_style(b, i);
            assert_eq!(r.slot, want);
            assert!(
                !r.synth_bold && !r.synth_italic,
                "{b},{i} should not synthesize"
            );
        }
    }

    #[test]
    fn resolve_style_keeps_real_weight_and_shears_when_italic_is_absent() {
        // D9/R2.5: this IS AppleSDGothicNeo — nine weights, zero italic faces.
        // Bold-italic must be real w700 + shear, never `embolden` on regular:
        // embolden is what erodes 593 Hangul syllables.
        let gothic_shaped = FontFamily {
            regular: dummy_face(),
            bold: Some(dummy_face()),
            italic: None,
            bold_italic: None,
        };
        let r = gothic_shaped.resolve_style(true, true);
        assert_eq!(r.slot, StyleSlot::Bold);
        assert!(!r.synth_bold, "the weight is real, so never embolden it");
        assert!(r.synth_italic, "the slant has no face, so shear it");

        let r = gothic_shaped.resolve_style(false, true);
        assert_eq!(r.slot, StyleSlot::Regular);
        assert!(!r.synth_bold && r.synth_italic);
    }

    #[test]
    fn resolve_style_keeps_real_slant_and_emboldens_when_bold_is_absent() {
        // The mirror of the AppleSDGothicNeo case, and the branch the Gate-3
        // checker found untested: a file with a real *italic* but no real bold.
        // The ladder must take `(Italic, synth_bold)` rather than collapsing to
        // full synthesis on regular — keeping whichever axis is real is the whole
        // point of resolving the axes independently (D9).
        let italic_only = FontFamily {
            regular: dummy_face(),
            bold: None,
            italic: Some(dummy_face()),
            bold_italic: None,
        };
        let r = italic_only.resolve_style(true, true);
        assert_eq!(r.slot, StyleSlot::Italic, "the real slant is kept");
        assert!(r.synth_bold, "the weight has no face, so embolden it");
        assert!(!r.synth_italic, "the slant is real, so never shear it");

        // And plain italic still uses the real face with no synthesis at all.
        let r = italic_only.resolve_style(false, true);
        assert_eq!(r.slot, StyleSlot::Italic);
        assert!(!r.synth_bold && !r.synth_italic);
    }

    #[test]
    fn resolve_style_synthesizes_both_axes_only_when_nothing_real_exists() {
        // The degraded shape: a one-face file (AppleGothic.ttf, SFNSMono.ttf).
        let bare = dummy_family();
        let r = bare.resolve_style(true, true);
        assert_eq!(r.slot, StyleSlot::Regular);
        assert!(r.synth_bold && r.synth_italic);
        assert_eq!(bare.real_slots(), vec![StyleSlot::Regular]);
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn probes_the_real_bold_faces_both_fonts_ship() {
        // R1.1: the whole point — these faces exist in the files and were never
        // opened before this change. Menlo Bold is face 1, Apple SD Gothic Neo
        // Bold (weight 700) is face 6.
        let set = FontSet::load().expect("a primary system font");
        let fams = set.families();
        assert_eq!(fams.len(), 2, "primary + Hangul fallback");

        let menlo = &fams[0];
        assert_eq!(
            menlo.slot(StyleSlot::Bold).map(|f| f.index),
            Some(1),
            "Menlo Bold"
        );
        assert_eq!(
            menlo.slot(StyleSlot::Italic).map(|f| f.index),
            Some(2),
            "Menlo Italic"
        );
        assert_eq!(
            menlo.slot(StyleSlot::BoldItalic).map(|f| f.index),
            Some(3),
            "Menlo Bold Italic"
        );

        let gothic = &fams[1];
        assert_eq!(
            gothic.slot(StyleSlot::Bold).map(|f| f.index),
            Some(6),
            "Apple SD Gothic Neo Bold (w700), NOT the .NeoI twin at 7"
        );
        // R1.4 made observable: the same rule ran on BOTH chain entries — the
        // primary got styled slots too, so no per-entry exception exists.
        assert!(
            gothic.slot(StyleSlot::Italic).is_none(),
            "all 18 Gothic faces are style=Normal; there is no italic to find"
        );
        assert!(gothic.slot(StyleSlot::BoldItalic).is_none());
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn styled_faces_share_the_regular_face_mapping() {
        // R1.5: the 55 MB is mapped once. Styled slots clone the Arc, not the
        // bytes — this is the whole reason H-D1/H-R1.2 chose mmap.
        let set = FontSet::load().expect("a primary system font");
        for family in set.families() {
            let base = &family.regular().data;
            for slot in family.real_slots() {
                let face = family.slot(slot).expect("a real slot");
                assert!(
                    Arc::ptr_eq(base, &face.data),
                    "{slot:?} must share the regular face's mapping, not re-map"
                );
            }
        }
    }

    #[test]
    #[ignore = "depends on a system font path; run with --ignored"]
    fn the_full_style_probe_is_off_the_startup_budget() {
        // R1.6: this runs on every FontSet::load, and Atlas::new re-runs on every
        // apply_config ppem change. Budget is input→screen ≤ 8 ms; measured ~4 µs.
        let start = std::time::Instant::now();
        let set = FontSet::load().expect("a primary system font");
        let elapsed = start.elapsed();
        assert_eq!(set.families().len(), 2);
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "FontSet::load took {elapsed:?}; the probe must stay far under the \
             8 ms input→screen budget (debug builds are ~10x release)"
        );
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
