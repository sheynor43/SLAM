// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/edge-coloring.cpp (edgeColoringSimple)

//! Assignment of colour channels to edges so that corners stay sharp.

use super::segment::{BLUE, CYAN, EdgeColor, EdgeSegment, GREEN, MAGENTA, RED, WHITE, YELLOW};
use super::shape::Shape;
use super::vector::{Vector2, cross, dot};

/// For each position < n, returns -1, 0 or 1 depending on whether the
/// position is closer to the beginning, middle or end. The total for
/// positions 0 through n-1 is zero.
fn symmetrical_trichotomy(position: usize, n: usize) -> usize {
    // Truncation toward zero of a positive value, as `int(...)` in msdfgen.
    let v = (3.0 + 2.875 * position as f64 / (n - 1) as f64 - 1.4375 + 0.5) as i32 - 3;
    (1 + v) as usize
}

fn is_corner(a_dir: Vector2, b_dir: Vector2, cross_threshold: f64) -> bool {
    dot(a_dir, b_dir) <= 0.0 || cross(a_dir, b_dir).abs() > cross_threshold
}

fn seed_extract2(seed: &mut u64) -> u32 {
    let v = (*seed & 1) as u32;
    *seed >>= 1;
    v
}

fn seed_extract3(seed: &mut u64) -> usize {
    let v = (*seed % 3) as usize;
    *seed /= 3;
    v
}

fn init_color(seed: &mut u64) -> EdgeColor {
    const COLORS: [EdgeColor; 3] = [CYAN, MAGENTA, YELLOW];
    COLORS[seed_extract3(seed)]
}

fn switch_color(color: &mut EdgeColor, seed: &mut u64) {
    let shifted = u32::from(*color) << (1 + seed_extract2(seed));
    *color = ((shifted | shifted >> 3) & u32::from(WHITE)) as EdgeColor;
}

fn switch_color_banned(color: &mut EdgeColor, seed: &mut u64, banned: EdgeColor) {
    let combined = *color & banned;
    if combined == RED || combined == GREEN || combined == BLUE {
        *color = combined ^ WHITE;
    } else {
        switch_color(color, seed);
    }
}

/// Colours the edges of `shape`; corners sharper than `angle_threshold`
/// radians get edges of different colours on each side.
pub fn edge_coloring_simple(shape: &mut Shape, angle_threshold: f64, mut seed: u64) {
    let cross_threshold = angle_threshold.sin();
    let mut color = init_color(&mut seed);
    let mut corners: Vec<usize> = Vec::new();
    for contour in &mut shape.contours {
        let edges = &mut contour.edges;
        if edges.is_empty() {
            continue;
        }
        // Identify corners
        corners.clear();
        let mut prev_direction = edges[edges.len() - 1].direction(1.0);
        for (index, edge) in edges.iter().enumerate() {
            if is_corner(
                prev_direction.normalize(false),
                edge.direction(0.0).normalize(false),
                cross_threshold,
            ) {
                corners.push(index);
            }
            prev_direction = edge.direction(1.0);
        }

        if corners.is_empty() {
            // Smooth contour
            switch_color(&mut color, &mut seed);
            for edge in edges.iter_mut() {
                edge.color = color;
            }
        } else if corners.len() == 1 {
            // "Teardrop" case
            let mut colors = [0; 3];
            switch_color(&mut color, &mut seed);
            colors[0] = color;
            colors[1] = WHITE;
            switch_color(&mut color, &mut seed);
            colors[2] = color;
            let corner = corners[0];
            let m = edges.len();
            if m >= 3 {
                for i in 0..m {
                    edges[(corner + i) % m].color = colors[symmetrical_trichotomy(i, m)];
                }
            } else {
                // Less than three edge segments for three colors => edges must be split
                let mut parts: [Option<EdgeSegment>; 7] = [None; 7];
                let first = edges[0].split_in_thirds();
                for (k, part) in first.into_iter().enumerate() {
                    parts[k + 3 * corner] = Some(part);
                }
                if m >= 2 {
                    let second = edges[1].split_in_thirds();
                    for (k, part) in second.into_iter().enumerate() {
                        parts[3 + k - 3 * corner] = Some(part);
                    }
                    for (k, part) in parts.iter_mut().take(6).enumerate() {
                        if let Some(part) = part {
                            part.color = colors[k / 2];
                        }
                    }
                } else {
                    for (k, part) in parts.iter_mut().take(3).enumerate() {
                        if let Some(part) = part {
                            part.color = colors[k];
                        }
                    }
                }
                edges.clear();
                edges.extend(parts.into_iter().map_while(|p| p));
            }
        } else {
            // Multiple corners
            let corner_count = corners.len();
            let mut spline = 0;
            let start = corners[0];
            let m = edges.len();
            switch_color(&mut color, &mut seed);
            let initial_color = color;
            for i in 0..m {
                let index = (start + i) % m;
                if spline + 1 < corner_count && corners[spline + 1] == index {
                    spline += 1;
                    let banned = if spline == corner_count - 1 {
                        initial_color
                    } else {
                        0
                    };
                    switch_color_banned(&mut color, &mut seed, banned);
                }
                edges[index].color = color;
            }
        }
    }
}
