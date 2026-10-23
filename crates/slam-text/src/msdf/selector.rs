// Copyright (c) 2014 - 2025 Viktor Chlumsky (msdfgen, MIT licence; see THIRD_PARTY_NOTICES).
// Ported from msdfgen v1.13: core/edge-selectors.cpp, core/contour-combiners.cpp, core/ShapeDistanceFinder.hpp

//! Per-channel distance selection and the overlapping contour combiner.

use super::segment::{BLUE, EdgeSegment, GREEN, RED};
use super::shape::Shape;
use super::vector::{Point2, SignedDistance, Vector2, cross, dot, median, non_zero_sign};

const DISTANCE_DELTA_FACTOR: f64 = 1.001;

/// Distances of the three channels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MultiDistance {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl MultiDistance {
    /// `resolveDistance`: the median of the channels.
    fn resolve(self) -> f64 {
        median(self.r, self.g, self.b)
    }
}

/// Per-edge cache that lets the selector skip edges which cannot be closer
/// than the current best for the next nearby pixel.
#[derive(Clone, Copy, Debug, Default)]
pub struct EdgeCache {
    point: Point2,
    abs_distance: f64,
    a_domain_distance: f64,
    b_domain_distance: f64,
    a_perpendicular_distance: f64,
    b_perpendicular_distance: f64,
}

/// `PerpendicularDistanceSelectorBase`: one channel.
#[derive(Clone, Copy, Debug)]
struct ChannelSelector {
    min_true_distance: SignedDistance,
    min_negative_perpendicular_distance: f64,
    min_positive_perpendicular_distance: f64,
    near_edge: Option<EdgeSegment>,
    near_edge_param: f64,
}

impl Default for ChannelSelector {
    fn default() -> Self {
        let min_true_distance = SignedDistance::default();
        Self {
            min_true_distance,
            min_negative_perpendicular_distance: -min_true_distance.distance.abs(),
            min_positive_perpendicular_distance: min_true_distance.distance.abs(),
            near_edge: None,
            near_edge_param: 0.0,
        }
    }
}

fn get_perpendicular_distance(distance: &mut f64, ep: Vector2, edge_dir: Vector2) -> bool {
    let ts = dot(ep, edge_dir);
    if ts > 0.0 {
        let perpendicular_distance = cross(ep, edge_dir);
        if perpendicular_distance.abs() < distance.abs() {
            *distance = perpendicular_distance;
            return true;
        }
    }
    false
}

impl ChannelSelector {
    fn reset(&mut self, delta: f64) {
        self.min_true_distance.distance += non_zero_sign(self.min_true_distance.distance) * delta;
        self.min_negative_perpendicular_distance = -self.min_true_distance.distance.abs();
        self.min_positive_perpendicular_distance = self.min_true_distance.distance.abs();
        self.near_edge = None;
        self.near_edge_param = 0.0;
    }

    fn is_edge_relevant(&self, cache: &EdgeCache, p: Point2) -> bool {
        let delta = DISTANCE_DELTA_FACTOR * (p - cache.point).length();
        cache.abs_distance - delta <= self.min_true_distance.distance.abs()
            || cache.a_domain_distance.abs() < delta
            || cache.b_domain_distance.abs() < delta
            || (cache.a_domain_distance > 0.0
                && if cache.a_perpendicular_distance < 0.0 {
                    cache.a_perpendicular_distance + delta
                        >= self.min_negative_perpendicular_distance
                } else {
                    cache.a_perpendicular_distance - delta
                        <= self.min_positive_perpendicular_distance
                })
            || (cache.b_domain_distance > 0.0
                && if cache.b_perpendicular_distance < 0.0 {
                    cache.b_perpendicular_distance + delta
                        >= self.min_negative_perpendicular_distance
                } else {
                    cache.b_perpendicular_distance - delta
                        <= self.min_positive_perpendicular_distance
                })
    }

