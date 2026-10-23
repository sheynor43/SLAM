// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/Vector2.hpp, core/SignedDistance.hpp, core/arithmetics.hpp

//! 2D vector, signed distance and scalar helpers of the MSDF generator.

use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub};

/// A 2D vector or point (`Vector2`/`Point2` in msdfgen).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector2 {
    pub x: f64,
    pub y: f64,
}

pub type Point2 = Vector2;

impl Vector2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Unit vector of the same direction; a zero vector gives `(0, !allow_zero)`.
    pub fn normalize(self, allow_zero: bool) -> Self {
        let len = self.length();
        if len != 0.0 {
            return Self::new(self.x / len, self.y / len);
        }
        Self::new(0.0, if allow_zero { 0.0 } else { 1.0 })
    }

    /// Orthogonal vector of the same length.
    pub fn orthogonal(self, polarity: bool) -> Self {
        if polarity {
            Self::new(-self.y, self.x)
        } else {
            Self::new(self.y, -self.x)
        }
    }

    /// Orthogonal unit vector (`getOrthonormal` with `allowZero = false`).
    pub fn orthonormal(self, polarity: bool) -> Self {
        let len = self.length();
        if len != 0.0 {
            return if polarity {
                Self::new(-self.y / len, self.x / len)
            } else {
                Self::new(self.y / len, -self.x / len)
            };
        }
        if polarity {
            Self::new(0.0, 1.0)
        } else {
            Self::new(0.0, -1.0)
        }
    }

    /// `operator bool`: true unless both components are zero.
    pub fn is_nonzero(self) -> bool {
        self.x != 0.0 || self.y != 0.0
    }
}

pub fn dot(a: Vector2, b: Vector2) -> f64 {
    a.x * b.x + a.y * b.y
}

pub fn cross(a: Vector2, b: Vector2) -> f64 {
    a.x * b.y - a.y * b.x
}

impl Add for Vector2 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y)
    }
}

impl AddAssign for Vector2 {
    fn add_assign(&mut self, o: Self) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl Sub for Vector2 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y)
    }
}

impl Neg for Vector2 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

impl Mul<Vector2> for f64 {
    type Output = Vector2;
    fn mul(self, v: Vector2) -> Vector2 {
        Vector2::new(self * v.x, self * v.y)
    }
}

impl Mul<f64> for Vector2 {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s)
    }
}

impl MulAssign<f64> for Vector2 {
    fn mul_assign(&mut self, s: f64) {
        self.x *= s;
        self.y *= s;
    }
}

impl Div<Vector2> for Vector2 {
    type Output = Self;
    fn div(self, o: Self) -> Self {
        Self::new(self.x / o.x, self.y / o.y)
    }
}

/// `mix(a, b, w) = (1 - w) * a + w * b` for points.
pub fn mix(a: Point2, b: Point2, w: f64) -> Point2 {
    (1.0 - w) * a + w * b
}

/// `mix` for scalars.
pub fn mix_f(a: f64, b: f64, w: f64) -> f64 {
    (1.0 - w) * a + w * b
}

/// 1 for positive, -1 for negative, 0 for zero.
pub fn sign(n: f64) -> i32 {
    i32::from(0.0 < n) - i32::from(n < 0.0)
}

/// 1 for positive, -1 otherwise (zero included).
pub fn non_zero_sign(n: f64) -> f64 {
    if n > 0.0 { 1.0 } else { -1.0 }
}

fn min<T: PartialOrd>(a: T, b: T) -> T {
    if b < a { b } else { a }
}

fn max<T: PartialOrd>(a: T, b: T) -> T {
    if a < b { b } else { a }
}

/// The middle of three values, with msdfgen's comparison order.
pub fn median<T: PartialOrd + Copy>(a: T, b: T, c: T) -> T {
    max(min(a, b), min(max(a, b), c))
}

/// A signed distance and the alignment used to break ties between edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignedDistance {
    pub distance: f64,
    pub dot: f64,
}

impl Default for SignedDistance {
    fn default() -> Self {
        Self {
            distance: -f64::MAX,
            dot: 0.0,
        }
    }
}

impl SignedDistance {
    pub const fn new(distance: f64, dot: f64) -> Self {
        Self { distance, dot }
    }

    /// `operator<` of msdfgen.
    pub fn closer_than(self, o: Self) -> bool {
        self.distance.abs() < o.distance.abs()
            || (self.distance.abs() == o.distance.abs() && self.dot < o.dot)
    }
}
