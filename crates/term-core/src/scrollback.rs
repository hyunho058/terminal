//! Scrollback history: a bounded ring of scrolled-off rows (D1).
//!
//! Rows that leave the top of the live viewport (primary screen only — D5) are
//! retained here, capped at `limit`. Oldest rows are evicted (`pop_front`) when
//! the cap is exceeded, so memory stays bounded (R1.2).

use std::collections::VecDeque;

use crate::grid::Cell;

/// One retained row of cells (length == the grid's columns when it was captured).
pub type Row = Vec<Cell>;

/// Default history cap (candidate 100k — see spec gap G1 on the exact number).
pub const DEFAULT_HISTORY_LIMIT: usize = 100_000;

/// A bounded FIFO of scrolled-off rows. Index 0 is the oldest retained row.
#[derive(Clone, Debug)]
pub struct History {
    rows: VecDeque<Row>,
    limit: usize,
    /// How many rows have ever been evicted off the front (selection-scroll-anchor
    /// D1). Monotonic: +1 per `pop_front`, never reset.
    ///
    /// This is what makes a content line number stable. Index 0 is the oldest
    /// *retained* row, so its meaning shifts on every eviction; `evicted + index`
    /// does not, because an eviction raises `evicted` by exactly as much as it
    /// lowers every index.
    evicted: u64,
}

impl History {
    pub fn new(limit: usize) -> Self {
        History {
            rows: VecDeque::new(),
            limit: limit.max(1),
            evicted: 0,
        }
    }

    /// Retain a row, evicting the oldest if over the cap (R1.2).
    pub fn push(&mut self, row: Row) {
        self.rows.push_back(row);
        while self.rows.len() > self.limit {
            self.rows.pop_front();
            self.evicted += 1;
        }
    }

    /// Total rows evicted off the front since this history was created (D1). Row
    /// `i` of [`get`](History::get) is content line `evicted() + i`.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row at `idx` (0 == oldest retained), or `None` if out of range.
    pub fn get(&self, idx: usize) -> Option<&Row> {
        self.rows.get(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Color;

    /// A one-cell row whose glyph encodes `n`, so eviction/FIFO order is legible.
    fn row(n: u32) -> Row {
        let ch = char::from_u32('0' as u32 + n).unwrap_or('?');
        vec![Cell {
            ch,
            fg: Color(0),
            bg: Color(0),
            attrs: crate::grid::CellAttrs::NONE,
        }]
    }

    #[test]
    fn evicts_oldest_over_the_cap_fifo() {
        // Cap of 3, push 5 → only the last 3 survive, oldest-first (R1.2).
        let mut h = History::new(3);
        for n in 0..5 {
            h.push(row(n));
        }
        assert_eq!(h.len(), 3, "bounded at the cap");
        assert_eq!(h.get(0).unwrap()[0].ch, '2', "oldest survivor is row 2");
        assert_eq!(h.get(2).unwrap()[0].ch, '4', "newest is row 4");
        assert!(h.get(3).is_none(), "nothing beyond the cap");
    }

    #[test]
    fn evicted_counts_every_row_popped_off_the_front() {
        // selection-scroll-anchor R1.1 — cap 3, push 5 → 2 evictions; a fresh
        // history has evicted nothing, and pushes under the cap evict nothing.
        let fresh = History::new(3);
        assert_eq!(fresh.evicted(), 0, "fresh history has evicted nothing");

        let mut h = History::new(3);
        for n in 0..3 {
            h.push(row(n));
        }
        assert_eq!(h.evicted(), 0, "pushes up to the cap evict nothing");
        for n in 3..5 {
            h.push(row(n));
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.evicted(), 2, "one per pop_front");
        // The D1 identity: retained index i is line evicted + i, so the row that
        // was pushed n-th (0-based) sits at index n - evicted.
        assert_eq!(h.get(0).unwrap()[0].ch, '2', "line 2 == evicted + 0");
    }

    #[test]
    fn limit_is_clamped_to_at_least_one() {
        // A zero limit would make a ring that can never retain — clamp to 1.
        let mut h = History::new(0);
        h.push(row(1));
        h.push(row(2));
        assert_eq!(h.len(), 1);
        assert_eq!(h.get(0).unwrap()[0].ch, '2', "keeps only the newest");
    }

    #[test]
    fn empty_history_reports_empty() {
        let h = History::new(10);
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
        assert!(h.get(0).is_none());
    }
}
