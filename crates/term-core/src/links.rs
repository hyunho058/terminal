//! Hyperlink detection over a [`GridSnapshot`] (spec `hyperlinks`, D2–D5, D12,
//! D13, D16, D23, D25).
//!
//! One entry point — [`link_at`] — resolves a single cell to the URL that covers
//! it, or `None`. Both callers use it: the app lights the returned span while ⌘ is
//! held, and opens that same span's `url` on release. Sharing one function is what
//! mechanically guarantees D20 ("what is underlined is exactly what opens") — there
//! is no second code path that could drift.
//!
//! GPU-free and window-free (C4): a `GridSnapshot` is the only input, so every rule
//! below is unit-testable with no display, no PTY, and no event loop.
//!
//! ## The rules, in the order they apply
//!
//! 1. **Scope** (D21): only the hovered row's contiguous *soft-wrapped run* is
//!    examined — never the whole grid. A URL that wrapped across the right edge is
//!    stitched back together (D5) using the same heuristic `selection_text` uses.
//! 2. **Scheme** (D2): a candidate starts at `http://` or `https://` (ASCII
//!    case-insensitive — RFC 3986 makes the scheme case-insensitive) and only at a
//!    token boundary, so `xhttps://…` is not a link.
//! 3. **Run** (D23): the candidate extends over ASCII *graphic* characters
//!    (`0x21..=0x7E`) and stops at the first space, C0/DEL byte, or non-ASCII
//!    character.
//! 4. **Trim** (D4): trailing punctuation is removed iteratively; a closing bracket
//!    survives when its opener appears inside the URL.
//! 5. **Reject** (D3): the candidate is dropped *entirely* — not truncated — when
//!    its authority is empty, contains userinfo (`@`), or when rule 3 cut the run
//!    with a hostile byte while still inside the authority. That last clause is what
//!    turns `https://gıthub.com/…` into "no link" rather than the perfectly
//!    clickable fragment `https://g`.
//!
//! ## Known limitation: a full-width hard line suppresses the next row's link
//!
//! `row_soft_wraps` cannot tell a genuine soft wrap from a hard line that happens to
//! fill the row exactly, so both join the next row into one run. When the joined
//! row's last character is alphanumeric, rule 2's token-boundary check then reads the
//! next row's `https://` as mid-token and the link **silently fails to appear**:
//!
//! ```text
//! row 0: abcdefghijklmnopqrst   (exactly `cols` wide, a hard line)
//! row 1: https://a.com/x        → no link
//! ```
//!
//! This is *not* the mis-stitch the spec's Known Gaps anticipate — it is the inverse,
//! a false negative, and D20 ("what is underlined is what opens") mitigates nothing
//! about a link that never lights.
//!
//! It is left unfixed on purpose. The boundary check is what stops the *other* half
//! of the same ambiguity: with it removed, `see https://a.co` filling a row followed
//! by a hard-broken `mmand output` resolves to `https://a.command` — a URL that
//! exists on no line the user can see. Given a heuristic that cannot distinguish the
//! two cases, suppressing a real link is the safer error than fabricating one.
//!

use crate::snapshot::GridSnapshot;

/// The allow-listed schemes (D2). Ordered longest-first so `https` is tried before
/// `http` — otherwise `https://x` would match `http` and then fail at `s`.
const SCHEMES: [&str; 2] = ["https://", "http://"];

/// A resolved hyperlink: the cells it occupies plus the URL those cells spell.
///
/// `start`/`end` are **inclusive** viewport `(col, row)` coordinates, ordered in
/// reading order, and may span two rows when the URL soft-wrapped (D5). The shape
/// mirrors [`crate::Selection`] (D13) so hit-testing and highlighting reuse the same
/// linear-span reasoning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub start: (u16, u16),
    pub end: (u16, u16),
    /// The exact text of the cells in `start..=end`. This is the string that gets
    /// opened; nothing re-derives it (D20).
    pub url: String,
}

