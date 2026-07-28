//! ai-monitor dashboard layout — the app-side presentation policy (SD4/D21).
//!
//! `crates/ai-monitor` accumulates raw numbers; everything about how they *look*
//! lives here: which sections exist, their order, truncation, the per-model
//! context-window table, and the placeholder rules. The renderer is untouched
//! (SD3) — the output is a plain `Vec<String>` drawn as glyphs.
//!
//! Two constraints shape every decision in this module:
//!
//! * **The app is width-blind.** `OverlayModel` carries no width, `AppCtx` no
//!   columns, and the `Overlay` trait has no resize hook — the renderer computes a
//!   column budget at draw time and clips. So sections are **open-right** (a rule
//!   that simply ends early when clipped) rather than closed boxes, whose missing
//!   corner would be a conspicuous tell. Even a width channel would not fix it:
//!   nothing recomposes the overlay on resize, so any learned width goes stale at
//!   the first window drag.
//! * **Pure.** [`compose`] takes a snapshot and an injected `now_ms` and reads no
//!   clock, no filesystem, no globals — which is what makes the `insta` snapshots
//!   byte-stable.

use ai_monitor::telemetry::TelemetrySnapshot;

/// Nominal rule length. At the default 900x540 logical window with `size_pt: 14.0`
/// on a 2x display, `cw = round(14*2*0.6) = 17` and the full-width panel budgets
/// `floor((1800 - 2*10)/17) = 104` columns — so a rule terminates almost exactly at
/// the panel edge by default, runs short on a wider window, and is clipped mid-rule
/// on a narrower one. That is the least conspicuous direction to be wrong in: a rule
/// is self-similar, so a clipped one still reads as a rule.
pub const RULE_COLS: usize = 104;

/// Width of the left label gutter. A fixed *left* gutter needs no width knowledge,
/// unlike a right-aligned column.
const LABEL_COLS: usize = 8;

/// The placeholder for every unknown value (U+2014).
pub const DASH: &str = "—";

/// Per-model context windows (SD4 — app-side, so a new model needs no `ai-monitor`
/// change). Seeded from the model ids that actually appear in the logs, measured
/// over `message.model` across every file under `~/.claude/projects`:
/// `claude-opus-4-8` (24,693), `claude-opus-5` (4,020),
/// `claude-haiku-4-5-20251001` (123), `claude-fable-5` (61), `<synthetic>` (28).
///
/// Longest-prefix, then default — one rule, no second mechanism. Note there is no
/// suffix parsing: model ids carry no bracketed context-window marker in any of the
/// 28,925 logged values, so a `[1m]`-style rule would be unreachable code.
const MODEL_WINDOWS: &[(&str, u64)] = &[
    ("claude-opus-", 200_000),
    ("claude-sonnet-", 200_000),
    ("claude-haiku-", 200_000),
    ("claude-fable-", 200_000),
];

/// The window for `model`, or `None` when no prefix matches.
///
/// `None` is load-bearing: the caller omits the percentage entirely rather than
/// assuming a denominator. A confident percentage against a guessed window is a
/// false statement about how much room the session has left, and `<synthetic>`
/// matches nothing.
pub fn context_window(model: &str) -> Option<u64> {
    MODEL_WINDOWS
        .iter()
        .filter(|(p, _)| model.starts_with(p))
        .max_by_key(|(p, _)| p.len())
        .map(|(_, w)| *w)
}

/// Compose the dashboard for a snapshot at `now_ms`.
///
/// Pure: the clock is a parameter, not a call.
///
/// `None` — no log resolved for this tab — renders the *same* frame as a snapshot
/// with nothing folded into it yet (F3a's empty example). The two are the same
/// statement to a reader ("nothing known"), and rendering nothing at all would flash
/// the panel empty on every open.
pub fn compose(snap: Option<&TelemetrySnapshot>, now_ms: u64) -> Vec<String> {
    match snap {
        Some(t) => summary(t, now_ms),
        None => summary(&TelemetrySnapshot::default(), now_ms),
    }
}

/// The summary block: rule, session/model/context, spend, activity.
///
/// `overlay-dashboard-boxes` composes this LAST, beneath its sections — the panel
/// keeps the *tail* of the output, so a summary placed first is the first thing
/// clipped on a short window.
pub fn summary(t: &TelemetrySnapshot, now_ms: u64) -> Vec<String> {
    let mut lines = Vec::with_capacity(4);
    lines.push(rule("ai-monitor"));

    // session id · model · context%
    let mut ident = vec![
        t.session_id
            .as_deref()
            .map(|s| s.chars().take(8).collect::<String>())
            .unwrap_or_else(|| DASH.to_string()),
        t.model.clone().unwrap_or_else(|| DASH.to_string()),
    ];
    // Omitted entirely — label and all — when the model is unmatched or the gauge
    // is zero. A fresh session's occupancy is unknown, not 0%.
    if let Some(pct) = context_pct(t) {
        ident.push(format!("context {pct}%"));
    }
    lines.push(row("session", &ident.join(" · "), None));

    // Cumulative COST counters, never presented as occupancy: cache-read is
    // re-billed every turn, so these run ~16x the live context on a real session.
    lines.push(row(
        "spend",
        &format!(
            "in {} · out {} · cache {}",
            tok(t.total_input),
            tok(t.total_output),
            tok(t.total_cache_read.saturating_add(t.total_cache_creation))
        ),
        Some("billed"),
    ));

    // Both values freeze between snapshots — nothing recomposes the overlay on a
    // timer — so they are framed as a reading, not a live counter.
    let activity = match (t.first_event_ms, t.last_event_ms) {
        (Some(f), Some(l)) => format!(
            "up {} · idle {}",
            dur(now_ms.saturating_sub(f)),
            dur(now_ms.saturating_sub(l))
        ),
        _ => DASH.to_string(),
    };
    lines.push(row("activity", &activity, Some("as of last update")));
    lines
}

