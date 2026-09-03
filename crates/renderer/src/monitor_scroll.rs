//! Scroll coordinate space for the ai-monitor panel (ai-monitor-panel-scroll D17/D22/D31).
//!
//! The panel scrolls in **rungs of the item ladder**, not pixels. That is D17, and D17 is
//! not a feel decision — the panel has no scissor rect (C1, `monitor_layout::Painter::fits`),
//! so a half-drawn row has no representation at all. Snapping the offset to a rung boundary
//! is what makes the missing capability unnecessary rather than missed.
//!
//! Everything here is a pure function over `(ladder, heights, viewport_h)`. None of it
//! touches a model, a frame or the GPU, so the whole coordinate space is exercised by
//! ordinary unit tests — which matters, because the arithmetic below is the part that is
//! wrong in a way nobody notices: an off-by-one lands the view a rung away from where the
//! user asked and nothing crashes.
//!
//! Lives in its own module rather than inside `monitor_layout` so that this work and the
//! layout restructuring do not queue behind each other on one file — and because "where
//! does the view sit" is a genuinely different concern from "what does a frame contain".

use crate::monitor_layout::Item;

/// Is `i` a position the view can come to rest at?
///
/// **The first body item of a section is not**, and that is the whole rule. D31 has the
/// sticky slot *consume* a heading: at the heading's own index `h` the slot shows it and
/// content starts at `h + 1`; at index `h + 1` the slot shows that same heading (now
/// off-screen) and content starts at `h + 1` again. The two frames are pixel-identical, so
/// offering both as stops spends a wheel notch changing nothing — six dead notches per pass
/// over the document, one per section.
///
/// A heading is always a stop, including the one right after an **empty** section's
/// heading: there the slot contents differ, so the frames differ.
pub fn is_stop(items: &[Item], i: usize) -> bool {
    match (i.checked_sub(1).and_then(|p| items.get(p)), items.get(i)) {
        // Reads as "NOT (a heading is above me and I am not one myself)" — i.e. "I am
        // not the first body rung of my section".
        (Some(prev), Some(cur)) => !prev.is_heading() || cur.is_heading(),
        // Index 0 is always a stop; an out-of-range index is not.
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Where the content region begins when the view sits at `top` — `top + 1` if the slot
/// consumed a heading there, `top` otherwise (D31).
pub fn content_start(items: &[Item], top: usize) -> usize {
    match items.get(top) {
        Some(item) if item.is_heading() => top + 1,
        _ => top,
    }
}

/// Total height of rungs `from..`, in px.
fn tail_h(heights: &[f32], from: usize) -> f32 {
    heights
        .get(from..)
        .map(|t| t.iter().filter(|h| h.is_finite()).sum())
        .unwrap_or(0.0)
}

/// Does the end of the document fit on screen when the view sits at `top`?
///
/// Measured from [`content_start`], not from `top`: a heading at the top is drawn in the
/// slot, so the content region has one more rung's worth of room than a naive `heights[top..]`
/// would suggest. Getting this wrong puts the answer one rung out at exactly the indices
/// D16's jumps land on.
fn bottom_visible(items: &[Item], heights: &[f32], viewport_h: f32, top: usize) -> bool {
    tail_h(heights, content_start(items, top)) <= viewport_h + f32::EPSILON
}

/// The index of section `section`'s heading — where `Cmd+Shift+N` lands (D16/R8.1).
pub fn jump_target(items: &[Item], section: usize) -> Option<usize> {
    items
        .iter()
        .position(|i| i.is_heading() && i.section == section)
}

/// The furthest the view may scroll (D22).
///
/// Three cases, and the order between them is the decision:
///
/// 1. **A degenerate viewport** is `0`. Two shapes of it, and the second is the one that is
///    easy to miss: no height or no ladder at all (the slot itself did not fit, so `layout`
///    handed back an empty band), *and* a band that exists but cannot hold a single rung
///    whole. The height sweep starts at 2px and both are reachable inside it.
/// 2. **A document that fits** is `0` (R3.6). This case is checked *before* case 3 and
///    overrides it: when everything is already on screen there is nothing to reach, and
///    applying case 3 anyway would let the user push a fully-visible document up until only
///    the last heading remained.
/// 3. Otherwise `max(first index whose tail fits, the last section's heading)`. Both terms
///    are needed and neither dominates: with a long final section the first is larger, with
///    a short one — TOOL USAGE with three bars is the motivating case — the second is. Using
///    only the first is the sidebar's `content_h - viewport_h` clamp, and under it
///    `Cmd+Shift+6` **silently fails to bring TOOL USAGE to the top**, which leaves D12
///    unsolved while looking implemented.
pub fn max_index(items: &[Item], heights: &[f32], viewport_h: f32) -> usize {
    if items.is_empty() || !viewport_h.is_finite() || viewport_h <= 0.0 {
        return 0;
    }
    if bottom_visible(items, heights, viewport_h, 0) {
        return 0;
    }
    // D22's degenerate clause, keyed the way D22 words it — **no rung fits whole**, not "the
    // tail never fits". The two come apart exactly when the LAST rung is oversized while
    // earlier ones fit: keying on the tail then answers `0` and strands everything past the
    // first screen, trading a real correctness fix for a total reachability loss. (The
    // shipped model does not produce that shape — TOOL USAGE ends the document with the
    // shortest rung there is — but this module is pure, `pub`, and takes the ladder as data,
    // which is the whole point of C11. Adding a section after TOOL USAGE would be enough.)
    if !heights
        .iter()
        .any(|h| h.is_finite() && *h > 0.0 && *h <= viewport_h + f32::EPSILON)
    {
        return 0;
    }
    let tail_fits = (0..items.len())
        .find(|&i| bottom_visible(items, heights, viewport_h, i))
        .unwrap_or(items.len().saturating_sub(1));
    let last_heading = items
        .iter()
        .rposition(Item::is_heading)
        .unwrap_or(items.len().saturating_sub(1));
    snap_down(items, tail_fits.max(last_heading))
}

/// The nearest stop at or below `i` — used to land an arithmetic result on a legal position.
fn snap_down(items: &[Item], i: usize) -> usize {
    let mut i = i.min(items.len().saturating_sub(1));
    while i > 0 && !is_stop(items, i) {
        i -= 1;
    }
    i
}

/// Bring `i` into `[0, max_index]` and onto a stop.
pub fn clamp(items: &[Item], heights: &[f32], viewport_h: f32, i: usize) -> usize {
    snap_down(items, i.min(max_index(items, heights, viewport_h)))
}

/// Move `delta` stops from `from` (negative is toward the top of the document).
///
/// Counts *stops*, not indices, so one wheel notch always changes the frame — see
/// [`is_stop`]. Saturates at both ends rather than wrapping; D9's edge fall-through is the
/// app's business and it needs to see that the panel did not move.
pub fn step(items: &[Item], heights: &[f32], viewport_h: f32, from: usize, delta: i32) -> usize {
    let max = max_index(items, heights, viewport_h);
    let mut at = snap_down(items, from.min(max));
    let mut left = delta.unsigned_abs() as usize;
    while left > 0 {
        let next = if delta > 0 {
            (at + 1..=max).find(|&i| is_stop(items, i))
        } else {
            (0..at).rev().find(|&i| is_stop(items, i))
        };
        match next {
            Some(i) => at = i,
            None => break,
        }
        left -= 1;
    }
    at
}

/// Where a wheel gesture of `px` leaves the view, and what is left over.
///
/// **Positive `px` means toward the top of the document** — winit's convention, the same one
/// `wheel_lines` follows for the grid, so the panel and the terminal answer a gesture in the
/// same direction without the caller flipping a sign.
///
/// Travel is measured in **pixels and spent in rungs**, which is the only honest way to do
/// it when rungs are unequal (a card is two lines, a bar is one). Converting px to a rung
/// count up front would make one notch cross two bars or half a card depending on where the
/// view happened to be.
///
/// The leftover is returned rather than discarded: a trackpad delivers a few px per event,
/// so truncating each one independently would round every gesture to zero and the panel
/// would look frozen. It is the panel's OWN residue — D23 keeps it strictly apart from the
/// grid's, because a rung and a text line are different units and mixing them makes a
/// reversal lurch.
///
/// Saturates at both ends. A caller detects the edge by the returned index not moving, which
/// is what D9's fall-through keys off.
pub fn scroll_by_px(
    items: &[Item],
    heights: &[f32],
    viewport_h: f32,
    top: usize,
    px: f32,
    residue: f32,
) -> (usize, f32) {
    let max = max_index(items, heights, viewport_h);
    let mut at = snap_down(items, top.min(max));
    if !px.is_finite() || !residue.is_finite() {
        // A non-finite delta must not poison the residue for the rest of the session.
        return (at, 0.0);
    }
    let mut left = px + residue;
    loop {
        // The rung a step would cross: going up it is the one we would land on, going down
        // it is the one we would leave.
        let next = if left > 0.0 {
            (0..at).rev().find(|&i| is_stop(items, i))
        } else if left < 0.0 {
            (at + 1..=max).find(|&i| is_stop(items, i))
        } else {
            None
        };
        let Some(target) = next else {
            // At the edge in this direction: drop the residue rather than bank it. Banking
            // it would make the first event after the view can move again lurch by whatever
            // accumulated while it could not.
            return (at, 0.0);
        };
        // What the *content* actually travels between the two positions — the difference of
        // their document offsets, not the height of the rungs between their indices. Those
        // differ whenever a heading is involved: moving from a heading's index to the first
        // stop below it crosses one index more than it crosses pixels, because the slot was
        // already showing that heading (D31's consumption rule). Measuring by index would
        // make every section boundary cost a phantom rung.
        let cost = (offset_px(items, heights, target) - offset_px(items, heights, at)).abs();
        if cost <= 0.0 || cost > left.abs() {
            break;
        }
        left -= cost * left.signum();
        at = target;
    }
    (at, left)
}

/// Document-local `y` of the content when the view sits at `top` — what the eye measures
/// travel against.
fn offset_px(items: &[Item], heights: &[f32], top: usize) -> f32 {
    span_h(heights, 0, content_start(items, top))
}

/// The nearest heading at or above `top` — the one the sticky slot is showing (D31).
///
/// `None` only for a document whose first rungs are not a heading, which `item_ladder`
/// cannot produce; the option is the honest return for an empty ladder.
pub fn heading_above(items: &[Item], top: usize) -> Option<usize> {
    (0..=top.min(items.len().saturating_sub(1)))
        .rev()
        .find(|&i| items.get(i).is_some_and(Item::is_heading))
}

/// Which rung a point in the content band lands on, or `None` past the last drawn one.
///
/// `y` is measured from the band's own top edge, not the window's — the caller owns that
/// translation, the same division the wheel already keeps (the app performs no geometry).
///
/// Only rungs that are **wholly** inside the band can be hit, which is not a nicety but the
/// same fact the ladder is built on: without a scissor rect a partly-visible rung is never
/// drawn, so a click at that height lands on nothing. Returning `None` there is honest;
/// returning the undrawn rung would toggle a fold the user cannot see.
pub fn hit(items: &[Item], heights: &[f32], viewport_h: f32, top: usize, y: f32) -> Option<usize> {
    if !(y.is_finite() && y >= 0.0 && viewport_h.is_finite()) {
        return None;
    }
    let mut acc = 0.0f32;
    for i in content_start(items, top)..items.len() {
        let h = *heights.get(i)?;
        if !h.is_finite() || h <= 0.0 {
            continue;
        }
        if acc + h > viewport_h + f32::EPSILON {
            return None; // this rung is clipped away, and so is everything after it
        }
        if y < acc + h {
            return Some(i);
        }
        acc += h;
    }
    None
}

/// Height of rungs `[a, b)`.
fn span_h(heights: &[f32], a: usize, b: usize) -> f32 {
    heights
        .get(a..b)
        .map(|s| s.iter().filter(|h| h.is_finite()).sum())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor_layout::ItemKind;

    #[test]
    fn r13_4_a_press_lands_on_the_rung_under_it_and_on_nothing_past_the_band() {
        // The click's half of the ladder. Heights are uniform here so the arithmetic is
        // checkable by eye; `hit` reads the real per-rung heights in production.
        let items = ladder(&[3, 3]);
        let h = vec![10.0; items.len()];

        // At the top the slot consumes rung 0's heading, so the band starts at rung 1.
        assert_eq!(hit(&items, &h, 25.0, 0, 0.0), Some(1));
        assert_eq!(hit(&items, &h, 25.0, 0, 9.9), Some(1));
        assert_eq!(hit(&items, &h, 25.0, 0, 10.0), Some(2));
        // The band holds two whole rungs of 10 in 25 px; the third is clipped and therefore
        // never drawn, so a press at its height lands on nothing rather than on a rung the
        // user cannot see. This is the no-scissor rule showing up in the input path.
        assert_eq!(hit(&items, &h, 25.0, 0, 21.0), None);

        // Scrolled: the band starts wherever `content_start` says, not at rung 0.
        assert_eq!(hit(&items, &h, 25.0, 2, 0.0), Some(2));
        // Nonsense coordinates resolve to nothing instead of panicking.
        assert_eq!(hit(&items, &h, 25.0, 0, -1.0), None);
        assert_eq!(hit(&items, &h, 25.0, 0, f32::NAN), None);
    }

    #[test]
    fn r13_5_the_slot_reports_the_heading_it_is_actually_showing() {
        let items = ladder(&[3, 3]);
        // Inside the first section, the slot shows section 0's heading — which is at 0.
        assert_eq!(heading_above(&items, 2), Some(0));
        // On a heading rung, that heading is its own answer (the slot consumes it, D31).
        assert_eq!(heading_above(&items, 4), Some(4));
        assert_eq!(heading_above(&items, 5), Some(4));
        // An index past the end degrades to the last heading rather than panicking.
        assert_eq!(heading_above(&items, 999), Some(4));
        assert_eq!(heading_above(&[], 0), None);
    }

    /// A ladder from a compact description: `&[(section_entries)]`. Section `k` contributes
    /// one heading and `entries[k]` body rungs, which is exactly what `item_ladder` produces.
    fn ladder(entries: &[usize]) -> Vec<Item> {
        let mut out = Vec::new();
        for (section, &n) in entries.iter().enumerate() {
            out.push(Item {
                section,
                kind: ItemKind::Heading,
                entry: None,
            });
            for e in 0..n {
                out.push(Item {
                    section,
                    kind: ItemKind::Row,
                    entry: Some(e),
                });
            }
        }
        out
    }

    /// Every rung the same height, so the arithmetic below reads in units of rungs.
    fn flat(items: &[Item], h: f32) -> Vec<f32> {
        vec![h; items.len()]
    }

    #[test]
    fn r3_4_the_first_body_rung_of_a_section_is_not_a_stop() {
        // At the heading's index the slot consumes it and content starts at h+1; at h+1 the
        // slot shows the same (off-screen) heading and content starts at h+1 again. Two
        // indices, one frame. Offering both spends a wheel notch on nothing.
        let items = ladder(&[3, 2]);
        // 0:H0 1:R 2:R 3:R 4:H1 5:R 6:R
        assert!(is_stop(&items, 0), "a heading is always a stop");
        assert!(
            !is_stop(&items, 1),
            "the section's first row duplicates index 0"
        );
        assert!(is_stop(&items, 2));
        assert!(is_stop(&items, 3));
        assert!(is_stop(&items, 4), "the next heading is a stop");
        assert!(!is_stop(&items, 5), "…and its first row is not");
        assert!(is_stop(&items, 6));
    }

    #[test]
    fn r3_4_an_empty_sections_neighbour_is_a_stop_because_the_slot_changes() {
        // The rule is "first body rung of a section", not "index after a heading". After an
        // empty section's heading comes the *next section's* heading, and there the slot
        // contents differ — so the frames differ and the index earns its stop.
        let items = ladder(&[0, 2]);
        // 0:H0(empty) 1:H1 2:R 3:R
        assert!(is_stop(&items, 0));
        assert!(
            is_stop(&items, 1),
            "a heading after a heading is still a stop"
        );
        assert!(!is_stop(&items, 2), "but H1's first row is not");
        assert!(is_stop(&items, 3));
    }

    #[test]
    fn r3_4_stepping_skips_the_duplicate_rather_than_wasting_a_notch() {
        // The user-visible consequence: one notch, one changed frame. Without the skip a
        // pass over six sections stalls six times.
        let items = ladder(&[3, 3]);
        let h = flat(&items, 10.0);
        let vp = 20.0; // short, so there is plenty of room to scroll
        assert_eq!(step(&items, &h, vp, 0, 1), 2, "0 -> 2, skipping 1");
        assert_eq!(step(&items, &h, vp, 2, -1), 0, "and back again");
        // Two notches from the top clears the duplicate and lands on real ground.
        assert_eq!(step(&items, &h, vp, 0, 2), 3);
    }

    #[test]
    fn r3_6_a_document_that_fits_does_not_scroll() {
        // Everything is already on screen, so there is nothing to reach. This case is
        // checked before the D22 formula and overrides it — otherwise a fully visible
        // document could still be pushed up until only the last heading remained.
        let items = ladder(&[2, 2]);
        let h = flat(&items, 10.0);
        assert_eq!(max_index(&items, &h, 1000.0), 0);
        assert_eq!(
            step(&items, &h, 1000.0, 0, 5),
            0,
            "the wheel cannot move it"
        );
    }

    #[test]
    fn r3_5_the_last_sections_heading_can_always_reach_the_top() {
        // The motivating case for D22, and the one a sidebar-style clamp gets wrong: a long
        // document ending in a *short* section. `content_h - viewport_h` stops before the
        // final heading reaches the slot, so `Cmd+Shift+6` would move the view and still not
        // show TOOL USAGE at the top — D12 unsolved while looking implemented.
        let items = ladder(&[20, 2]); // 0:H0 1..20:rows 21:H1 22,23:rows
        let h = flat(&items, 10.0);
        let vp = 50.0; // five rungs
        let last_heading = jump_target(&items, 1).expect("section 1 exists");
        assert_eq!(last_heading, 21);

        // The tail-fits term alone would stop well short: from 19 the remaining rungs fit.
        let tail_only = (0..items.len())
            .find(|&i| tail_h(&h, content_start(&items, i)) <= vp + f32::EPSILON)
            .unwrap();
        assert!(
            tail_only < last_heading,
            "premise: the tail fits ({tail_only}) before the last heading ({last_heading})"
        );

        assert_eq!(max_index(&items, &h, vp), last_heading);
        assert_eq!(
            clamp(&items, &h, vp, last_heading),
            last_heading,
            "the jump target must survive the clamp, or the jump silently fails"
        );
    }

    #[test]
    fn r3_5_a_long_final_section_uses_the_tail_term_instead() {
        // The other side of the `max`: when the final section is longer than the viewport,
        // stopping at its heading would leave its own rows unreachable — R0's promise.
        let items = ladder(&[2, 20]);
        let h = flat(&items, 10.0);
        let vp = 50.0;
        let last_heading = jump_target(&items, 1).unwrap();
        let max = max_index(&items, &h, vp);
        assert!(
            max > last_heading,
            "max {max} must pass the final heading {last_heading} to reach its rows"
        );
        // And the very last rung is reachable: from `max` the tail fits.
        assert!(tail_h(&h, content_start(&items, max)) <= vp + f32::EPSILON);
    }

    #[test]
    fn r0_2_every_rung_is_reachable_by_stepping_from_the_top() {
        // The completion criterion in coordinate form: walking the stops from 0 to
        // `max_index` must expose every rung of the ladder at least once. A regression in
        // any of the three terms shows up here as an unreachable tail.
        let items = ladder(&[9, 7, 5, 11, 32, 4]);
        let h = flat(&items, 10.0);
        let vp = 60.0; // six rungs on screen
        let max = max_index(&items, &h, vp);
        let mut seen = vec![false; items.len()];
        let mut at = 0usize;
        loop {
            // What this frame shows: whole rungs from `content_start`, plus the slot.
            let start = content_start(&items, at);
            let mut used = 0.0;
            for (i, rh) in h.iter().enumerate().skip(start) {
                if used + rh > vp + f32::EPSILON {
                    break;
                }
                used += rh;
                seen[i] = true;
            }
            seen[at] = true; // the rung the slot is showing
            if at >= max {
                break;
            }
            let next = step(&items, &h, vp, at, 1);
            assert!(next > at, "step stalled at {at}");
            at = next;
        }
        let missed: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter(|(_, s)| !**s)
            .map(|(i, _)| i)
            .collect();
        assert!(missed.is_empty(), "unreachable rungs: {missed:?}");
    }

    #[test]
    fn r5_1_a_gesture_moves_the_view_and_banks_its_sub_rung_remainder() {
        // A trackpad delivers a few px per event. Truncating each independently would round
        // every one to zero rungs and the panel would look frozen — the residue is what
        // makes a slow drag work at all.
        let items = ladder(&[6]);
        let h = flat(&items, 10.0);
        let vp = 20.0;
        // Down (negative px) by half a rung: nothing moves, the half is banked.
        let (at, res) = scroll_by_px(&items, &h, vp, 0, -5.0, 0.0);
        assert_eq!(at, 0);
        assert_eq!(res, -5.0);
        // Another half completes the rung. Index 1 is not a stop (it is TASKS' first row),
        // so the step lands on 2.
        let (at, res) = scroll_by_px(&items, &h, vp, at, -5.0, res);
        assert_eq!(at, 2, "one rung of travel, one stop crossed");
        assert!(res.abs() < 1e-6, "and the bank is spent: {res}");
    }

    #[test]
    fn r5_3_and_r5_4_the_edge_releases_only_the_direction_that_pushes_into_it() {
        // C12 — the rule the naive version gets wrong. "At the end, hand the wheel to the
        // grid" read literally means the panel NEVER GETS IT BACK: scroll up and the panel
        // is still at the end, so the condition is still true. Direction is what makes the
        // release temporary.
        let items = ladder(&[8]);
        let h = flat(&items, 10.0);
        let vp = 20.0;
        let max = max_index(&items, &h, vp);
        assert!(max > 0, "premise: this document scrolls");

        // At the bottom, pushing further down does not move — the app reads that as
        // fall-through.
        let (at, res) = scroll_by_px(&items, &h, vp, max, -100.0, 0.0);
        assert_eq!(at, max, "the panel cannot go further down");
        assert_eq!(res, 0.0, "and the residue is dropped, not banked");

        // The very same position, pushed UP, still belongs to the panel.
        let (at, _) = scroll_by_px(&items, &h, vp, max, 10.0, 0.0);
        assert!(
            at < max,
            "upward at the bottom edge must still move the panel"
        );

        // Symmetrically at the top.
        let (at, _) = scroll_by_px(&items, &h, vp, 0, 100.0, 0.0);
        assert_eq!(at, 0);
        let (at, _) = scroll_by_px(&items, &h, vp, 0, -10.0, 0.0);
        assert!(at > 0, "downward at the top edge must still move the panel");
    }

    #[test]
    fn r5_1_travel_is_measured_in_pixels_and_spent_in_unequal_rungs() {
        // Cards are two lines, bars are one. Converting a gesture to a rung COUNT up front
        // would make one notch cross two bars or half a card depending on where the view
        // happened to be; spending pixels against real heights keeps the distance honest.
        let items = ladder(&[4]);
        let mut h = flat(&items, 10.0);
        h[2] = 40.0; // one tall rung in the middle
        let vp = 15.0;
        // 10px reaches stop 2 (heading is 10 tall, index 1 is not a stop).
        let (at, _) = scroll_by_px(&items, &h, vp, 0, -10.0, 0.0);
        assert_eq!(at, 2);
        // Crossing the tall rung costs 40px, not 10.
        let (at_short, _) = scroll_by_px(&items, &h, vp, 2, -20.0, 0.0);
        assert_eq!(at_short, 2, "20px is not enough to leave a 40px rung");
        let (at_long, _) = scroll_by_px(&items, &h, vp, 2, -40.0, 0.0);
        assert_eq!(at_long, 3, "40px is");
    }

    #[test]
    fn a_non_finite_delta_cannot_poison_the_residue() {
        // A poisoned bank fails silently for the rest of the session — nothing errors, the
        // panel simply stops answering the wheel — so the guard is worth its own test.
        let items = ladder(&[4]);
        let h = flat(&items, 10.0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let (at, res) = scroll_by_px(&items, &h, 20.0, 2, bad, 0.0);
            assert_eq!(res, 0.0, "delta {bad} banked something");
            assert!(is_stop(&items, at));
            let (_, res) = scroll_by_px(&items, &h, 20.0, 2, -5.0, bad);
            assert_eq!(res, 0.0, "residue {bad} survived");
        }
    }

    #[test]
    fn r0_2_an_oversized_final_rung_does_not_strand_the_rungs_before_it() {
        // A regression this file caused and did not notice: the degenerate guard once keyed
        // on "no index makes the TAIL fit" rather than "no RUNG fits". Those agree everywhere
        // except when the last rung alone is oversized — and there the tail-keyed guard
        // answers 0 and strands the whole document past the first screen.
        //
        // The shipped model cannot produce that shape (TOOL USAGE ends the document with the
        // shortest rung there is), which is exactly why it needs a test: `max_index` is `pub`,
        // takes the ladder as data (C11), and would fail silently the day a section is added
        // after TOOL USAGE.
        let items = ladder(&[8]);
        let mut h = flat(&items, 10.0);
        let last = h.len() - 1;
        h[last] = 100.0; // taller than any viewport this test uses
        let vp = 40.0;
        let max = max_index(&items, &h, vp);
        assert!(max > 0, "the document must still scroll (got max={max})");

        let mut seen = vec![false; items.len()];
        for top in (0..=max).filter(|&i| is_stop(&items, i)) {
            seen[top] = true;
            let start = content_start(&items, top);
            let mut used = 0.0;
            for (i, rh) in h.iter().enumerate().skip(start) {
                if used + rh > vp + f32::EPSILON {
                    break;
                }
                used += rh;
                seen[i] = true;
            }
        }
        // Every rung but the oversized one — that one is the documented Known Gap, and it was
        // unreachable before this guard existed too.
        let missed: Vec<usize> = (0..last).filter(|&i| !seen[i]).collect();
        assert!(missed.is_empty(), "stranded rungs: {missed:?}");
    }

    #[test]
    fn r3_7_a_degenerate_viewport_answers_zero_instead_of_panicking() {
        // The height sweep starts at 2px, so this is reached by the tests, not just in
        // theory: a panel whose section area cannot take one rung has no scroll space, and
        // `max_index`'s "first index whose tail fits" has no candidate at all.
        let items = ladder(&[3, 3]);
        let h = flat(&items, 10.0);
        for vp in [0.0f32, -5.0, f32::NAN, f32::INFINITY] {
            let max = max_index(&items, &h, vp);
            assert_eq!(max, 0, "vp {vp} produced {max}, not the degenerate answer");
            assert_eq!(clamp(&items, &h, vp, 99), max);
            assert_eq!(step(&items, &h, vp, 0, 3), max);
        }
        // And an empty ladder — the panel before its first model arrives.
        assert_eq!(max_index(&[], &[], 100.0), 0);
        assert_eq!(clamp(&[], &[], 100.0, 7), 0);
        assert!(!is_stop(&[], 0));
        assert_eq!(jump_target(&[], 0), None);
    }

    #[test]
    fn r3_3_content_starts_below_a_heading_the_slot_took() {
        // D31's consumption rule in its smallest form. The `+1` is what stops the same
        // heading being drawn twice — once in the slot and once directly under it.
        let items = ladder(&[2, 2]);
        assert_eq!(content_start(&items, 0), 1, "the slot took heading 0");
        assert_eq!(content_start(&items, 2), 2, "a row is not consumed");
        assert_eq!(content_start(&items, 3), 4, "the slot took heading 1");
    }

    #[test]
    fn clamping_lands_on_a_stop_never_between_two() {
        // Anything that produces an index — a restored offset, a jump, a re-clamp after the
        // document shrank — must land somewhere the wheel could also have reached, or the
        // next notch behaves as if it moved by two.
        let items = ladder(&[0, 5, 0]);
        let h = flat(&items, 10.0);
        let vp = 20.0;
        for i in 0..items.len() + 3 {
            let c = clamp(&items, &h, vp, i);
            assert!(is_stop(&items, c), "clamp({i}) = {c} is not a stop");
            assert!(c <= max_index(&items, &h, vp));
        }
    }
}
