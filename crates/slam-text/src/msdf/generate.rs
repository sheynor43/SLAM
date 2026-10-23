// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/msdfgen.cpp, core/rasterization.cpp, core/Projection.cpp, core/DistanceMapping.cpp

//! Multi-channel distance field generation and sign correction.

use super::scanline::{FillRule, Scanline};
use super::selector::ShapeDistanceFinder;
use super::shape::Shape;
use super::vector::{Point2, Vector2, median};

/// Maps bitmap pixel coordinates to shape coordinates: `shape = pixel / scale - translate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub scale: Vector2,
    pub translate: Vector2,
}

impl Projection {
    pub fn project(&self, coord: Point2) -> Point2 {
        Point2::new(
            self.scale.x * (coord.x + self.translate.x),
            self.scale.y * (coord.y + self.translate.y),
        )
    }

    pub fn unproject_vector(&self, vector: Vector2) -> Vector2 {
        Vector2::new(vector.x / self.scale.x, vector.y / self.scale.y)
    }

    pub fn unproject(&self, coord: Point2) -> Point2 {
        Point2::new(
            coord.x / self.scale.x - self.translate.x,
            coord.y / self.scale.y - self.translate.y,
        )
    }

    pub fn unproject_x(&self, x: f64) -> f64 {
        x / self.scale.x - self.translate.x
    }

    pub fn unproject_y(&self, y: f64) -> f64 {
        y / self.scale.y - self.translate.y
    }
}

/// Distance range in shape units mapped to `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub lower: f64,
    pub upper: f64,
}

impl Range {
    /// A range symmetric around zero (`Range(double symmetricalWidth)`).
    pub fn symmetric(width: f64) -> Self {
        Self {
            lower: -0.5 * width,
            upper: 0.5 * width,
        }
    }

    /// Field value of distance zero (`sdfZeroValue` of the msdfgen utility).
    pub fn zero_value(&self) -> f32 {
        if self.lower != self.upper {
            (self.lower / (self.lower - self.upper)) as f32
        } else {
            0.5
        }
    }
}

/// `DistanceMapping(Range)`.
#[derive(Clone, Copy, Debug)]
struct DistanceMapping {
    scale: f64,
    translate: f64,
}

impl DistanceMapping {
    fn new(range: Range) -> Self {
        Self {
            scale: 1.0 / (range.upper - range.lower),
            translate: -range.lower,
        }
    }

    fn map(&self, d: f64) -> f64 {
        self.scale * (d + self.translate)
    }
}

/// A three-channel float bitmap. Row 0 is the bottom row (Y up, as the shape).
#[derive(Clone, Debug, PartialEq)]
pub struct MsdfField {
    pub width: usize,
    pub height: usize,
    /// `pixels[y * width + x]` is the pixel whose centre is `(x + 0.5, y + 0.5)`.
    pub pixels: Vec<[f32; 3]>,
}

impl MsdfField {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![[0.0; 3]; width * height],
        }
    }
}

/// `generateMSDF` without error correction; `overlap_support` as in
/// `MSDFGeneratorConfig`.
pub fn generate_msdf(
    output: &mut MsdfField,
    shape: &Shape,
    projection: &Projection,
    range: Range,
    overlap_support: bool,
) {
    let mapping = DistanceMapping::new(range);
    let mut finder = ShapeDistanceFinder::new(shape, overlap_support);
    let (w, h) = (output.width, output.height);
    // Rows alternate direction so the edge cache sees neighbouring pixels.
    let mut left_to_right = true;
    for y in 0..h {
        for col in 0..w {
            let x = if left_to_right { col } else { w - 1 - col };
            let p = projection.unproject(Point2::new(x as f64 + 0.5, y as f64 + 0.5));
            let distance = finder.distance(p);
            output.pixels[y * w + x] = [
                mapping.map(distance.r) as f32,
                mapping.map(distance.g) as f32,
                mapping.map(distance.b) as f32,
            ];
        }
        left_to_right = !left_to_right;
    }
}

/// Flips pixels whose sign disagrees with a scanline fill of the shape
/// (`distanceSignCorrection` for three channels).
pub fn distance_sign_correction(
    sdf: &mut MsdfField,
    shape: &Shape,
    projection: &Projection,
    sdf_zero_value: f32,
    fill_rule: FillRule,
) {
    let (w, h) = (sdf.width, sdf.height);
    if w == 0 || h == 0 {
        return;
    }
    let double_zero = sdf_zero_value + sdf_zero_value;
    let mut scanline = Scanline::default();
    let mut ambiguous = false;
    let mut match_map = vec![0i8; w * h];
    for y in 0..h {
        shape.scanline(&mut scanline, projection.unproject_y(y as f64 + 0.5));
        for x in 0..w {
            let fill = scanline.filled(projection.unproject_x(x as f64 + 0.5), fill_rule);
            let msd = &mut sdf.pixels[y * w + x];
            let sd = median(msd[0], msd[1], msd[2]);
            if sd == sdf_zero_value {
                ambiguous = true;
            } else if (sd > sdf_zero_value) != fill {
                for c in msd.iter_mut() {
                    *c = double_zero - *c;
                }
                match_map[y * w + x] = -1;
            } else {
                match_map[y * w + x] = 1;
            }
        }
    }
    // This step is necessary to avoid artifacts when whole shape is inverted
    if ambiguous {
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if match_map[i] != 0 {
                    continue;
                }
                let mut neighbor_match = 0;
                if x > 0 {
                    neighbor_match += i32::from(match_map[i - 1]);
                }
                if x < w - 1 {
                    neighbor_match += i32::from(match_map[i + 1]);
                }
                if y > 0 {
                    neighbor_match += i32::from(match_map[i - w]);
                }
                if y < h - 1 {
                    neighbor_match += i32::from(match_map[i + w]);
                }
                if neighbor_match < 0 {
                    for c in sdf.pixels[i].iter_mut() {
                        *c = double_zero - *c;
                    }
                }
            }
        }
    }
}
