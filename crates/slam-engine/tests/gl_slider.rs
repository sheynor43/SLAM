//! Slider bodies through the OpenGL backend (ADR-0041): the distance prepass with Max
//! blending, the gradient composite and snaking, checked by reading pixels back.
//! Needs a display and a GL 3.3 driver:
//! `cargo test -p slam-engine --test gl_slider -- --ignored`.

use glow::HasContext;
use slam_engine::gl::SdlGlSurface;
use slam_input::InputWindow;
use slam_render::gl::GlRenderTarget;
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::slider::{
    SliderBuffer, SliderDraw, SliderError, SliderGradient, SliderMesh, SliderRenderer,
};
use slam_render::sprite::Projection;
use slam_render::{Color, Device, Extent, RenderTargetDesc, TextureFormat};
use slam_testkit::count_allocs;

slam_testkit::install_counting_allocator!();

const SIZE: u32 = 64;
const RADIUS: f32 = 8.0;

/// Reads one pixel of the bound render target, rows from the top (ADR-0019).
fn pixel(device: &GlDevice<SdlGlSurface>, x: i32, y: i32) -> [u8; 4] {
    let mut pixel = [0u8; 4];
    let gl = device.gl();
    // SAFETY: the context is current on this thread; `pixel` holds one RGBA8 pixel.
    unsafe {
        gl.read_buffer(glow::COLOR_ATTACHMENT0);
        gl.read_pixels(
            x,
            y,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixel)),
        );
    }
    pixel
}

#[track_caller]
fn assert_rgb(actual: [u8; 4], expected: [u8; 3]) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= 2);
    assert!(close, "pixel {actual:?}, expected about {expected:?}");
}

/// Straight segments through `points` (pixels of the target).
fn mesh(points: &[[f32; 2]]) -> SliderMesh {
    let mut cumulative = vec![0.0f64];
    for w in points.windows(2) {
        let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        cumulative.push(cumulative[cumulative.len() - 1] + f64::from(d));
    }
    SliderMesh::new(points, &cumulative)
}

type Gl = GlDevice<SdlGlSurface>;

/// One frame: clears `target` to black and draws the bodies into it.
fn frame(
    d: &mut Gl,
    sliders: &mut SliderRenderer<Gl>,
    target: &GlRenderTarget,
    draws: &[(&SliderBuffer<Gl>, SliderDraw<'_>)],
) {
    sliders
        .begin_frame(d, Extent::new(SIZE, SIZE))
        .expect("begin frame");
    d.begin_pass(Some(target), Some(Color::BLACK));
    for (buffer, draw) in draws {
        sliders.draw(d, buffer, draw, Some(target)).expect("draw");
    }
}

fn body<'a>(
    projection: &'a Projection,
    gradient: &'a SliderGradient,
    range: (f32, f32),
    tint: [f32; 4],
) -> SliderDraw<'a> {
    SliderDraw {
        projection,
        radius: RADIUS,
        range,
        tint,
        gradient,
    }
}

/// Straight-alpha byte channel of `c` blended over black.
fn over_black(c: f32, alpha: f32) -> u8 {
    (c * alpha * 255.0).round() as u8
}

