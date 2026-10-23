// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/Scanline.cpp

//! Horizontal scanline through a shape, for fill tests.

/// An intersection of the scanline with an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Intersection {
    pub x: f64,
    /// Y direction of the edge at the intersection; after preprocessing,
    /// the running sum of directions up to and including this one.
    pub direction: i32,
}

/// Rule that turns the winding number into inside/outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    // msdfgen's other rules; the glyph pipeline only uses `NonZero` (ADR-0062).
    #[allow(dead_code)]
    Odd,
    #[allow(dead_code)]
    Positive,
    #[allow(dead_code)]
    Negative,
}

impl FillRule {
    pub fn is_filled(self, winding: i32) -> bool {
        match self {
            Self::NonZero => winding != 0,
            Self::Odd => winding & 1 != 0,
            Self::Positive => winding > 0,
            Self::Negative => winding < 0,
        }
    }
}

/// Intersections of a shape with one horizontal line, sorted by X.
#[derive(Clone, Debug, Default)]
pub struct Scanline {
    intersections: Vec<Intersection>,
    last_index: usize,
}

impl Scanline {
    pub(super) fn intersections_mut(&mut self) -> &mut Vec<Intersection> {
        &mut self.intersections
    }

    /// Sorts the intersections and accumulates their directions.
    pub(super) fn preprocess(&mut self) {
        self.last_index = 0;
        // Intersections at the same X may end up in any order: lookups always
        // land on the last of them, whose running sum does not depend on it.
        self.intersections
            .sort_unstable_by(|a, b| a.x.total_cmp(&b.x));
        let mut total_direction = 0;
        for intersection in &mut self.intersections {
            total_direction += intersection.direction;
            intersection.direction = total_direction;
        }
    }

    /// Index of the last intersection at or left of `x`.
    fn move_to(&mut self, x: f64) -> Option<usize> {
        if self.intersections.is_empty() {
            return None;
        }
        let mut index = self.last_index;
        if x < self.intersections[index].x {
            loop {
                if index == 0 {
                    self.last_index = 0;
                    return None;
                }
                index -= 1;
                // `while (x < xs[index])` in msdfgen, also for NaN.
                #[allow(clippy::neg_cmp_op_on_partial_ord)]
                if !(x < self.intersections[index].x) {
                    break;
                }
            }
        } else {
            while index < self.intersections.len() - 1 && x >= self.intersections[index + 1].x {
                index += 1;
            }
        }
        self.last_index = index;
        Some(index)
    }

    /// Winding number of the shape left of `x`.
    pub fn sum_intersections(&mut self, x: f64) -> i32 {
        self.move_to(x)
            .map_or(0, |index| self.intersections[index].direction)
    }

    pub fn filled(&mut self, x: f64, fill_rule: FillRule) -> bool {
        fill_rule.is_filled(self.sum_intersections(x))
    }
}