/// Context occupancy as a whole percentage, or `None` when it cannot be stated.
pub fn context_pct(t: &TelemetrySnapshot) -> Option<u64> {
    if t.context_used == 0 {
        return None;
    }
    let window = context_window(t.model.as_deref()?)?;
    if window == 0 {
        return None;
    }
    // Round-half-up without floats, then floor at 1: `0%` is reserved for "unknown"
    // (F7/R6.2 omit the label entirely there), so a displayed percentage always
    // means real, non-zero occupancy. Rounding a live gauge down to the one value
    // that elsewhere means "no reading" is the one ambiguity worth spending a
    // fraction of a percent to avoid.
    let pct = (t.context_used.saturating_mul(200) / window).div_ceil(2);
    Some(pct.max(1))
}

/// `── label ─────…` to `RULE_COLS`. Open on the right: no closing glyph to lose.
pub fn rule(label: &str) -> String {
    rule_with(label, None)
}

/// A rule carrying an optional marker next to its label (e.g. `+7 more`), which is
/// where a marker must live: at the tail it would be clipped on exactly the narrow
/// windows that make truncation likely.
pub fn rule_with(label: &str, marker: Option<&str>) -> String {
    let head = match marker {
        Some(m) => format!("── {label} {m} "),
        None => format!("── {label} "),
    };
    let used = head.chars().count();
    format!("{head}{}", "─".repeat(RULE_COLS.saturating_sub(used)))
}

/// A labelled row with an optional right-hand qualifier, one space after the value.
/// Never padded to a column — aligning would be the fixed-width assumption the
/// open-right design rejects, and it would break the moment a value changed length.
fn row(label: &str, value: &str, qualifier: Option<&str>) -> String {
    let mut s = format!(" {:<w$} {value}", label, w = LABEL_COLS);
    if let Some(q) = qualifier {
        s.push_str(&format!(" ({q})"));
    }
    s
}

/// A billed counter, or `—` when it is zero.
///
/// Zero and "not yet known" are indistinguishable in this data, and a real session
/// never bills zero input or zero output tokens — so the placeholder is the honest
/// reading (F7 rejects showing `0` for an unknown). The only case that reaches zero
/// legitimately is a session with nothing folded, which is exactly the empty state.
fn tok(n: u64) -> String {
    if n == 0 {
        DASH.to_string()
    } else {
        num(n)
    }
}