    fn add_edge_true_distance(&mut self, edge: &EdgeSegment, distance: SignedDistance, param: f64) {
        if distance.closer_than(self.min_true_distance) {
            self.min_true_distance = distance;
            self.near_edge = Some(*edge);
            self.near_edge_param = param;
        }
    }

    fn add_edge_perpendicular_distance(&mut self, distance: f64) {
        if distance <= 0.0 && distance > self.min_negative_perpendicular_distance {
            self.min_negative_perpendicular_distance = distance;
        }
        if distance >= 0.0 && distance < self.min_positive_perpendicular_distance {
            self.min_positive_perpendicular_distance = distance;
        }
    }

    fn merge(&mut self, other: &Self) {
        if other.min_true_distance.closer_than(self.min_true_distance) {
            self.min_true_distance = other.min_true_distance;
            self.near_edge = other.near_edge;
            self.near_edge_param = other.near_edge_param;
        }
        if other.min_negative_perpendicular_distance > self.min_negative_perpendicular_distance {
            self.min_negative_perpendicular_distance = other.min_negative_perpendicular_distance;
        }
        if other.min_positive_perpendicular_distance < self.min_positive_perpendicular_distance {
            self.min_positive_perpendicular_distance = other.min_positive_perpendicular_distance;
        }
    }

    fn compute_distance(&self, p: Point2) -> f64 {
        let mut min_distance = if self.min_true_distance.distance < 0.0 {
            self.min_negative_perpendicular_distance
        } else {
            self.min_positive_perpendicular_distance
        };
        if let Some(near_edge) = &self.near_edge {
            let mut distance = self.min_true_distance;
            near_edge.distance_to_perpendicular_distance(&mut distance, p, self.near_edge_param);
            if distance.distance.abs() < min_distance.abs() {
                min_distance = distance.distance;
            }
        }
        min_distance
    }
}

/// `MultiDistanceSelector`: pseudo-distances of the three channels.
#[derive(Clone, Copy, Debug, Default)]
pub struct MultiDistanceSelector {
    p: Point2,
    r: ChannelSelector,
    g: ChannelSelector,
    b: ChannelSelector,
}

impl MultiDistanceSelector {
    pub fn reset(&mut self, p: Point2) {
        let delta = DISTANCE_DELTA_FACTOR * (p - self.p).length();
        self.r.reset(delta);
        self.g.reset(delta);
        self.b.reset(delta);
        self.p = p;
    }

    pub fn add_edge(
        &mut self,
        cache: &mut EdgeCache,
        prev_edge: &EdgeSegment,
        edge: &EdgeSegment,
        next_edge: &EdgeSegment,
    ) {
        let p = self.p;
        let (has_r, has_g, has_b) = (
            edge.color & RED != 0,
            edge.color & GREEN != 0,
            edge.color & BLUE != 0,
        );
        if !((has_r && self.r.is_edge_relevant(cache, p))
            || (has_g && self.g.is_edge_relevant(cache, p))
            || (has_b && self.b.is_edge_relevant(cache, p)))
        {
            return;
        }
        let (distance, param) = edge.signed_distance(p);
        if has_r {
            self.r.add_edge_true_distance(edge, distance, param);
        }
        if has_g {
            self.g.add_edge_true_distance(edge, distance, param);
        }
        if has_b {
            self.b.add_edge_true_distance(edge, distance, param);
        }
        cache.point = p;
        cache.abs_distance = distance.distance.abs();

        let ap = p - edge.point(0.0);
        let bp = p - edge.point(1.0);
        let a_dir = edge.direction(0.0).normalize(true);
        let b_dir = edge.direction(1.0).normalize(true);
        let prev_dir = prev_edge.direction(1.0).normalize(true);
        let next_dir = next_edge.direction(0.0).normalize(true);
        let add = dot(ap, (prev_dir + a_dir).normalize(true));
        let bdd = -dot(bp, (b_dir + next_dir).normalize(true));
        if add > 0.0 {
            let mut pd = distance.distance;
            if get_perpendicular_distance(&mut pd, ap, -a_dir) {
                pd = -pd;
                self.add_perpendicular(pd, has_r, has_g, has_b);
            }
            cache.a_perpendicular_distance = pd;
        }
        if bdd > 0.0 {
            let mut pd = distance.distance;
            if get_perpendicular_distance(&mut pd, bp, b_dir) {
                self.add_perpendicular(pd, has_r, has_g, has_b);
            }
            cache.b_perpendicular_distance = pd;
        }
        cache.a_domain_distance = add;
        cache.b_domain_distance = bdd;
    }

