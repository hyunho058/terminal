//! Bold comes from a real face — the glyph-level guarantee (T6).
//!
//! This is the half of the contract `golden_frame.rs` structurally cannot express.
//! That file deliberately asserts **which cells carry ink** and not how much, so it
//! survives a font substitution — which also means it stays green while every bold
//! glyph on screen is a faint ghost. The claim this feature rests on is a claim
//! about *quantity*, over all 11,172 Hangul syllables, so it needs its own sweep.
//!
//! What it pins, and why each number is what it is:
//!
//! * **No syllable loses ink when bolded (R0.2).** At `ae4be97` this fails for
//!   **593 of 11,172** syllables — 안 간 건 난 인 언 연 강 변 교 among them, at
//!   −86% to −89%. `swash`'s `embolden` offsets AppleSDGothicNeo's contours in a
//!   direction that makes the fill cancel, so the glyph erodes instead of
//!   thickening. It is not a hinting artifact: the counts are the same with
//!   `hint(false)`, at ppem 14 and 28 alike.
//! * **Bold is legible *as* bold (R7.1).** "Not thinner" is too weak a bar — a
//!   1.01× bold reads as regular. D18 sets the floor at **1.15×** coverage-weighted
//!   ink, measured at the real default (`size_pt = 14`, so ppem 14 at 1× backing
//!   and 28 at 2×).
//! * **The Latin exceptions have not grown (R7.2).** Menlo Bold draws a handful of
//!   punctuation marks *lighter* than Menlo Regular. That is the font designer's
//!   choice, not a defect, and bold rarely carries meaning on a comma — so the set
//!   is recorded and frozen rather than fixed.
//!
//! No GPU and no atlas: this runs against `FontSet` + `GlyphRasterizer` directly.
//! It reads the machine's system fonts, so it follows the repo's `#[ignore]`
//! convention — run with `cargo test -p renderer --test bold_faces -- --ignored`.

use renderer::font_set::{FontSet, StyleSlot};
use renderer::glyph::GlyphRasterizer;

/// The ratio below which a bold glyph stops reading as bold (D18/R7.1).
const DIFFERENTIABILITY_FLOOR: f64 = 1.15;

/// `size_pt = 14` at 1× and 2× backing scale — the two sizes a real window uses.
const PPEMS: [f32; 2] = [14.0, 28.0];

/// Menlo Bold glyphs that carry *less* ink than Menlo Regular, measured at both
/// ppems (R7.2). Frozen, not fixed: these are punctuation, and the bold face draws
/// them as the designer intended. The test asserts the set does not **grow** — a
/// new member means something changed about how Latin bold is selected.
const KNOWN_LIGHT_LATIN: &[char] = &['"', '\'', '.', ',', '%', '_', ':', ';'];

/// Coverage-weighted ink: sum of alpha, so antialiasing counts proportionally
/// rather than a hard on/off threshold.
fn ink(bitmap: &renderer::glyph::GlyphBitmap) -> f64 {
    bitmap.coverage.iter().map(|&a| a as f64 / 255.0).sum()
}

/// A regular/bold rasterizer pair for one chain entry.
///
/// `None` means **this machine has no such font at all** — the only condition that
/// may legitimately skip these tests. A family that loaded but ships no bold slot
/// is a *failure*, not a skip: on any machine that has these fonts, Menlo and
/// AppleSDGothicNeo both carry a real bold face, so an empty slot means the probe
/// regressed to synthesis — which is the exact defect this file exists to catch.
///
/// The distinction is load-bearing. An earlier version returned `None` for a
/// missing bold slot, and every assertion below silently turned into a no-op: with
/// the bold probe deliberately disabled the whole file still reported
/// `4 passed; 0 failed` in 0.00s.
fn pair(family_index: usize, ppem: f32) -> Option<(GlyphRasterizer, GlyphRasterizer)> {
    let Ok(set) = FontSet::load() else {
        return None; // no system fonts — skip
    };
    let Some(family) = set.families().get(family_index) else {
        return None; // no such chain entry — skip
    };
    let regular = family.regular().clone();
    let bold = family
        .slot(StyleSlot::Bold)
        .unwrap_or_else(|| {
            panic!(
                "family {family_index} loaded but has NO real bold face — the style \
                 probe has fallen back to synthesis (real slots: {:?})",
                family.real_slots()
            )
        })
        .clone();
    Some((
        GlyphRasterizer::new(regular.data, regular.index, ppem).expect("regular rasterizer"),
        GlyphRasterizer::new(bold.data, bold.index, ppem).expect("bold rasterizer"),
    ))
}

/// Every character in `chars` whose bold ink is below `floor` times its regular
/// ink, together with the two extremes for a useful failure message.
struct Sweep {
    below: Vec<(char, f64)>,
    scanned: u32,
    worst: Option<(char, f64)>,
}

