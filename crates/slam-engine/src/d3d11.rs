//! Glue between the SDL window's native handle (`slam-input`) and the D3D11 swapchain
//! (`slam-render`), which do not depend on each other (ADR-0082).

use std::ffi::c_void;

use slam_input::NativeHandle;
use slam_render::Extent;
use slam_render::d3d11::NativeWindow;

/// [`NativeHandle`] as a [`NativeWindow`] for [`slam_render::d3d11::SwapchainSurface`].
pub struct SdlNativeWindow(pub NativeHandle);

impl NativeWindow for SdlNativeWindow {
    fn hwnd(&self) -> *mut c_void {
        self.0.hwnd()
    }

    fn size_in_pixels(&self) -> Extent {
        let (width, height) = self.0.size_in_pixels();
        Extent::new(width, height)
    }
}
