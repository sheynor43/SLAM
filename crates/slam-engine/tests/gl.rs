//! OpenGL backend on a real window, driven from another thread as the draw thread
//! does (ADR-0018). Needs a display and a GL 3.3 driver:
//! `cargo test -p slam-engine --test gl -- --ignored`.

use glow::HasContext;
use slam_engine::gl::SdlGlSurface;
use slam_input::InputWindow;
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{
    Color, DescError, Device, DeviceError, Extent, TextureDesc, TextureFormat, TextureUsage,
};
use slam_testkit::count_allocs;

slam_testkit::install_counting_allocator!();

/// Reads the bottom-left pixel of the back buffer.
fn back_pixel(device: &GlDevice<SdlGlSurface>) -> [u8; 4] {
    let mut pixel = [0u8; 4];
    let gl = device.gl();
    // SAFETY: the context is current on this thread; `pixel` holds one RGBA8 pixel.
    unsafe {
        gl.read_buffer(glow::BACK);
        gl.read_pixels(
            0,
            0,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixel)),
        );
    }
    pixel
}

// One test per binary: SDL is process-global.
#[test]
#[ignore = "needs a display and an OpenGL 3.3 driver"]
fn clears_and_presents_from_another_thread() {
    let mut window = InputWindow::new_opengl("SLAM GL test", 64, 64).expect("window");
    let surface = SdlGlSurface(window.create_gl_context().expect("GL context"));

    let surface = std::thread::spawn(move || {
        let mut device = GlDevice::new(surface, GlDebug::On).expect("GL device");
        assert!(device.limits().max_texture_size >= 1024);

        let desc = TextureDesc {
            size: Extent::new(2, 2),
            format: TextureFormat::Rgba8,
            usage: TextureUsage::SAMPLED,
        };
        let texture = device
            .create_texture(&desc, Some(&[255; 16]))
            .expect("texture");
        assert_eq!(texture.desc(), &desc);
        device.destroy_texture(texture);
        assert!(matches!(
            device.create_texture(&desc, Some(&[0; 3])),
            Err(DeviceError::Desc(DescError::DataSize { .. }))
        ));

        let size = device.begin_frame(Color::rgb(1.0, 0.0, 0.0));
        assert!(!size.is_empty(), "drawable size {size:?}");
        assert_eq!(back_pixel(&device), [255, 0, 0, 255]);
        device.present().expect("present");

        let (result, stats) = count_allocs(|| {
            for i in 0..100 {
                device.begin_frame(Color::rgb(0.0, (i % 2) as f32, 1.0));
                device.present()?;
            }
            Ok::<_, DeviceError>(())
        });
        result.expect("present");
        assert_eq!(stats.total(), 0, "frame loop allocated: {stats}");

        device.into_surface().expect("release")
    })
    .join()
    .expect("draw thread");

    // The released surface can be taken over again, with debug output re-installed
    // on the new device (the old callback was removed while its context was current).
    let mut device = GlDevice::new(surface, GlDebug::On).expect("second GL device");
    device.begin_frame(Color::BLACK);
    device.present().expect("present");
    let surface = device.into_surface().expect("release");

    drop(surface);
    drop(window);
}