impl LinkSpan {
    /// Whether cell `(col, row)` lies inside this span on a `cols`-wide grid.
    ///
    /// Linearizes to a scalar index so the span is one range test — the same
    /// technique (and the same never-panics-on-edge-coords property) as
    /// [`crate::Selection::contains`].
    pub fn contains(&self, col: u16, row: u16, cols: u16) -> bool {
        let cols = cols.max(1) as u64;
        let idx = row as u64 * cols + col as u64;
        let s = self.start.1 as u64 * cols + self.start.0 as u64;
        let e = self.end.1 as u64 * cols + self.end.0 as u64;
        idx >= s && idx <= e
    }
}

/// Resolve the cell at `(col, row)` to the link covering it, or `None`.
///
/// Pure: same snapshot + same coordinates always yields the same value, with no
/// observable side effect (R1.17).
pub fn link_at(snap: &GridSnapshot, col: u16, row: u16) -> Option<LinkSpan> {
    if col >= snap.cols || row >= snap.rows {
        return None;
    }
    // A double-width spacer holds no character of its own, and a URL can never
    // contain a wide char anyway (rule 3 stops at the non-ASCII leader).
    if snap.cell(col, row).is_none_or(|c| c.is_continuation()) {
        return None;
    }

    let (chars, coords) = wrap_run(snap, row);
    let target = coords.iter().position(|&c| c == (col, row))?;

    let mut i = 0;
    while i < chars.len() {
        let Some(scheme_len) = scheme_at(&chars, i) else {
            i += 1;
            continue;
        };
        // Candidates are scanned left to right, so once one starts past the queried
        // cell no later one can contain it either.
        if i > target {
            return None;
        }
        let (end, hostile_cut) = run_end(&chars, i);
        let trimmed = trim_end(&chars[i..end]) + i;

        if target >= i
            && target < trimmed
            && is_valid(&chars[i..trimmed], &chars[i..end], scheme_len, hostile_cut)
        {
            return Some(LinkSpan {
                start: coords[i],
                end: coords[trimmed - 1],
                url: chars[i..trimmed].iter().collect(),
            });
        }
        // Past this candidate (valid or rejected): a rejected candidate is never
        // retried at an inner offset, which is what makes D3 a *rejection* rather
        // than a truncation.
        i = end.max(i + 1);
    }
    None
}

