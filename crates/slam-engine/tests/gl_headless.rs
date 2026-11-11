//! OpenGL on a headless window: SDL's offscreen driver on Mesa's software EGL device, the
//! setup of the screenshot tests. Needs Mesa with llvmpipe:
//! `cargo test -p slam-engine --test gl_headless -- --ignored`.

use slam_engine::gl::SdlGlSurface;
use slam_input::InputWindow;
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{Color, Device, Extent};

// One test per binary: SDL is process-global.
#[test]
#[ignore = "needs Mesa with llvmpipe"]
fn draws_on_llvmpipe_and_reads_the_frame_back() {
    let mut window =
        InputWindow::new_headless_opengl("SLAM headless test", 48, 32).expect("window");
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));
    let surface = std::thread::spawn(move || {
        let mut device = GlDevice::new(surface, GlDebug::On).expect("GL device");
        let info = device.driver_info();
        assert!(info.contains("llvmpipe"), "renderer: {info}");
        let size = device.begin_frame(Color::rgb(0.0, 0.0, 1.0));
        assert_eq!(size, Extent::new(48, 32));
        let pixels = device.read_frame(size);
        assert_eq!(pixels.len(), 48 * 32 * 4);
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 0, 255, 255])
        );
        device.present().expect("present");
        device.into_surface().expect("release")
    })
    .join()
    .expect("draw thread");
    drop(surface);
    drop(window);
}
