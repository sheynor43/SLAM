use super::*;

fn mesh(points: &[[f32; 2]]) -> SliderMesh {
    let mut cumulative = Vec::with_capacity(points.len());
    let mut d = 0.0f64;
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            let q = points[i - 1];
            d += f64::from(((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt());
        }
        cumulative.push(d);
    }
    SliderMesh::new(points, &cumulative)
}

#[test]
fn empty_path_makes_an_empty_mesh() {
    let m = SliderMesh::new(&[], &[]);
    assert!(m.is_empty());
    assert_eq!(m.bounds(), None);
    assert_eq!(m.clipped(0.0, 1.0).count(), 0);
}

#[test]
fn one_point_makes_a_zero_length_segment() {
    let m = mesh(&[[3.0, 4.0]]);
    assert_eq!(m.segments(), 1);
    assert_eq!(
        m.clipped(0.0, 0.0).collect::<Vec<_>>(),
        [([3.0, 4.0], [3.0, 4.0])]
    );
}

#[test]
fn one_quad_per_segment_with_the_shared_corner_order() {
    let m = mesh(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
    assert_eq!(m.segments(), 2);
    let v = m.vertices();
    assert_eq!(v.len(), 8);
    for quad in v.as_chunks::<4>().0 {
        let corners: Vec<_> = quad.iter().map(|v| v.corner).collect();
        assert_eq!(corners, CORNERS);
        assert!(quad.iter().all(|v| v.start == quad[0].start
            && v.end == quad[0].end
            && v.distance == quad[0].distance));
    }
    assert_eq!(v[0].distance, [0.0, 10.0]);
    assert_eq!(v[4].start, [10.0, 0.0]);
    assert_eq!(v[4].distance, [10.0, 20.0]);
    assert_eq!(m.length(), 20.0);
}

#[test]
fn bounds_cover_every_point() {
    let m = mesh(&[[5.0, -2.0], [-3.0, 8.0], [1.0, 1.0]]);
    assert_eq!(
        m.bounds(),
        Some(Bounds {
            min: [-3.0, -2.0],
            max: [5.0, 8.0]
        })
    );
    assert_eq!(
        m.bounds().unwrap().inflate(2.0),
        Bounds {
            min: [-5.0, -4.0],
            max: [7.0, 10.0]
        }
    );
}

#[test]
fn full_range_draws_the_whole_path() {
    let points = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
    let m = mesh(&points);
    let segments: Vec<_> = m.clipped(0.0, m.length()).collect();
    assert_eq!(
        segments,
        [
            (points[0], points[1]),
            (points[1], points[2]),
            (points[2], points[3])
        ]
    );
}

#[test]
fn snaking_in_clips_the_end() {
    let m = mesh(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
    let segments: Vec<_> = m.clipped(0.0, 15.0).collect();
    assert_eq!(
        segments,
        [([0.0, 0.0], [10.0, 0.0]), ([10.0, 0.0], [10.0, 5.0])]
    );
}

#[test]
fn snaking_out_clips_the_start() {
    let m = mesh(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
    let segments: Vec<_> = m.clipped(12.5, 20.0).collect();
    assert_eq!(segments, [([10.0, 2.5], [10.0, 10.0])]);
}

#[test]
fn a_range_inside_one_segment_clips_both_ends() {
    let m = mesh(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
    let segments: Vec<_> = m.clipped(2.0, 7.0).collect();
    assert_eq!(segments, [([2.0, 0.0], [7.0, 0.0])]);
}

#[test]
fn an_empty_range_is_a_point_on_each_touching_segment() {
    let m = mesh(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
    // Inside a segment: one point, drawn as a circle.
    assert_eq!(
        m.clipped(4.0, 4.0).collect::<Vec<_>>(),
        [([4.0, 0.0], [4.0, 0.0])]
    );
    // On a joint both segments touch it; Max blending draws the circle once.
    assert_eq!(
        m.clipped(10.0, 10.0).collect::<Vec<_>>(),
        [([10.0, 0.0], [10.0, 0.0]), ([10.0, 0.0], [10.0, 0.0])]
    );
}

#[test]
fn zero_length_segments_stay_at_their_start() {
    // A repeated point (a red anchor) makes a zero-length segment in the middle.
    let m = mesh(&[[0.0, 0.0], [10.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
    let segments: Vec<_> = m.clipped(0.0, 20.0).collect();
    assert_eq!(segments.len(), 3);
    assert_eq!(segments[1], ([10.0, 0.0], [10.0, 0.0]));
    assert!(
        segments
            .iter()
            .flat_map(|(a, b)| a.iter().chain(b))
            .all(|x| x.is_finite())
    );
}

#[test]
fn reversing_paths_and_self_intersections_keep_every_segment() {
    // Out and back over itself, then across: the prepass blends with Max, so
    // overlaps need no special geometry.
    let points = [
        [0.0, 0.0],
        [20.0, 0.0],
        [5.0, 0.0],
        [5.0, -10.0],
        [5.0, 10.0],
    ];
    let m = mesh(&points);
    assert_eq!(m.segments(), 4);
    assert_eq!(m.clipped(0.0, m.length()).count(), 4);
}

#[test]
fn ranges_outside_the_path_draw_nothing_past_the_ends() {
    let m = mesh(&[[0.0, 0.0], [10.0, 0.0]]);
    assert_eq!(
        m.clipped(-5.0, 30.0).collect::<Vec<_>>(),
        [([0.0, 0.0], [10.0, 0.0])]
    );
    // Lazer's `GetPathToProgress` would give a circle at the end here, but snaking
    // ranges are clamped to the path (`progressToDistance`), so it never happens.
    assert_eq!(m.clipped(11.0, 30.0).count(), 0);
}

#[test]
fn gradient_width_is_twice_the_truncated_radius() {
    let g = |r: f32| SliderGradient::new(r, |_| [1.0; 4]).width();
    assert_eq!(g(36.48), 72);
    assert_eq!(g(36.99), 72);
    assert_eq!(g(0.3), 2);
    assert_eq!(g(54.4), 108);
    assert_eq!(g(200.0), MAX_GRADIENT_TEXELS as u32);
}

#[test]
fn gradient_texels_sample_colour_at_their_progress() {
    // Width 4: progress 0, 1/3, 2/3, 1.
    let g = SliderGradient::new(2.0, |p| [p, 1.0 - p, 0.5, 1.0]);
    assert_eq!(g.width(), 4);
    // Texel 0 has zero alpha (anti-aliased edge); the others are opaque.
    assert_eq!(g.texel(0), [0, 255, 128, 0]);
    assert_eq!(g.texel(1), [85, 170, 128, 255]);
    assert_eq!(g.texel(2), [170, 85, 128, 255]);
    assert_eq!(g.texel(3), [255, 0, 128, 255]);
    assert_eq!(g.texel(4), [0; 4]);
    assert_eq!(g.texel(1000), [0; 4]);
}

#[test]
fn gradient_alpha_ramps_over_the_outer_two_percent() {
    // Width 100: texel 1 is at progress 1/99, half of the 0.02 anti-aliasing ramp.
    let g = SliderGradient::new(50.0, |_| [1.0; 4]);
    assert_eq!(g.width(), 100);
    assert_eq!(g.texel(0)[3], 0);
    assert_eq!(g.texel(1)[3], 129);
    assert_eq!(g.texel(2)[3], 255);
}

#[test]
fn bytes_round_to_nearest_and_clamp() {
    assert_eq!(byte(0.5), 128);
    assert_eq!(byte(127.4 / 255.0), 127);
    assert_eq!(byte(-1.0), 0);
    assert_eq!(byte(2.0), 255);
    assert_eq!(byte(f32::NAN), 0);
}

#[test]
fn sampling_is_linear_between_texel_centres_and_clamped() {
    let g = SliderGradient::new(2.0, |p| [p, 0.0, 0.0, 1.0]);
    let r = |v: f32| g.sample(v)[0];
    // Texel centres at 1/8, 3/8, 5/8, 7/8.
    assert_eq!(r(1.0 / 8.0), 0.0);
    assert_eq!(r(3.0 / 8.0), 85.0 / 255.0);
    assert!((r(0.5) - 127.5 / 255.0).abs() < 1e-6);
    assert_eq!(r(1.0), 1.0);
    assert_eq!(r(0.01), 0.0);
    // Zero prepass value: transparent, whatever the texel.
    assert_eq!(g.sample(0.0)[3], 0.0);
    assert!(g.sample(1.0 / 16.0)[3] == 0.0);
    assert_eq!(g.sample(0.5)[3], 1.0);
}

mod renderer {
    use super::*;
    use crate::fake_device::{Call, Fake};
    use crate::hal::{Blend, Extent};
    use crate::sprite::{MAX_QUADS_PER_DRAW, Projection};

    fn draw<'a>(projection: &'a Projection, gradient: &'a SliderGradient) -> SliderDraw<'a> {
        SliderDraw {
            projection,
            radius: 4.0,
            range: (0.0, 10.0),
            tint: [1.0; 4],
            gradient,
        }
    }

    #[test]
    fn draws_the_prepass_then_the_composite() {
        let mut d = Fake::default();
        let mut r = SliderRenderer::new(&mut d, 2).unwrap();
        let buffer = r
            .upload(&mut d, &mesh(&[[0.0, 0.0], [10.0, 0.0]]))
            .unwrap()
            .unwrap();
        let (p, g) = (
            Projection::pixels(Extent::new(100, 100)),
            SliderGradient::new(4.0, |_| [1.0; 4]),
        );
        assert!(matches!(
            r.draw(&mut d, &buffer, &draw(&p, &g), None),
            Err(SliderError::NoTarget)
        ));
        r.begin_frame(&mut d, Extent::new(100, 100)).unwrap();
        d.log.clear();
        r.draw(&mut d, &buffer, &draw(&p, &g), None).unwrap();
        let draws: Vec<_> = d
            .draws()
            .into_iter()
            .filter(|c| matches!(c, Call::Pipeline(_) | Call::DrawIndexed(_)))
            .collect();
        assert_eq!(
            draws,
            [
                Call::Pipeline(Blend::Max),
                Call::DrawIndexed(6),
                Call::Pipeline(Blend::Alpha),
                Call::DrawIndexed(6),
            ]
        );
        r.release(&mut d, buffer);
        r.destroy(&mut d);
        assert!(d.live_buffers.is_empty());
        assert!(d.live_pipelines.is_empty());
    }

    #[test]
    fn long_paths_draw_in_chunks_of_u16_quads() {
        let mut d = Fake::default();
        let mut r = SliderRenderer::new(&mut d, 1).unwrap();
        let points: Vec<[f32; 2]> = (0..=MAX_QUADS_PER_DRAW + 10)
            .map(|i| [i as f32, 0.0])
            .collect();
        let buffer = r.upload(&mut d, &mesh(&points)).unwrap().unwrap();
        assert_eq!(buffer.segments(), MAX_QUADS_PER_DRAW + 10);
        let (p, g) = (
            Projection::pixels(Extent::new(100, 100)),
            SliderGradient::new(4.0, |_| [1.0; 4]),
        );
        r.begin_frame(&mut d, Extent::new(100, 100)).unwrap();
        d.log.clear();
        r.draw(&mut d, &buffer, &draw(&p, &g), None).unwrap();
        let stride = size_of::<SliderVertex>() as u64;
        let draws: Vec<_> = d
            .draws()
            .into_iter()
            .filter(|c| matches!(c, Call::Vertices { .. } | Call::DrawIndexed(_)))
            .collect();
        assert!(matches!(
            draws[..],
            [
                Call::Vertices { offset: 0, .. },
                Call::DrawIndexed(a),
                Call::Vertices { offset, .. },
                Call::DrawIndexed(b),
                Call::Vertices { offset: 0, .. },
                Call::DrawIndexed(6),
            ] if a == MAX_QUADS_PER_DRAW * 6
                && b == 60
                && offset == u64::from(MAX_QUADS_PER_DRAW) * 4 * stride
        ));
    }

    #[test]
    fn draws_per_frame_are_limited_and_reset_by_begin_frame() {
        let mut d = Fake::default();
        let mut r = SliderRenderer::new(&mut d, 2).unwrap();
        let buffer = r
            .upload(&mut d, &mesh(&[[0.0, 0.0], [10.0, 0.0]]))
            .unwrap()
            .unwrap();
        let (p, g) = (
            Projection::pixels(Extent::new(100, 100)),
            SliderGradient::new(4.0, |_| [1.0; 4]),
        );
        for _ in 0..3 {
            r.begin_frame(&mut d, Extent::new(100, 100)).unwrap();
            r.draw(&mut d, &buffer, &draw(&p, &g), None).unwrap();
            r.draw(&mut d, &buffer, &draw(&p, &g), None).unwrap();
            assert!(matches!(
                r.draw(&mut d, &buffer, &draw(&p, &g), None),
                Err(SliderError::OutOfDraws { draws: 2 })
            ));
        }
    }

    #[test]
    fn a_destination_of_another_size_is_refused() {
        let mut d = Fake::default();
        let mut r = SliderRenderer::new(&mut d, 2).unwrap();
        let buffer = r
            .upload(&mut d, &mesh(&[[0.0, 0.0], [10.0, 0.0]]))
            .unwrap()
            .unwrap();
        let (p, g) = (
            Projection::pixels(Extent::new(8, 8)),
            SliderGradient::new(4.0, |_| [1.0; 4]),
        );
        // The fake's destination is 100x100.
        r.begin_frame(&mut d, Extent::new(8, 8)).unwrap();
        assert!(matches!(
            r.draw(&mut d, &buffer, &draw(&p, &g), None),
            Err(SliderError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn a_zero_radius_draws_nothing() {
        let mut d = Fake::default();
        let mut r = SliderRenderer::new(&mut d, 2).unwrap();
        let buffer = r
            .upload(&mut d, &mesh(&[[0.0, 0.0], [10.0, 0.0]]))
            .unwrap()
            .unwrap();
        let (p, g) = (
            Projection::pixels(Extent::new(100, 100)),
            SliderGradient::new(4.0, |_| [1.0; 4]),
        );
        r.begin_frame(&mut d, Extent::new(100, 100)).unwrap();
        d.log.clear();
        let body = SliderDraw {
            radius: 0.0,
            ..draw(&p, &g)
        };
        r.draw(&mut d, &buffer, &body, None).unwrap();
        assert!(d.draws().is_empty());
    }

    #[test]
    fn creation_failures_free_what_was_made() {
        for n in 0..8 {
            let mut d = Fake::failing_after(n);
            if SliderRenderer::new(&mut d, 2).is_err() {
                assert!(d.live_buffers.is_empty(), "after {n}");
                assert!(d.live_pipelines.is_empty(), "after {n}");
            }
        }
    }
}
