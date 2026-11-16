//! Where the rows of the song list are: sets and difficulties have their own heights
//! (ADR-0149), so a row's place is a sum, worked out in closed form from the flat index.

use std::ops::Range;

use super::SongFilter;

/// The column of rows with the visible set at `open` expanded, in theme reference pixels
/// from the top of the list. `Copy`, and it does not allocate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowGeometry {
    /// Pitch of a set row: its plate and the gap below it.
    pub set_pitch: f32,
    /// Pitch of a difficulty row.
    pub difficulty_pitch: f32,
    /// Gap between two rows, the part of a pitch that is not the plate.
    pub gap: f32,
    /// Flat index of the expanded set's row; its difficulties follow it.
    pub open: usize,
    /// How many difficulty rows follow the expanded set.
    pub count: usize,
    /// All the rows.
    pub total: usize,
}

impl RowGeometry {
    /// The rows of `filter` with the visible set at position `selected_set` expanded.
    pub fn new(
        set_pitch: f32,
        difficulty_pitch: f32,
        gap: f32,
        filter: &SongFilter,
        selected_set: usize,
    ) -> RowGeometry {
        let count = if filter.set_count() == 0 {
            0
        } else {
            filter.difficulty_count(selected_set)
        };
        RowGeometry {
            set_pitch,
            difficulty_pitch,
            gap,
            open: selected_set,
            count,
            total: filter.set_count() + count,
        }
    }

    /// Whether flat index `index` is a difficulty row.
    fn is_difficulty(&self, index: usize) -> bool {
        index > self.open && index <= self.open + self.count
    }

    /// Top of row `index`; `index == total` is the bottom of the last row.
    pub fn top(&self, index: usize) -> f32 {
        let (s, d) = (self.set_pitch, self.difficulty_pitch);
        if index <= self.open {
            index as f32 * s
        } else if index <= self.open + self.count {
            (self.open + 1) as f32 * s + (index - self.open - 1) as f32 * d
        } else {
            (index - self.count) as f32 * s + self.count as f32 * d
        }
    }

    /// Pitch of row `index`.
    pub fn pitch(&self, index: usize) -> f32 {
        if self.is_difficulty(index) {
            self.difficulty_pitch
        } else {
            self.set_pitch
        }
    }

    /// Height of the plate of row `index`, without the gap.
    pub fn plate(&self, index: usize) -> f32 {
        (self.pitch(index) - self.gap).max(0.0)
    }

    /// Height of all the rows.
    pub fn height(&self) -> f32 {
        self.top(self.total)
    }

    /// The row whose pitch holds `y` (from the top of the list), not clamped to `total`;
    /// `None` above the first row.
    pub fn index_at(&self, y: f32) -> Option<usize> {
        if y < 0.0 || !y.is_finite() {
            return None;
        }
        let (s, d) = (
            self.set_pitch.max(f32::EPSILON),
            self.difficulty_pitch.max(f32::EPSILON),
        );
        let first_difficulty = self.top(self.open + 1);
        let after = self.top(self.open + 1 + self.count);
        Some(if y < first_difficulty {
            (y / s) as usize
        } else if y < after {
            self.open + 1 + ((y - first_difficulty) / d) as usize
        } else {
            self.open + 1 + self.count + ((y - after) / s) as usize
        })
    }

    /// Rows that touch `[scroll, scroll + viewport)`, out of `total`.
    pub fn visible(&self, scroll: f32, viewport: f32) -> Range<usize> {
        let scroll = scroll.max(0.0);
        let first = self.index_at(scroll).unwrap_or(0).min(self.total);
        let mut last = first;
        while last < self.total && self.top(last) < scroll + viewport {
            last += 1;
        }
        first..last
    }

    /// The scroll that puts the middle of row `index`'s plate in the middle of a
    /// `viewport` tall list (ADR-0149), not clamped.
    pub fn centred(&self, index: usize, viewport: f32) -> f32 {
        self.top(index) + self.plate(index) / 2.0 - viewport / 2.0
    }

    /// The furthest scroll: the last row's plate at the bottom of the list.
    pub fn max_scroll(&self, viewport: f32) -> f32 {
        (self.height() - self.gap - viewport).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(open: usize, count: usize, sets: usize) -> RowGeometry {
        RowGeometry {
            set_pitch: 75.0,
            difficulty_pitch: 48.0,
            gap: 3.0,
            open,
            count,
            total: sets + count,
        }
    }

    #[test]
    fn rows_stack_with_their_own_pitches() {
        let g = geometry(2, 3, 10);
        assert_eq!(g.top(0), 0.0);
        assert_eq!(g.top(2), 150.0);
        // The difficulties of set 2.
        assert_eq!(g.top(3), 225.0);
        assert_eq!(g.top(5), 225.0 + 96.0);
        // Set 3 follows them.
        assert_eq!(g.top(6), 225.0 + 144.0);
        assert_eq!(g.top(7), 225.0 + 144.0 + 75.0);
        assert_eq!(g.pitch(4), 48.0);
        assert_eq!(g.pitch(6), 75.0);
        assert_eq!(g.plate(4), 45.0);
        assert_eq!(g.height(), 10.0 * 75.0 + 144.0);
    }

    #[test]
    fn index_at_inverts_top() {
        let g = geometry(2, 3, 10);
        for index in 0..g.total {
            assert_eq!(g.index_at(g.top(index)), Some(index));
            assert_eq!(
                g.index_at(g.top(index) + g.pitch(index) - 0.01),
                Some(index)
            );
        }
        assert_eq!(g.index_at(-1.0), None);
        assert_eq!(g.index_at(g.height()), Some(g.total));
    }

    #[test]
    fn visible_covers_the_viewport() {
        let g = geometry(2, 3, 10);
        assert_eq!(g.visible(0.0, 150.0), 0..2);
        assert_eq!(g.visible(0.0, 151.0), 0..3);
        assert_eq!(g.visible(230.0, 50.0), 3..5);
        assert_eq!(g.visible(0.0, 1e6), 0..g.total);
    }

    #[test]
    fn centred_puts_the_plate_in_the_middle() {
        let g = geometry(2, 3, 10);
        let scroll = g.centred(4, 400.0);
        assert_eq!(scroll + 200.0, g.top(4) + 22.5);
        assert_eq!(g.max_scroll(400.0), g.height() - 3.0 - 400.0);
        assert_eq!(geometry(0, 0, 2).max_scroll(400.0), 0.0);
    }
}
