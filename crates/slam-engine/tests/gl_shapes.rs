//! Interface shapes (ADR-0116) through the OpenGL backend on a headless window, checked by
//! reading the frame back: rounded corners, the skew, outlines and the pattern. Needs Mesa
//! with llvmpipe: `cargo test -p slam-engine --test gl_shapes -- --ignored`.

use slam_engine::gl::SdlGlSurface;
use slam_input::InputWindow;
use slam_render::atlas::Atlas;
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::shaders;
use slam_render::shape::{SHAPE_LAYOUT, Shape, ShapeBatch};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::{Color, Device, Extent, TextureFormat};

const W: u32 = 96;
const H: u32 = 64;

struct Frame(Vec<u8>);

impl Frame {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * W + x) * 4) as usize;
        self.0[i..i + 4].try_into().expect("four bytes")
    }

    /// Red channel: shapes are drawn white on black.
    fn red(&self, x: u32, y: u32) -> u8 {
        self.at(x, y)[0]
    }
}

// One test per binary: SDL is process-global.
#[test]
#[ignore = "needs Mesa with llvmpipe"]
fn draws_rounded_skewed_outlined_and_patterned_shapes() {
    let mut window =
        InputWindow::new_headless_opengl("SLAM shape test", W as i32, H as i32).expect("window");
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));
    let surface = std::thread::spawn(move || {
        let mut d = GlDevice::new(surface, GlDebug::On).expect("GL device");
        let mut textures = Textures::new();
        let mut atlas = Atlas::new(Extent::new(16, 16), TextureFormat::Rgba8);
        let white = atlas
            .add(&mut d, &mut textures, Extent::new(1, 1), &[255; 4])
            .expect("white");
        let mut renderer =
            SpriteRenderer::with_layout(&mut d, DEFAULT_SLOTS, &shaders::UI_SHAPE, SHAPE_LAYOUT)
                .expect("shape renderer");
        let shape =
            |position, size| Shape::new(white.texture, white.uv, position, size, Color::WHITE);
        let mut batch = ShapeBatch::with_capacity(8);
        // Rounded: 32x24 at (4, 4), radius 10.
        batch.push_shape(&Shape {
            radius: 10.0,
            ..shape([4.0, 4.0], [32.0, 24.0])
        });
        // Skewed 45°: 20x20 at (56, 4), the top edge 10 px right of the centre line.
        batch.push_shape(&Shape {
            skew: std::f32::consts::FRAC_PI_4,
            ..shape([56.0, 4.0], [20.0, 20.0])
        });
        // Outlined, 3 px: 30x26 at (4, 34).
        batch.push_shape(&Shape {
            outline: 3.0,
            ..shape([4.0, 34.0], [30.0, 26.0])
        });
        // Patterned, grey: 50x26 at (42, 34).
        batch.push_shape(&Shape {
            pattern: 1.0,
            pattern_phase: 0.3,
            colour: Color::rgb(0.25, 0.25, 0.25),
            ..shape([42.0, 34.0], [50.0, 26.0])
        });

        let size = d.begin_frame(Color::rgb(0.0, 0.0, 0.0));
        assert_eq!(size, Extent::new(W, H));
        renderer.begin_frame();
        renderer
            .draw(&mut d, &batch, &Projection::pixels(size), |id| {
                textures.get(id)
            })
            .expect("draw");
        let frame = Frame(d.read_frame(size));
        d.present().expect("present");

        // Rounded: the middle and the middle of an edge are filled, the very corner is
        // not, a point on the corner's diagonal inside the arc is.
        assert_eq!(frame.red(20, 16), 255);
        assert_eq!(frame.red(4, 16), 255);
        assert_eq!(frame.red(4, 4), 0);
        assert_eq!(frame.red(35, 27), 0);
        assert_eq!(frame.red(9, 9), 255);

        // Skewed: the top row starts about 10 px right, the bottom row 10 px left.
        assert_eq!(frame.red(57, 5), 0);
        assert_eq!(frame.red(70, 5), 255);
        assert_eq!(frame.red(50, 23), 255);
        assert_eq!(frame.red(70, 23), 0);

        // Outlined: the edge band is drawn, the inside is not.
        assert_eq!(frame.red(5, 47), 255);
        assert_eq!(frame.red(19, 47), 0);
        assert_eq!(frame.red(19, 35), 255);

        // Patterned: some triangles lighten the grey, the rest keep it.
        let reds: Vec<u8> = (44..90)
            .flat_map(|x| (36..58).map(move |y| (x, y)))
            .map(|(x, y)| frame.red(x, y))
            .collect();
        let base = reds.iter().copied().min().expect("pixels");
        assert!((60..=70).contains(&base), "pattern base {base}");
        assert!(reds.iter().any(|&r| r > base + 40), "no lit triangle");
        assert!(reds.iter().filter(|&&r| r == base).count() > reds.len() / 3);

        renderer.destroy(&mut d);
        for texture in textures.drain() {
            d.destroy_texture(texture);
        }
        d.into_surface().expect("release")
    })
    .join()
    .expect("draw thread");
    drop(surface);
    drop(window);
}