fn sweep(
    regular: &mut GlyphRasterizer,
    bold: &mut GlyphRasterizer,
    chars: impl Iterator<Item = char>,
    floor: f64,
) -> Sweep {
    let mut below = Vec::new();
    let mut scanned = 0;
    let mut worst: Option<(char, f64)> = None;
    for ch in chars {
        // Each face is asked for the id in ITS OWN charmap: glyph ids are a
        // per-face namespace and do not survive a face change (D15).
        let Some(r) = regular.rasterize(ch) else {
            continue;
        };
        let regular_ink = ink(&r);
        if regular_ink <= 0.0 {
            continue;
        }
        let bold_ink = bold.rasterize(ch).map(|b| ink(&b)).unwrap_or(0.0);
        scanned += 1;
        let ratio = bold_ink / regular_ink;
        if worst.is_none_or(|(_, w)| ratio < w) {
            worst = Some((ch, ratio));
        }
        if ratio < floor {
            below.push((ch, ratio));
        }
    }
    Sweep {
        below,
        scanned,
        worst,
    }
}

fn hangul() -> impl Iterator<Item = char> {
    (0xAC00u32..=0xD7A3).filter_map(char::from_u32)
}

fn ascii() -> impl Iterator<Item = char> {
    (0x21u32..=0x7E).filter_map(char::from_u32)
}

#[test]
#[ignore = "depends on a system font path; run with --ignored"]
fn no_hangul_syllable_loses_ink_when_bolded() {
    // R0.2 — the headline. 593 syllables fail this at ae4be97.
    for ppem in PPEMS {
        let Some((mut regular, mut bold)) = pair(1, ppem) else {
            eprintln!("skipping: no Hangul fallback family with a real bold face");
            return;
        };
        let s = sweep(&mut regular, &mut bold, hangul(), 1.0);
        assert_eq!(
            s.scanned, 11172,
            "the whole Hangul syllable block should rasterize at ppem {ppem}"
        );
        assert!(
            s.below.is_empty(),
            "ppem {ppem}: {} of {} Hangul syllables LOSE ink when bolded — \
             embolden is eroding outlines instead of thickening them. \
             First 10: {:?}",
            s.below.len(),
            s.scanned,
            &s.below[..s.below.len().min(10)]
        );
    }
}

#[test]
#[ignore = "depends on a system font path; run with --ignored"]
fn every_hangul_syllable_reads_as_bold() {
    // R7.1 — the stronger bar: not merely "not thinner" but visibly heavier.
    for ppem in PPEMS {
        let Some((mut regular, mut bold)) = pair(1, ppem) else {
            eprintln!("skipping: no Hangul fallback family with a real bold face");
            return;
        };
        let s = sweep(&mut regular, &mut bold, hangul(), DIFFERENTIABILITY_FLOOR);
        assert!(
            s.below.is_empty(),
            "ppem {ppem}: {} of {} syllables fall below the {DIFFERENTIABILITY_FLOOR}x \
             differentiability floor (D18). Worst: {:?}. First 10: {:?}",
            s.below.len(),
            s.scanned,
            s.worst,
            &s.below[..s.below.len().min(10)]
        );
    }
}

#[test]
#[ignore = "depends on a system font path; run with --ignored"]
fn the_latin_bold_exceptions_have_not_grown() {
    // R7.2 — Latin is where D18's objection survives, so it is pinned rather than
    // asserted away. A NEW member of this set means Latin bold face selection
    // changed, which is exactly what we would want to hear about.
    for ppem in PPEMS {
        let Some((mut regular, mut bold)) = pair(0, ppem) else {
            eprintln!("skipping: no primary family with a real bold face");
            return;
        };
        let s = sweep(&mut regular, &mut bold, ascii(), DIFFERENTIABILITY_FLOOR);
        let unexpected: Vec<(char, f64)> = s
            .below
            .iter()
            .copied()
            .filter(|(ch, _)| !KNOWN_LIGHT_LATIN.contains(ch))
            .collect();
        assert!(
            unexpected.is_empty(),
            "ppem {ppem}: ASCII characters newly below the {DIFFERENTIABILITY_FLOOR}x \
             floor that are not in the recorded exception set: {unexpected:?}"
        );
    }
}

#[test]
#[ignore = "depends on a system font path; run with --ignored"]
fn bold_hangul_is_not_the_emboldened_regular() {
    // D1 made concrete: the bold bitmap must come from the w700 outlines, not from
    // the regular ones with embolden applied. If these ever match, the probe has
    // silently fallen back to synthesis and every guarantee above is vacuous.
    let Some((mut regular, mut bold)) = pair(1, 28.0) else {
        eprintln!("skipping: no Hangul fallback family with a real bold face");
        return;
    };
    let mut differed = 0;
    for ch in ['가', '글', '안', '뭘', '한'] {
        let gid = regular.glyph_id(ch);
        let synthetic = regular.rasterize_glyph_synth(gid, true, false);
        let real = bold.rasterize(ch);
        assert!(real.is_some(), "the bold face must cover {ch}");
        if real != synthetic {
            differed += 1;
        }
    }
    assert_eq!(
        differed, 5,
        "every sample should differ from its synthetic twin"
    );
}