/// `45` / `19.3K` / `1.9M` — one decimal above each threshold, bare below 1000.
/// Thousands separators are locale-dependent and wider.
pub fn num(n: u64) -> String {
    // The threshold is the value that *rounds* to 1.0M, not 1M itself: `{:.1}`
    // rounds after the branch is chosen, so a plain `>= 1_000_000` prints
    // `1000.0K` for the 50-value band below it — a band a cumulative cache
    // counter passes straight through.
    if n >= 999_950 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// `12s` / `2m03s` / `1h05m`.
pub fn dur(ms: u64) -> String {
    let s = ms / 1000;
    let (m, h) = (s / 60, s / 3600);
    if h > 0 {
        format!("{h}h{:02}m", m % 60)
    } else if m > 0 {
        format!("{m}m{:02}s", s % 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A populated snapshot: claude-opus-5, 117,066 of a 200k window, 45m12s uptime,
    /// 2m03s since the last event, at a fixed `now_ms`.
    fn populated() -> (TelemetrySnapshot, u64) {
        let now = 1_785_226_000_000;
        let t = TelemetrySnapshot {
            session_id: Some("5e59b629-d34a-4ab6".into()),
            model: Some("claude-opus-5".into()),
            total_input: 45,
            total_output: 19_348,
            total_cache_read: 1_900_000,
            total_cache_creation: 12_000,
            context_used: 117_066,
            first_event_ms: Some(now - (45 * 60 + 12) * 1000),
            last_event_ms: Some(now - (2 * 60 + 3) * 1000),
            ..TelemetrySnapshot::default()
        };
        (t, now)
    }

    #[test]
    fn populated_summary_matches_its_pinned_form() {
        let (t, now) = populated();
        insta::assert_snapshot!(summary(&t, now).join("\n"));
    }

    #[test]
    fn empty_snapshot_renders_the_frame_with_placeholders() {
        let t = TelemetrySnapshot::default();
        insta::assert_snapshot!(summary(&t, 1_785_226_000_000).join("\n"));
    }

    #[test]
    fn an_absent_snapshot_renders_the_same_frame_as_an_empty_one() {
        // R6.1 — "no log resolved" and "nothing folded yet" are one statement to a
        // reader. Never a blank panel.
        let now = 1_785_226_000_000;
        assert_eq!(
            compose(None, now),
            summary(&TelemetrySnapshot::default(), now)
        );
        assert_eq!(compose(None, now).len(), 4, "the full frame, not a stub");
    }

    #[test]
    fn every_rule_is_exactly_rule_cols_wide() {
        let (t, now) = populated();
        for line in summary(&t, now).iter().filter(|l| l.starts_with("──")) {
            assert_eq!(line.chars().count(), RULE_COLS, "{line}");
        }
        assert_eq!(
            rule_with("Other Tools", Some("+7 more")).chars().count(),
            RULE_COLS
        );
    }

    #[test]
    fn a_rule_never_carries_a_closing_glyph() {
        // Open-right: clipping a rule must leave a valid rule, never an orphaned corner.
        let r = rule("ai-monitor");
        assert!(r.ends_with('─'));
        assert!(!r.contains('╗') && !r.contains('┐') && !r.contains('│'));
    }

    #[test]
    fn context_window_is_longest_prefix_then_default() {
        assert_eq!(context_window("claude-opus-5"), Some(200_000));
        assert_eq!(context_window("claude-opus-4-8"), Some(200_000));
        assert_eq!(context_window("claude-haiku-4-5-20251001"), Some(200_000));
        assert_eq!(context_window("claude-fable-5"), Some(200_000));
        // The one observed value matching nothing — and the reason `None` exists.
        assert_eq!(context_window("<synthetic>"), None);
        assert_eq!(context_window("opus"), None);
    }

    #[test]
    fn an_unmatched_model_omits_the_percentage_entirely() {
        let (mut t, now) = populated();
        t.model = Some("<synthetic>".into());
        let s = summary(&t, now).join("\n");
        assert!(!s.contains("context"), "no label, not 0% and not —%:\n{s}");
        assert!(s.contains("<synthetic>"), "the model is still named");
    }

    #[test]
    fn a_zero_gauge_omits_the_percentage() {
        let (mut t, now) = populated();
        t.context_used = 0;
        assert!(!summary(&t, now).join("\n").contains("context"));
    }

    #[test]
    fn context_percent_rounds_to_the_nearest_whole() {
        let (mut t, _) = populated();
        assert_eq!(context_pct(&t), Some(59), "117066/200000 = 58.5%");
        t.context_used = 100_000;
        assert_eq!(context_pct(&t), Some(50));
        t.context_used = 1;
        assert_eq!(context_pct(&t), Some(1), "a non-zero gauge never reads 0%");
    }

    #[test]
    fn spend_is_labelled_billed_and_is_never_a_percentage() {
        let (t, now) = populated();
        let s = summary(&t, now).join("\n");
        assert!(s.contains("(billed)"));
        // cache = read + creation, summed because both are billed
        assert!(s.contains("cache 1.9M"), "{s}");
        // the ONLY percentage is the context gauge
        assert_eq!(s.matches('%').count(), 1);
    }

    #[test]
    fn activity_carries_the_staleness_qualifier() {
        let (t, now) = populated();
        let s = summary(&t, now).join("\n");
        assert!(s.contains("up 45m12s · idle 2m03s"));
        assert!(s.contains("(as of last update)"), "not a live counter");
    }

    #[test]
    fn missing_event_times_render_a_dash_not_an_epoch() {
        let (mut t, now) = populated();
        t.first_event_ms = None;
        t.last_event_ms = None;
        let s = summary(&t, now).join("\n");
        assert!(s.contains(&format!("activity {DASH}")) || s.contains(DASH));
        assert!(!s.contains("up "), "no epoch-derived duration:\n{s}");
    }

    #[test]
    fn the_composer_is_pure() {
        // Same inputs -> byte-identical output. This is what makes the snapshots
        // stable, and it only holds because the clock is a parameter.
        let (t, now) = populated();
        assert_eq!(summary(&t, now), summary(&t, now));
    }

    #[test]
    fn number_and_duration_formatting() {
        assert_eq!(num(45), "45");
        assert_eq!(num(999), "999");
        assert_eq!(num(19_348), "19.3K");
        assert_eq!(num(1_912_000), "1.9M");
        // the rounding band at the magnitude boundary — never "1000.0K"
        assert_eq!(num(999_949), "999.9K");
        assert_eq!(num(999_950), "1.0M");
        assert_eq!(num(1_000_000), "1.0M");
        assert_eq!(dur(12_000), "12s");
        assert_eq!(dur((2 * 60 + 3) * 1000), "2m03s");
        assert_eq!(dur((65 * 60) * 1000), "1h05m");
    }
}
