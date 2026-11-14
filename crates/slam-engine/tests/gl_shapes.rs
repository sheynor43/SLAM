//! Interface shapes (ADR-0116) through the OpenGL backend on a headless window, checked by
//! reading the frame back: rounded corners, the skew, outlines, the sheen and the glow. Needs Mesa
//! with llvmpipe: `cargo test -p slam-engine --test gl_shapes -- --ignored`.

use slam_engine::gl::SdlGlSurface;
use slam_input::InputWindow;
use slam_render::atlas::{Atlas, UvRect};
use slam_render::blur::Blur;
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::shaders;
use slam_render::shape::{SHAPE_LAYOUT, Shape, ShapeBatch};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, Sprite, SpriteBatch, SpriteRenderer};
use slam_render::textures::TextureId;
use slam_render::textures::Textures;
use slam_render::{Color, Device, Extent, TextureDesc, TextureFormat, TextureUsage};

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
fn draws_rounded_skewed_outlined_shining_and_glowing_shapes() {
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
        // Shining, grey: 30x26 at (42, 34), the sheen's band in its middle.
        batch.push_shape(&Shape {
            sheen: 1.0,
            phase: 0.5,
            colour: Color::rgb(0.25, 0.25, 0.25),
            ..shape([42.0, 34.0], [30.0, 26.0])
        });
        // Glowing: 6x10 at (80, 40), 6 px of glow, no pulse.
        batch.push_shape(&Shape {
            glow: 6.0,
            ..shape([80.0, 40.0], [6.0, 10.0])
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

        // Shining: the middle is lit, the sides keep the grey.
        assert!(frame.red(57, 47) > 200, "{}", frame.red(57, 47));
        assert!(
            (60..=70).contains(&frame.red(44, 47)),
            "{}",
            frame.red(44, 47)
        );
        assert!(
            (60..=70).contains(&frame.red(70, 47)),
            "{}",
            frame.red(70, 47)
        );
        // Glowing: nothing inside, bright just outside, fading further out.
        assert_eq!(frame.red(83, 45), 0);
        let near = frame.red(87, 45);
        let far = frame.red(90, 45);
        assert!(near > 100 && far < near && far > 0, "{near} {far}");
        assert_eq!(frame.red(93, 45), 0);

        // The blur (ADR-0123): an image white on top and black below, blurred and drawn
        // over the whole window, stays white on top, black below, and grey between.
        let mut source = vec![0u8; 16 * 16 * 4];
        for (i, px) in source.chunks_mut(4).enumerate() {
            let white = i / 16 < 8;
            px.copy_from_slice(&if white { [255; 4] } else { [0, 0, 0, 255] });
        }
        let halves = d
            .create_texture(
                &TextureDesc {
                    size: Extent::new(16, 16),
                    format: TextureFormat::Rgba8,
                    usage: TextureUsage::SAMPLED,
                },
                Some(&source),
            )
            .expect("texture");
        let mut blur = Blur::new(&mut d).expect("blur");
        let size = d.begin_frame(Color::rgb(0.0, 0.0, 0.0));
        blur.run(&mut d, &halves, UvRect::FULL, Extent::new(32, 32), 6.0)
            .expect("blur run");
        d.begin_pass(None, None);
        let blurred = blur.result(&d).expect("a result");
        let mut sprites = SpriteRenderer::new(&mut d, DEFAULT_SLOTS).expect("sprites");
        let mut over = SpriteBatch::with_capacity(1);
        over.push(&Sprite::new(
            TextureId(0),
            UvRect::FULL,
            [0.0, 0.0],
            [W as f32, H as f32],
        ));
        sprites
            .draw(&mut d, &over, &Projection::pixels(size), |_| Some(blurred))
            .expect("draw");
        let frame = Frame(d.read_frame(size));
        d.present().expect("present");
        let column = |y| frame.red(W / 2, y);
        assert!(column(2) > 240, "top {}", column(2));
        assert!(column(H - 3) < 15, "bottom {}", column(H - 3));
        let middle = column(H / 2);
        assert!((60..=200).contains(&middle), "edge {middle}");
        sprites.destroy(&mut d);
        blur.destroy(&mut d);
        d.destroy_texture(halves);

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
