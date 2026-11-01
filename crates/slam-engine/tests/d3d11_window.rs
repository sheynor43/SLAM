//! The D3D11 swapchain surface on a real SDL window, driven from another thread as the
//! draw thread does (ADR-0082). Runs on the WARP software adapter; needs a desktop
//! session (the Windows CI runner has one).
#![cfg(windows)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use slam_engine::d3d11::SdlNativeWindow;
use slam_input::{InputWindow, WindowError};
use slam_render::d3d11::{D3d11Adapter, D3d11Debug, D3d11Device, NativeWindow, SwapchainSurface};
use slam_render::{Color, Device, Extent, RenderTargetDesc, TextureFormat};
use slam_testkit::count_allocs;

#[path = "common/d3d11.rs"]
mod common;

use common::{assert_no_debug_messages, require_debug_layer};

slam_testkit::install_counting_allocator!();

/// The SDL window with a size the test sets, standing in for resize events: the event
/// loop does not run here. Zero width and height means the window's own size.
struct Resizable {
    window: SdlNativeWindow,
    size: Arc<AtomicU64>,
}

impl NativeWindow for Resizable {
    fn hwnd(&self) -> *mut std::ffi::c_void {
        self.window.hwnd()
    }

    fn size_in_pixels(&self) -> Extent {
        match self.size.load(Ordering::Relaxed) {
            0 => self.window.size_in_pixels(),
            u64::MAX => Extent::default(),
            packed => Extent::new((packed >> 32) as u32, packed as u32),
        }
    }
}

fn set_size(size: &AtomicU64, extent: Extent) {
    size.store(
        (extent.width as u64) << 32 | extent.height as u64,
        Ordering::Relaxed,
    );
}

// One test per binary: SDL is process-global.
#[test]
fn presents_resizes_and_skips_minimised_frames() {
    let mut window = InputWindow::new_native_in("SLAM D3D11 test", Default::default(), [0.5, 0.5])
        .expect("window");
    let handle = window.native_handle().expect("native handle");
    assert!(matches!(
        window.native_handle(),
        Err(WindowError::SurfaceExists)
    ));
    assert!(matches!(
        window.create_gl_context(),
        Err(WindowError::NotOpenGl)
    ));

    let size = Arc::new(AtomicU64::new(0));
    let shared = Arc::clone(&size);
    let handle = std::thread::spawn(move || {
        let resizable = Resizable {
            window: SdlNativeWindow(handle),
            size: shared,
        };
        let mut d = D3d11Device::new(D3d11Adapter::Warp, D3d11Debug::On, |d| {
            SwapchainSurface::new(d, resizable)
        })
        .expect("D3D11 device and swapchain");
        require_debug_layer(&d);
        // Which present path runs depends on the system; show it in the test log.
        eprintln!("tearing: {}", d.surface().tearing());

        let own = d.surface().window().window.size_in_pixels();
        assert!(!own.is_empty(), "the window has a drawable size");
        assert_eq!(d.begin_frame(Color::BLACK), own);
        assert_eq!(d.surface().buffer_size(), own);
        assert_no_debug_messages(&mut d);
        d.present().expect("present");

        // Steady frames allocate nothing (debug messages, if any, would).
        for _ in 0..3 {
            d.begin_frame(Color::BLACK);
            d.present().expect("present");
        }
        let (result, stats) = count_allocs(|| {
            d.begin_frame(Color::BLACK);
            d.present()
        });
        result.expect("present");
        assert_eq!(stats.total(), 0, "a frame allocated: {stats}");

        // Resize while a render target, not the surface, was bound last; then again
        // right away, so the view is made anew each time.
        let target = d
            .create_render_target(&RenderTargetDesc {
                size: Extent::new(8, 8),
                format: TextureFormat::Rgba8,
            })
            .expect("render target");
        d.begin_pass(Some(&target), Some(Color::BLACK));
        let resized = Extent::new(32, 48);
        set_size(&d.surface().window().size, resized);
        assert_eq!(d.begin_frame(Color::rgb(1.0, 0.0, 0.0)), resized);
        assert_eq!(d.surface().buffer_size(), resized);
        assert_no_debug_messages(&mut d);
        d.present().expect("present after resize");
        d.destroy_render_target(target);
        for size in [Extent::new(40, 24), resized] {
            set_size(&d.surface().window().size, size);
            assert_eq!(d.begin_frame(Color::BLACK), size);
            assert_eq!(d.surface().buffer_size(), size);
            d.present().expect("present after resize");
        }
        assert_no_debug_messages(&mut d);

        // Minimised: nothing to draw on, and the buffers keep their size.
        d.surface().window().size.store(u64::MAX, Ordering::Relaxed);
        let presented = d.surface().present_count();
        assert_eq!(d.begin_frame(Color::BLACK), Extent::default());
        assert_eq!(d.surface().buffer_size(), resized);
        d.present().expect("present while minimised");
        assert_eq!(d.surface().present_count(), presented, "nothing shown");

        d.surface().window().size.store(0, Ordering::Relaxed);
        assert_eq!(d.begin_frame(Color::BLACK), own);
        assert_no_debug_messages(&mut d);
        d.present().expect("present after restore");
        assert_eq!(d.surface().present_count(), presented + 1);
    })
    .join();
    handle.expect("draw thread");
    drop(size);
    drop(window);
}
