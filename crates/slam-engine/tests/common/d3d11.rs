//! Helpers shared by the Direct3D 11 tests: a device on the WARP software adapter with
//! an offscreen surface, read-back of the surface and of render targets, pixel probes.
#![allow(dead_code)]

use slam_render::d3d11::{D3d11Adapter, D3d11Debug, D3d11Device, D3d11RenderTarget};
use slam_render::d3d11::{D3d11Surface, OffscreenSurface};
use slam_render::{Device, Extent};

pub type TestDevice = D3d11Device<OffscreenSurface>;

/// A debug-layer device on WARP whose surface is `width` x `height` (no window
/// manager is involved, so the surface keeps exactly this size).
pub fn device(width: u32, height: u32) -> TestDevice {
    let device = D3d11Device::new(D3d11Adapter::Warp, D3d11Debug::On, |d| {
        OffscreenSurface::new(d, Extent::new(width, height))
    })
    .expect("D3D11 device on WARP");
    require_debug_layer(&device);
    device
}

/// Fails if `SLAM_REQUIRE_D3D11_DEBUG` is set but the device has no debug layer.
#[track_caller]
pub fn require_debug_layer<S: D3d11Surface>(device: &D3d11Device<S>) {
    if std::env::var_os("SLAM_REQUIRE_D3D11_DEBUG").is_some_and(|v| !v.is_empty()) {
        assert!(
            device.has_debug_layer(),
            "the D3D11 debug layer is missing: install the Graphics Tools optional Windows \
             feature (SLAM_REQUIRE_D3D11_DEBUG is set)"
        );
    }
}

/// RGBA8 pixels of the surface, rows top first (ADR-0019).
pub fn read_surface(d: &mut TestDevice) -> Vec<u8> {
    let texture = d.surface().texture().clone();
    d.read_texture(&texture).expect("read surface")
}

/// RGBA8 pixels of a render target, rows top first (ADR-0019).
pub fn read_target(d: &mut TestDevice, target: &D3d11RenderTarget) -> Vec<u8> {
    let texture = d.render_target_texture(target).raw().clone();
    d.read_texture(&texture).expect("read render target")
}

/// One pixel of a tightly packed RGBA8 image, `x` and `y` from the top left.
pub fn pixel(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = (y as usize * width as usize + x as usize) * 4;
    pixels[i..i + 4].try_into().expect("pixel inside the image")
}

#[track_caller]
pub fn assert_near(actual: [u8; 4], expected: [u8; 4]) {
    let close = actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= 2);
    assert!(close, "pixel {actual:?}, expected about {expected:?}");
}

/// Fails if the debug layer reported a warning or error since the last call.
/// `present` drains (and only logs) these messages, so call this before it.
#[track_caller]
pub fn assert_no_debug_messages<S: D3d11Surface>(d: &mut D3d11Device<S>) {
    let messages = d.take_debug_messages();
    assert!(
        messages.is_empty(),
        "D3D11 debug layer:\n{}",
        messages.join("\n")
    );
}

/// One pixel of a render target; `x` and `y` from the top left.
pub fn target_pixel(d: &mut TestDevice, target: &D3d11RenderTarget, x: u32, y: u32) -> [u8; 4] {
    let pixels = read_target(d, target);
    pixel(&pixels, target.size().width, x, y)
}

/// One pixel of the surface (`width` wide); `x` and `y` from the top left.
pub fn surface_pixel(d: &mut TestDevice, width: u32, x: u32, y: u32) -> [u8; 4] {
    let pixels = read_surface(d);
    pixel(&pixels, width, x, y)
}
