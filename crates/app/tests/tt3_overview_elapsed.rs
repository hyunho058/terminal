//! Acceptance test for `specs/session-overview-ui` R3.5 (D7): the elapsed text, through
//! `app::overview::format_elapsed`.

use app::overview::format_elapsed;

const NOW: u64 = 1_793_404_800_000;
const SEC: u64 = 1_000;
const MIN: u64 = 60 * SEC;
const HOUR: u64 = 60 * MIN;

#[test]
fn r3_5_elapsed_format() {
    assert_eq!(format_elapsed(Some(NOW - 30 * SEC), NOW), "<1m");
    assert_eq!(format_elapsed(Some(NOW - 12 * MIN), NOW), "12m");
    assert_eq!(format_elapsed(Some(NOW - (HOUR + 41 * MIN)), NOW), "1h 41m");
    // A session with no digest has no prompt time.
    assert_eq!(format_elapsed(None, NOW), "—");
}