    fn add_perpendicular(&mut self, pd: f64, has_r: bool, has_g: bool, has_b: bool) {
        if has_r {
            self.r.add_edge_perpendicular_distance(pd);
        }
        if has_g {
            self.g.add_edge_perpendicular_distance(pd);
        }
        if has_b {
            self.b.add_edge_perpendicular_distance(pd);
        }
    }

    pub fn merge(&mut self, other: &Self) {
        self.r.merge(&other.r);
        self.g.merge(&other.g);
        self.b.merge(&other.b);
    }

    pub fn distance(&self) -> MultiDistance {
        MultiDistance {
            r: self.r.compute_distance(self.p),
            g: self.g.compute_distance(self.p),
            b: self.b.compute_distance(self.p),
        }
    }
}

/// `OverlappingContourCombiner<MultiDistanceSelector>`: resolves the
/// distance of shapes whose contours overlap.
pub struct OverlappingContourCombiner {
    p: Point2,
    windings: Vec<i32>,
    edge_selectors: Vec<MultiDistanceSelector>,
}

impl OverlappingContourCombiner {
    pub fn new(shape: &Shape) -> Self {
        Self {
            p: Point2::ZERO,
            windings: shape.contours.iter().map(|c| c.winding()).collect(),
            edge_selectors: vec![MultiDistanceSelector::default(); shape.contours.len()],
        }
    }

    pub fn reset(&mut self, p: Point2) {
        self.p = p;
        for selector in &mut self.edge_selectors {
            selector.reset(p);
        }
    }

    pub fn edge_selector(&mut self, i: usize) -> &mut MultiDistanceSelector {
        &mut self.edge_selectors[i]
    }

    pub fn distance(&self) -> MultiDistance {
        let mut shape_edge_selector = MultiDistanceSelector::default();
        let mut inner_edge_selector = MultiDistanceSelector::default();
        let mut outer_edge_selector = MultiDistanceSelector::default();
        shape_edge_selector.reset(self.p);
        inner_edge_selector.reset(self.p);
        outer_edge_selector.reset(self.p);
        for (selector, &winding) in self.edge_selectors.iter().zip(&self.windings) {
            let edge_distance = selector.distance();
            shape_edge_selector.merge(selector);
            if winding > 0 && edge_distance.resolve() >= 0.0 {
                inner_edge_selector.merge(selector);
            }
            if winding < 0 && edge_distance.resolve() <= 0.0 {
                outer_edge_selector.merge(selector);
            }
        }

        let shape_distance = shape_edge_selector.distance();
        let inner_distance = inner_edge_selector.distance();
        let outer_distance = outer_edge_selector.distance();
        let inner_scalar_distance = inner_distance.resolve();
        let outer_scalar_distance = outer_distance.resolve();
        let mut distance;
        let winding;

        if inner_scalar_distance >= 0.0
            && inner_scalar_distance.abs() <= outer_scalar_distance.abs()
        {
            distance = inner_distance;
            winding = 1;
            for (selector, &w) in self.edge_selectors.iter().zip(&self.windings) {
                if w > 0 {
                    let contour_distance = selector.distance();
                    if contour_distance.resolve().abs() < outer_scalar_distance.abs()
                        && contour_distance.resolve() > distance.resolve()
                    {
                        distance = contour_distance;
                    }
                }
            }
        } else if outer_scalar_distance <= 0.0
            && outer_scalar_distance.abs() < inner_scalar_distance.abs()
        {
            distance = outer_distance;
            winding = -1;
            for (selector, &w) in self.edge_selectors.iter().zip(&self.windings) {
                if w < 0 {
                    let contour_distance = selector.distance();
                    if contour_distance.resolve().abs() < inner_scalar_distance.abs()
                        && contour_distance.resolve() < distance.resolve()
                    {
                        distance = contour_distance;
                    }
                }
            }
        } else {
            return shape_distance;
        }

        for (selector, &w) in self.edge_selectors.iter().zip(&self.windings) {
            if w != winding {
                let contour_distance = selector.distance();
                if contour_distance.resolve() * distance.resolve() >= 0.0
                    && contour_distance.resolve().abs() < distance.resolve().abs()
                {
                    distance = contour_distance;
                }
            }
        }
        if distance.resolve() == shape_distance.resolve() {
            distance = shape_distance;
        }
        distance
    }
}