/// The characters of the soft-wrapped run containing `row`, plus each character's
/// cell coordinate (D5, D21).
///
/// A row continues into the next iff its last column holds a non-blank,
/// non-continuation cell — [`GridSnapshot::row_soft_wraps`], the same heuristic
/// `selection_text` uses for copy. Double-width spacers are skipped, so no
/// `Cell::CONTINUATION` sentinel can leak into a URL (R1.15).
fn wrap_run(snap: &GridSnapshot, row: u16) -> (Vec<char>, Vec<(u16, u16)>) {
    let mut first = row;
    while first > 0 && snap.row_soft_wraps(first - 1) {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < snap.rows && snap.row_soft_wraps(last) {
        last += 1;
    }

    let cap = (last - first + 1) as usize * snap.cols as usize;
    let mut chars = Vec::with_capacity(cap);
    let mut coords = Vec::with_capacity(cap);
    for r in first..=last {
        for c in 0..snap.cols {
            match snap.cell(c, r) {
                Some(cell) if cell.is_continuation() => {}
                Some(cell) => {
                    chars.push(cell.ch);
                    coords.push((c, r));
                }
                None => {}
            }
        }
    }
    (chars, coords)
}

/// If an allow-listed scheme starts at `i` *at a token boundary*, its length (D2).
///
/// The boundary check is what keeps `xhttps://evil` from resolving: the character
/// before the scheme must not itself be a scheme character.
fn scheme_at(chars: &[char], i: usize) -> Option<usize> {
    if i > 0 && is_scheme_char(chars[i - 1]) {
        return None;
    }
    SCHEMES.iter().find_map(|scheme| {
        let n = scheme.len();
        let matches = chars.len() >= i + n
            && scheme
                .chars()
                .zip(&chars[i..i + n])
                .all(|(a, b)| b.eq_ignore_ascii_case(&a));
        matches.then_some(n)
    })
}

/// What may *precede* a scheme without it counting as a token boundary.
///
/// Deliberately **narrower** than the RFC 3986 scheme character set, which also
/// admits `.`, `+` and `-`. Every one of those three suppressed a real link and
/// bought nothing, because D2's allow-list already refuses anything but `http`/
/// `https`. A compound scheme like `svn+https://a.com/x` therefore resolves to its
/// inner `https://a.com/x` rather than being rejected — acceptable, because what
/// reaches the OS is still on the allow-list and D20 still holds (only the
/// `https://…` cells are underlined). What the three characters actually cost was:
/// `...https://…` (ellipsis-truncated output) and `+https://…` / `-https://…` (a URL
/// on an added or removed line of `git diff` output) all resolved to nothing.
/// Alphanumerics are the whole of the guard's real job — stopping `xhttps://evil`.
fn is_scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

/// Extend the run from `i` over ASCII graphic characters (D23).
///
/// Returns the exclusive end index and whether the run was cut by a **hostile**
/// byte — non-ASCII, C0, or DEL — as opposed to ending at a space or at the end of
/// the run. D3 clause (b) needs that distinction.
fn run_end(chars: &[char], i: usize) -> (usize, bool) {
    let mut j = i;
    while j < chars.len() && chars[j].is_ascii_graphic() {
        j += 1;
    }
    // `j` stopped at a non-graphic character (or the end), so anything other than a
    // plain space there is non-ASCII, C0, or DEL.
    let hostile = chars.get(j).is_some_and(|c| *c != ' ');
    (j, hostile)
}

/// Iteratively strip trailing punctuation (D4), returning the new length.
///
/// `.,;:!?"'` always go. A closing bracket goes only when its opener does **not**
/// appear earlier in the URL — so `…/pull/12).` loses both characters while
/// `…/wiki/Foo_(bar)` keeps its paren.
fn trim_end(url: &[char]) -> usize {
    let mut k = url.len();
    while k > 0 {
        let drop = match url[k - 1] {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' => true,
            ')' => !url[..k - 1].contains(&'('),
            ']' => !url[..k - 1].contains(&'['),
            '}' => !url[..k - 1].contains(&'{'),
            '>' => !url[..k - 1].contains(&'<'),
            _ => false,
        };
        if !drop {
            break;
        }
        k -= 1;
    }
    k
}

/// D3: is this candidate acceptable at all?
///
/// The authority is everything after `://` up to — exclusive — the first `/`, `?`,
/// or `#`, or the end of the run. It must be non-empty, must not carry a userinfo
/// component, and must not be where a hostile byte cut the run short.
///
/// Two slices, deliberately, because the two questions have different scopes:
///
/// - **Did the hostile cut land inside the authority?** Asked of `run` — the
///   *untrimmed* candidate — because D3 says "…or the end of **the run**". Asking it
///   of the trimmed candidate is wrong whenever trimming eats the very delimiter
///   that ended the authority: `https://example.com?│` loses its `?` to D4, and the
///   trimmed text then looks like a bare authority cut short by `│` — so the whole
///   link was rejected even though the authority (`example.com`) had completed
///   before the cut. That is a false negative on exactly the shape D3's own example
///   is about.
/// - **Is the authority well-formed?** Asked of `url` — the *trimmed* candidate —
///   because that is what would actually be opened. `https://.` trims to `https://`,
///   whose authority is empty; validating the untrimmed text would let it through.
fn is_valid(url: &[char], run: &[char], scheme_len: usize, hostile_cut: bool) -> bool {
    let (Some(rest), Some(run_rest)) = (url.get(scheme_len..), run.get(scheme_len..)) else {
        return false;
    };
    let delimits = |c: &char| matches!(c, '/' | '?' | '#');
    // A hostile byte is only fatal while the authority is still open — i.e. when no
    // delimiter closed it anywhere in the run. Otherwise the cut merely ended the
    // path/query, which D23 treats as an ordinary terminator.
    if hostile_cut && !run_rest.iter().any(delimits) {
        return false;
    }
    let authority = match rest.iter().position(delimits) {
        Some(d) => &rest[..d],
        None => rest,
    };
    !authority.is_empty() && !authority.contains(&'@')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Terminal};

    /// A snapshot of `text` rendered into a `cols × rows` terminal.
    fn snap(cols: u16, rows: u16, text: &str) -> GridSnapshot {
        let mut t = Terminal::new(cols, rows, Color(0xE0E0E0), Color(0x101010));
        t.advance(text.as_bytes());
        t.snapshot()
    }

    /// The URL resolved at `(col, row)`, if any — the shape most tests assert on.
    fn url_at(s: &GridSnapshot, col: u16, row: u16) -> Option<String> {
        link_at(s, col, row).map(|l| l.url)
    }

    /// Every column of `row` that resolves to nothing (used by the "no link
    /// anywhere on this row" tests, which must hold for the *whole* row).
    fn none_across_row(s: &GridSnapshot, row: u16) -> bool {
        (0..s.cols).all(|c| link_at(s, c, row).is_none())
    }

    #[test]
    fn cell_inside_a_url_resolves_to_that_url_and_span() {
        // R1.1: `see ` occupies cols 0-3, so the 21-char URL is cols 4..=24.
        let s = snap(40, 3, "see https://example.com/a now");
        let link = link_at(&s, 8, 0).expect("column 8 is inside the URL");
        assert_eq!(link.url, "https://example.com/a");
        assert_eq!(link.start, (4, 0));
        assert_eq!(link.end, (24, 0));
        // Both edges of the span resolve to the same link; the cells around it do not.
        assert_eq!(link_at(&s, 4, 0).as_ref(), Some(&link));
        assert_eq!(link_at(&s, 24, 0).as_ref(), Some(&link));
        assert!(
            link_at(&s, 25, 0).is_none(),
            "the space after is not a link"
        );
    }

    #[test]
    fn cell_outside_any_url_resolves_to_none() {
        // R1.2.
        let s = snap(40, 3, "see https://example.com/a now");
        assert!(link_at(&s, 1, 0).is_none(), "inside the word `see`");
        assert!(link_at(&s, 3, 0).is_none(), "the space before the URL");
        assert!(link_at(&s, 0, 1).is_none(), "a blank row");
    }

    #[test]
    fn non_allow_listed_schemes_are_not_links() {
        // R1.3 / R6.3: file, mailto, ftp — none of them, at any column.
        let s = snap(
            60,
            3,
            "file:///Users/me/.ssh/id_rsa mailto:a@b.com ftp://h/x",
        );
        assert!(none_across_row(&s, 0));
    }

    #[test]
    fn a_run_ends_at_a_non_ascii_char() {
        // R1.4: the lazygit case — a URL abutting a box-drawing border.
        let s = snap(40, 3, "https://example.com/a│b");
        assert_eq!(
            url_at(&s, 8, 0).as_deref(),
            Some("https://example.com/a"),
            "the │ is excluded"
        );
        let link = link_at(&s, 8, 0).expect("resolves");
        assert_eq!(link.end, (20, 0), "span stops before the border");
    }

    #[test]
    fn run_ends_at_whitespace_and_at_control_bytes() {
        // R1.4 (the other two terminators).
        let s = snap(40, 3, "https://example.com/a b");
        assert_eq!(url_at(&s, 0, 0).as_deref(), Some("https://example.com/a"));
        // A tab is a C0 byte; the grid renders it as advancement, so assert on the
        // resolver's own rule instead of on a rendered tab.
        let chars: Vec<char> = "https://example.com/a\tb".chars().collect();
        let (end, hostile) = run_end(&chars, 0);
        assert_eq!(end, 21);
        assert!(hostile, "a C0 byte is a hostile cut");
    }

    #[test]
    fn trailing_punctuation_is_trimmed_iteratively() {
        // R1.5: both the `)` and the `.` go.
        let s = snap(50, 3, "(see https://example.com/a/12).");
        assert_eq!(
            url_at(&s, 10, 0).as_deref(),
            Some("https://example.com/a/12")
        );
    }

    #[test]
    fn a_closing_bracket_with_a_matching_opener_is_kept() {
        // R1.6.
        let s = snap(60, 3, "https://en.wikipedia.org/wiki/Foo_(bar)");
        assert_eq!(
            url_at(&s, 5, 0).as_deref(),
            Some("https://en.wikipedia.org/wiki/Foo_(bar)")
        );
    }

    #[test]
    fn an_authority_with_userinfo_is_rejected_outright() {
        // R1.7: the classic phishing form — no link at all, not a truncated one.
        let s = snap(50, 3, "https://github.com@evil.com/pull/1");
        assert!(none_across_row(&s, 0));
    }

    #[test]
    fn an_at_sign_after_the_authority_is_legal() {
        // R1.8.
        let s = snap(40, 3, "https://example.com/a@b");
        assert_eq!(url_at(&s, 5, 0).as_deref(), Some("https://example.com/a@b"));
    }

    #[test]
    fn a_non_ascii_byte_inside_the_authority_rejects_the_candidate() {
        // R1.9: U+0131 dotless i. Truncation alone would leave `https://g` — a
        // perfectly clickable link to nowhere the user meant.
        let s = snap(40, 3, "https://gıthub.com/pull/1");
        assert!(
            none_across_row(&s, 0),
            "no link, and specifically not https://g"
        );
    }

    #[test]
    fn a_non_ascii_byte_after_the_authority_merely_ends_the_run() {
        // R1.10: the authority completed, so the cut is benign.
        let s = snap(40, 3, "https://example.com/aı");
        assert_eq!(url_at(&s, 5, 0).as_deref(), Some("https://example.com/a"));
    }

    #[test]
    fn an_empty_authority_is_rejected() {
        // R1.11.
        let s = snap(40, 3, "https:// and https:///p");
        assert!(none_across_row(&s, 0));
    }

    #[test]
    fn a_soft_wrapped_url_is_rejoined_into_one_link() {
        // R1.12: 18 columns splits the URL as `https://example.co` / `m/pull/9 done`.
        let s = snap(18, 4, "https://example.com/pull/9 done");
        assert_eq!(
            s.cell(17, 0).map(|c| c.ch),
            Some('o'),
            "row 0 fills its last column"
        );
        let link = link_at(&s, 3, 0).expect("resolves from the first half");
        assert_eq!(link.url, "https://example.com/pull/9");
        assert_eq!(link.start, (0, 0));
        assert_eq!(link.end, (7, 1), "the span reaches into the second row");
    }

    #[test]
    fn either_half_of_a_wrapped_url_resolves_to_the_same_link() {
        // R1.13.
        let s = snap(18, 4, "https://example.com/pull/9 done");
        assert_eq!(link_at(&s, 3, 0), link_at(&s, 2, 1));
    }

    #[test]
    fn a_hard_wrapped_row_does_not_stitch() {
        // R1.14: row 0's last column is blank, so row 1 never joins the URL.
        let s = snap(20, 4, "https://a.com\r\nfoo/bar");
        assert!(!s.row_soft_wraps(0), "the row does not soft-wrap");
        assert_eq!(url_at(&s, 3, 0).as_deref(), Some("https://a.com"));
    }

    #[test]
    fn double_width_continuation_cells_never_enter_a_url() {
        // R1.15: the wide leader is non-ASCII so it ends the run, and its spacer
        // cell contributes no character at all.
        let s = snap(40, 3, "https://example.com/한글");
        let url = url_at(&s, 5, 0).expect("resolves");
        assert_eq!(url, "https://example.com/");
        assert!(!url.contains(crate::Cell::CONTINUATION));
    }

    #[test]
    fn resolution_works_on_a_history_composed_snapshot() {
        // R1.16 (D8): the same URL resolves identically once it is scrolled into
        // history, because a snapshot composes its rows from history + live.
        let mut t = Terminal::new(40, 4, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"https://example.com/pull/7\r\n");
        for i in 0..10 {
            t.advance(format!("filler{i}\r\n").as_bytes());
        }
        t.scroll_up(11);
        let s = t.snapshot();
        assert!(s.view_offset > 0, "the viewport is in history");
        let (row, _) = (0..s.rows)
            .find_map(|r| url_at(&s, 3, r).map(|u| (r, u)))
            .expect("the URL is on screen somewhere");
        assert_eq!(
            url_at(&s, 3, row).as_deref(),
            Some("https://example.com/pull/7")
        );
    }

    #[test]
    fn the_resolver_is_pure() {
        // R1.17: no window, no GPU, no PTY — and two calls agree.
        let s = snap(40, 3, "see https://example.com/a now");
        assert_eq!(link_at(&s, 8, 0), link_at(&s, 8, 0));
    }

    #[test]
    fn a_scheme_inside_a_longer_token_is_not_a_link() {
        // The token-boundary half of D2: `xhttps://` must not resolve, while a URL
        // preceded by punctuation still does.
        let s = snap(60, 3, "xhttps://evil.com foo=https://ok.com/p");
        assert!(
            link_at(&s, 3, 0).is_none(),
            "no link inside xhttps://evil.com"
        );
        assert_eq!(url_at(&s, 25, 0).as_deref(), Some("https://ok.com/p"));
    }

    #[test]
    fn a_delimiter_that_trimming_removes_still_closes_the_authority() {
        // Regression (Gate-3 checker, defect A): the `?` closes the authority, then
        // D4 trims it away. Validating the *trimmed* text made the `│` cut look like
        // it landed inside a bare authority, and D3 rejected a perfectly good link.
        let s = snap(40, 3, "https://example.com?│");
        assert_eq!(url_at(&s, 5, 0).as_deref(), Some("https://example.com"));
        // The neighbouring shapes it must not disturb.
        let keep = snap(40, 3, "https://example.com?q│");
        assert_eq!(
            url_at(&keep, 5, 0).as_deref(),
            Some("https://example.com?q")
        );
        let slash = snap(40, 3, "https://example.com/│");
        assert_eq!(
            url_at(&slash, 5, 0).as_deref(),
            Some("https://example.com/")
        );
        // …and the rejection it must still perform: no delimiter anywhere in the run,
        // so the cut really is inside the authority.
        let reject = snap(40, 3, "https://gıthub.com/pull/1");
        assert!(none_across_row(&reject, 0));
        // Trimming that empties the authority is still caught, because the
        // well-formedness half is asked of the trimmed text.
        let empty = snap(40, 3, "https://. rest");
        assert!(none_across_row(&empty, 0));
    }

    #[test]
    fn punctuation_abutting_the_scheme_does_not_suppress_the_link() {
        // Regression (Gate-3 checker, defects C and 2): `.`, `+` and `-` all used to
        // count as scheme characters, so ellipsis-truncated output and — worse — a URL
        // on any added or removed line of a `git diff` swallowed the link entirely.
        for line in [
            "...https://a.com/x",
            "+https://a.com/x",
            "-https://a.com/x",
            "1.https://a.com/x",
        ] {
            let s = snap(40, 3, line);
            let col = line.find("https").expect("scheme present") as u16 + 3;
            assert_eq!(
                url_at(&s, col, 0).as_deref(),
                Some("https://a.com/x"),
                "in {line:?}"
            );
        }
        // The guard this rule exists for still holds: an alphanumeric prefix is
        // still mid-token, so `xhttps://` is not a link.
        let guard = snap(40, 3, "xhttps://evil.com");
        assert!(none_across_row(&guard, 0));
    }

    #[test]
    fn a_full_width_hard_line_suppresses_the_next_rows_link() {
        // Documents the module's stated known limitation rather than asserting a
        // behaviour we want — the heuristic cannot tell this from a real wrap. Pinned
        // so that a future change to `scheme_at` or `row_soft_wraps` surfaces here
        // instead of silently flipping to the *other* failure (a fabricated URL).
        let suppressed = snap(20, 4, "abcdefghijklmnopqrsthttps://a.com/x");
        assert!(
            suppressed.row_soft_wraps(0),
            "row 0 looks wrapped to the heuristic"
        );
        assert!(
            link_at(&suppressed, 3, 1).is_none(),
            "known limitation: the joined alphanumeric tail hides the scheme"
        );
        // One character narrower — a real blank at the row end — and it resolves.
        let ok = snap(20, 4, "abcdefghijklmnopqrs\r\nhttps://a.com/x");
        assert_eq!(url_at(&ok, 3, 1).as_deref(), Some("https://a.com/x"));
    }

    #[test]
    fn the_scheme_match_is_case_insensitive() {
        // RFC 3986 makes the scheme case-insensitive; D2's allow-list is about
        // *which* scheme, not its spelling.
        let s = snap(40, 3, "HTTPS://Example.com/A");
        assert_eq!(url_at(&s, 3, 0).as_deref(), Some("HTTPS://Example.com/A"));
    }

    #[test]
    fn a_rejected_candidate_does_not_fall_back_to_an_inner_match() {
        // D3 is a rejection, not a truncation: nothing inside the rejected span may
        // resolve on a second pass.
        let s = snap(60, 3, "https://github.com@evil.com/pull/1 https://ok.com/x");
        assert!(
            link_at(&s, 10, 0).is_none(),
            "inside the rejected candidate"
        );
        assert_eq!(
            url_at(&s, 40, 0).as_deref(),
            Some("https://ok.com/x"),
            "a later valid candidate still resolves"
        );
    }

    #[test]
    fn contains_is_a_linear_span_across_rows() {
        let link = LinkSpan {
            start: (14, 0),
            end: (7, 1),
            url: "https://example.com/pull/9".into(),
        };
        assert!(link.contains(15, 0, 18));
        assert!(link.contains(0, 1, 18));
        assert!(link.contains(7, 1, 18));
        assert!(!link.contains(13, 0, 18));
        assert!(!link.contains(8, 1, 18));
        // Never panics on edge coordinates.
        assert!(!link.contains(u16::MAX, u16::MAX, 0));
    }

    #[test]
    fn an_osc8_hyperlink_produces_no_link_and_no_damage() {
        // R6.1 (D6): OSC 8 stays ignored. The display text is printed, is NOT
        // clickable, and neither the OSC 2 title nor the OSC 133 channel is
        // disturbed by the sequence passing through.
        let mut t = Terminal::new(40, 3, Color(0xE0E0E0), Color(0x101010));
        t.advance(b"\x1b]8;;https://evil.com\x1b\\PR #123\x1b]8;;\x1b\\");
        let s = t.snapshot();
        assert!(
            s.to_text().contains("PR #123"),
            "the display text is printed: {:?}",
            s.to_text()
        );
        assert!(
            none_across_row(&s, 0),
            "the escape's target is not clickable"
        );
        assert!(t.take_osc_title().is_none(), "OSC 8 sets no title");
        assert!(
            t.take_command_end().is_none(),
            "OSC 8 raises no command end"
        );
    }
}