// One test per binary: SDL is process-global.
#[test]
#[ignore = "needs a display and an OpenGL 3.3 driver"]
fn draws_slider_bodies() {
    let mut window = InputWindow::new_opengl("SLAM slider test", 64, 64).expect("window");
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));

    let surface = std::thread::spawn(move || {
        let mut d = GlDevice::new(surface, GlDebug::On).expect("GL device");
        let size = Extent::new(SIZE, SIZE);
        let target = d
            .create_render_target(&RenderTargetDesc {
                size,
                format: TextureFormat::Rgba8,
            })
            .expect("target");
        let mut sliders = SliderRenderer::new(&mut d, 4).expect("slider renderer");
        let projection = Projection::pixels(size);

        let red = SliderGradient::new(RADIUS, |_| [1.0, 0.0, 0.0, 1.0]);
        let half_red = SliderGradient::new(RADIUS, |_| [1.0, 0.0, 0.0, 0.5]);
        // Red rises from the edge to the centre.
        let ramp = SliderGradient::new(RADIUS, |p| [p, 0.0, 0.0, 1.0]);

        let straight = sliders
            .upload(&mut d, &mesh(&[[10.0, 32.0], [54.0, 32.0]]))
            .expect("upload")
            .expect("not empty");
        // Out and back over itself, then down across the first leg.
        let folded = sliders
            .upload(
                &mut d,
                &mesh(&[[10.0, 20.0], [54.0, 20.0], [30.0, 20.0], [30.0, 50.0]]),
            )
            .expect("upload")
            .expect("not empty");
        assert!(
            sliders
                .upload(&mut d, &SliderMesh::default())
                .expect("upload")
                .is_none()
        );

        let p = &projection;

        // A whole straight body: round caps, nothing past the radius.
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[(&straight, body(p, &red, (0.0, 44.0), [1.0; 4]))],
        );
        assert_rgb(pixel(&d, 30, 32), [255, 0, 0]);
        assert_rgb(pixel(&d, 30, 32 + 6), [255, 0, 0]);
        assert_rgb(pixel(&d, 30, 32 + 10), [0, 0, 0]);
        // The caps are round: inside along the axis, outside at the corner.
        assert_rgb(pixel(&d, 4, 32), [255, 0, 0]);
        assert_rgb(pixel(&d, 3, 25), [0, 0, 0]);

        // The gradient follows the prepass value 1 − distance / radius, sampled as
        // `SliderGradient::sample` (pixel centres are half a pixel off the axis).
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[(&straight, body(p, &ramp, (0.0, 44.0), [1.0; 4]))],
        );
        for dy in 0..8 {
            let distance = dy as f32 + 0.5;
            let [r, _, _, a] = ramp.sample(1.0 - distance / RADIUS);
            assert_rgb(pixel(&d, 30, 32 + dy), [over_black(r, a), 0, 0]);
        }

        // Overlapping parts blend once: half-transparent red over black stays half
        // red where the path folds back and where it crosses itself.
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[(&folded, body(p, &half_red, (0.0, 98.0), [1.0; 4]))],
        );
        let half = over_black(1.0, 128.0 / 255.0);
        assert_rgb(pixel(&d, 40, 20), [half, 0, 0]);
        assert_rgb(pixel(&d, 30, 20), [half, 0, 0]);
        assert_rgb(pixel(&d, 30, 40), [half, 0, 0]);
        // The tint multiplies the result.
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[(&folded, body(p, &red, (0.0, 98.0), [0.0, 1.0, 1.0, 0.5]))],
        );
        assert_rgb(pixel(&d, 40, 20), [0, 0, 0]);

        // Two sliders: each is its own body, so they do blend over each other.
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[
                (&straight, body(p, &half_red, (0.0, 44.0), [1.0; 4])),
                (&straight, body(p, &half_red, (0.0, 44.0), [1.0; 4])),
            ],
        );
        let twice = over_black(1.0, 1.0 - (1.0 - 128.0 / 255.0) * (1.0 - 128.0 / 255.0));
        assert_rgb(pixel(&d, 30, 32), [twice, 0, 0]);

        // Snaking in: only the first 20 pixels of path length.
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[(&straight, body(p, &red, (0.0, 20.0), [1.0; 4]))],
        );
        assert_rgb(pixel(&d, 25, 32), [255, 0, 0]);
        assert_rgb(pixel(&d, 30 + RADIUS as i32 + 2, 32), [0, 0, 0]);
        // Snaking out: the last 10.
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[(&straight, body(p, &red, (34.0, 44.0), [1.0; 4]))],
        );
        assert_rgb(pixel(&d, 50, 32), [255, 0, 0]);
        assert_rgb(pixel(&d, 44 - RADIUS as i32 - 2, 32), [0, 0, 0]);
        // An empty range is a circle at that point of the path.
        frame(
            &mut d,
            &mut sliders,
            &target,
            &[(&straight, body(p, &red, (0.0, 0.0), [1.0; 4]))],
        );
        assert_rgb(pixel(&d, 10, 32), [255, 0, 0]);
        assert_rgb(pixel(&d, 10 + RADIUS as i32 + 2, 32), [0, 0, 0]);

        // At most `per_frame` draws a frame.
        sliders.begin_frame(&mut d, size).expect("begin frame");
        let draw = body(p, &red, (0.0, 44.0), [1.0; 4]);
        for _ in 0..4 {
            sliders
                .draw(&mut d, &straight, &draw, Some(&target))
                .expect("draw");
        }
        assert!(matches!(
            sliders.draw(&mut d, &straight, &draw, Some(&target)),
            Err(SliderError::OutOfDraws { draws: 4 })
        ));

        // Steady frames allocate nothing.
        let ((), allocs) = count_allocs(|| {
            for _ in 0..50 {
                sliders.begin_frame(&mut d, size).expect("begin frame");
                d.begin_pass(Some(&target), Some(Color::BLACK));
                for _ in 0..4 {
                    sliders
                        .draw(&mut d, &folded, &draw, Some(&target))
                        .expect("draw");
                }
            }
        });
        assert_eq!(allocs.total(), 0, "steady slider frames: {allocs}");

        // Into the surface (the other y-flip variant): a body in the upper quarter of
        // a 64x64 scene stretched over the window, read back with rows from the bottom.
        let upper = sliders
            .upload(&mut d, &mesh(&[[8.0, 16.0], [56.0, 16.0]]))
            .expect("upload")
            .expect("not empty");
        let window = d.begin_frame(Color::BLACK);
        assert!(!window.is_empty(), "drawable size {window:?}");
        sliders.begin_frame(&mut d, window).expect("begin frame");
        let scene = Projection::pixels(Extent::new(SIZE, SIZE));
        sliders
            .draw(
                &mut d,
                &upper,
                &body(&scene, &red, (0.0, 48.0), [1.0; 4]),
                None,
            )
            .expect("draw");
        let (w, h) = (window.width as f32 / 64.0, window.height as f32 / 64.0);
        let back = |d: &Gl, x: f32, y: f32| {
            let mut px = [0u8; 4];
            // SAFETY: the context is current on this thread; `px` holds one RGBA8 pixel.
            unsafe {
                let gl = d.gl();
                gl.read_buffer(glow::BACK);
                gl.read_pixels(
                    (x * w) as i32,
                    window.height as i32 - 1 - (y * h) as i32,
                    1,
                    1,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut px)),
                );
            }
            px
        };
        assert_rgb(back(&d, 32.0, 16.0), [255, 0, 0]);
        assert_rgb(back(&d, 32.0, 48.0), [0, 0, 0]);
        d.present().expect("present");
        sliders.release(&mut d, upper);

        sliders.release(&mut d, straight);
        sliders.release(&mut d, folded);
        sliders.destroy(&mut d);
        d.destroy_render_target(target);
        d.into_surface().expect("release")
    })
    .join()
    .expect("draw thread");
    drop(surface);
    drop(window);
}