/// `SimpleContourCombiner<MultiDistanceSelector>` or
/// `OverlappingContourCombiner<MultiDistanceSelector>`.
enum Combiner {
    // Boxed: the selector is several times larger than the other variant.
    Simple(Box<MultiDistanceSelector>),
    Overlapping(OverlappingContourCombiner),
}

impl Combiner {
    fn reset(&mut self, p: Point2) {
        match self {
            Self::Simple(selector) => selector.reset(p),
            Self::Overlapping(combiner) => combiner.reset(p),
        }
    }

    fn edge_selector(&mut self, i: usize) -> &mut MultiDistanceSelector {
        match self {
            Self::Simple(selector) => selector,
            Self::Overlapping(combiner) => combiner.edge_selector(i),
        }
    }

    fn distance(&self) -> MultiDistance {
        match self {
            Self::Simple(selector) => selector.distance(),
            Self::Overlapping(combiner) => combiner.distance(),
        }
    }
}

/// `ShapeDistanceFinder<ContourCombiner<MultiDistanceSelector>>`.
pub struct ShapeDistanceFinder<'a> {
    shape: &'a Shape,
    combiner: Combiner,
    edge_cache: Vec<EdgeCache>,
}

impl<'a> ShapeDistanceFinder<'a> {
    /// `overlap_support` selects `OverlappingContourCombiner` over
    /// `SimpleContourCombiner`, as in `MSDFGeneratorConfig`.
    pub fn new(shape: &'a Shape, overlap_support: bool) -> Self {
        let combiner = if overlap_support {
            Combiner::Overlapping(OverlappingContourCombiner::new(shape))
        } else {
            Combiner::Simple(Box::default())
        };
        Self {
            shape,
            combiner,
            edge_cache: vec![EdgeCache::default(); shape.edge_count()],
        }
    }

    /// Multi-channel distance from `origin` to the shape.
    pub fn distance(&mut self, origin: Point2) -> MultiDistance {
        self.combiner.reset(origin);
        let mut cache = self.edge_cache.iter_mut();
        for (i, contour) in self.shape.contours.iter().enumerate() {
            let edges = &contour.edges;
            if edges.is_empty() {
                continue;
            }
            let selector = self.combiner.edge_selector(i);
            let mut prev_edge = if edges.len() >= 2 {
                &edges[edges.len() - 2]
            } else {
                &edges[0]
            };
            let mut cur_edge = &edges[edges.len() - 1];
            for next_edge in edges {
                // The cache has exactly one entry per edge (`edge_count`).
                let entry = cache.next().expect("edge cache sized by edge count");
                selector.add_edge(entry, prev_edge, cur_edge, next_edge);
                prev_edge = cur_edge;
                cur_edge = next_edge;
            }
        }
        self.combiner.distance()
    }
}
