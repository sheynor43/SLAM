//! The window surface of the D3D11 backend: a flip-model swapchain (ADR-0082).

use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_FEATURE_PRESENT_ALLOW_TEARING, DXGI_MWA_NO_ALT_ENTER, DXGI_MWA_NO_WINDOW_CHANGES,
    DXGI_PRESENT, DXGI_PRESENT_ALLOW_TEARING, DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING,
    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2, IDXGIFactory5, IDXGISwapChain1,
    IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::core::{BOOL, Interface};

use super::{D3d11Surface, backend};
use crate::hal::{DeviceError, Extent};

/// Back buffers of the swapchain (ADR-0082; 2 or 3 is for #271 to measure).
const BUFFER_COUNT: u32 = 2;

/// Frames that may be queued for display.
const MAX_FRAME_LATENCY: u32 = 1;

/// How long [`SwapchainSurface::next_frame`] waits for the queue before drawing anyway,
/// e.g. while the compositor does not show the window.
const WAIT_TIMEOUT_MS: u32 = 100;

/// The window a [`SwapchainSurface`] presents to.
pub trait NativeWindow {
    /// The window's HWND, valid while `self` is alive.
    fn hwnd(&self) -> *mut c_void;

    /// The drawable size in pixels, empty while minimised. Must not allocate or block:
    /// it is read on every frame.
    fn size_in_pixels(&self) -> Extent;
}

/// A flip-model swapchain on a window (ADR-0082): no vsync, tearing where supported,
/// at most [`MAX_FRAME_LATENCY`] frame queued. Created on the draw thread with
/// `D3d11Device::new(.., |d| SwapchainSurface::new(d, window))`.
///
/// The buffers follow [`NativeWindow::size_in_pixels`]: [`D3d11Surface::next_frame`]
/// resizes them when it changes.
pub struct SwapchainSurface<W: NativeWindow> {
    // Fields drop in order: the view and the swapchain go before the window (the
    // device releases its references to the buffers before dropping the surface).
    view: Option<ID3D11RenderTargetView>,
    swapchain: IDXGISwapChain1,
    waitable: HANDLE,
    flags: DXGI_SWAP_CHAIN_FLAG,
    present_flags: DXGI_PRESENT,
    size: Extent,
    /// Whether `next_frame` handed out a buffer that `present` has not shown yet.
    drawn: bool,
    /// The last failure to resize or to get a buffer, reported by `present`.
    error: Option<(&'static str, windows::core::Error)>,
    window: W,
}

impl<W: NativeWindow> SwapchainSurface<W> {
    pub fn new(device: &ID3D11Device, window: W) -> Result<Self, DeviceError> {
        let hwnd = HWND(window.hwnd());
        let factory = factory(device).map_err(backend("DXGI factory"))?;
        let tearing = allows_tearing(&factory);
        if !tearing {
            tracing::warn!("DXGI does not allow tearing: frames wait for the display refresh");
        }
        let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;
        let mut present_flags = DXGI_PRESENT(0);
        if tearing {
            flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING;
            present_flags = DXGI_PRESENT_ALLOW_TEARING;
        }
        // A zero size takes the window's client size. The window is shown, not
        // minimised, when the device is created (`InputWindow::new_native_in`).
        let size = window.size_in_pixels();
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.width,
            Height: size.height,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFER_COUNT,
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: flags.0 as u32,
        };
        // SAFETY: `hwnd` is a live window (guaranteed by `NativeWindow`), `desc` is
        // valid, and the device belongs to the factory's adapter.
        let swapchain = unsafe {
            let swapchain = factory
                .CreateSwapChainForHwnd(device, hwnd, &desc, None, None)
                .map_err(backend("swapchain"))?;
            // Alt+Enter belongs to the window (ADR-0070), not to DXGI; and DXGI does
            // not watch the window's messages, which the main thread pumps while the
            // draw thread presents (no exclusive fullscreen to follow).
            factory
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_WINDOW_CHANGES | DXGI_MWA_NO_ALT_ENTER)
                .map_err(backend("window association"))?;
            swapchain
        };
        // SAFETY: a live swapchain. Read before taking the waitable handle, which
        // nothing would close on an early return.
        let actual = unsafe { swapchain.GetDesc1() }.map_err(backend("swapchain desc"))?;
        // SAFETY: plain calls on a live swapchain made with the waitable flag.
        let waitable = unsafe {
            let swapchain2 = swapchain
                .cast::<IDXGISwapChain2>()
                .map_err(backend("swapchain 2"))?;
            swapchain2
                .SetMaximumFrameLatency(MAX_FRAME_LATENCY)
                .map_err(backend("frame latency"))?;
            swapchain2.GetFrameLatencyWaitableObject()
        };
        Ok(Self {
            view: None,
            swapchain,
            waitable,
            flags,
            present_flags,
            size: Extent::new(actual.Width, actual.Height),
            drawn: false,
            error: None,
            window,
        })
    }

    pub fn window(&self) -> &W {
        &self.window
    }

    /// Whether frames are presented with `DXGI_PRESENT_ALLOW_TEARING`.
    pub fn tearing(&self) -> bool {
        self.present_flags == DXGI_PRESENT_ALLOW_TEARING
    }

    /// How many frames were presented, for tests. 0 if DXGI cannot tell.
    pub fn present_count(&self) -> u32 {
        // SAFETY: a live swapchain.
        unsafe { self.swapchain.GetLastPresentCount() }.unwrap_or(0)
    }

    /// The size of the back buffers.
    pub fn buffer_size(&self) -> Extent {
        self.size
    }

    /// Resizes the buffers to `size`, dropping the view first. On failure the old
    /// buffers are kept.
    fn resize(&mut self, device: &ID3D11Device, size: Extent) -> windows::core::Result<()> {
        self.view = None;
        // SAFETY: the device holds no reference to the old view (the trait's contract)
        // and this surface held the only other one; flushing releases views whose
        // destruction the runtime deferred, which ResizeBuffers would refuse.
        let resized = unsafe {
            device.GetImmediateContext().map(|c| c.Flush()).ok();
            self.swapchain.ResizeBuffers(
                0,
                size.width,
                size.height,
                DXGI_FORMAT_UNKNOWN,
                self.flags,
            )
        };
        resized?;
        self.size = size;
        Ok(())
    }

    fn create_view(&mut self, device: &ID3D11Device) -> windows::core::Result<()> {
        let mut view = None;
        // SAFETY: buffer 0 of a flip-discard swapchain is the current back buffer; the
        // out-pointer points to a live `Option`.
        unsafe {
            let buffer: ID3D11Texture2D = self.swapchain.GetBuffer(0)?;
            device.CreateRenderTargetView(&buffer, None, Some(&mut view))?;
        }
        self.view = view;
        Ok(())
    }

    /// Wraps up a frame that has no buffer: `present` then reports `error`, if any,
    /// and shows nothing.
    fn no_frame(
        &mut self,
        error: Option<(&'static str, windows::core::Error)>,
    ) -> Option<(ID3D11RenderTargetView, Extent)> {
        if error.is_some() {
            self.error = error;
        }
        None
    }
}

impl<W: NativeWindow> D3d11Surface for SwapchainSurface<W> {
    fn next_frame(&mut self, device: &ID3D11Device) -> Option<(ID3D11RenderTargetView, Extent)> {
        self.drawn = false;
        let size = self.window.size_in_pixels();
        if size.is_empty() {
            return self.no_frame(None);
        }
        if size != self.size
            && let Err(e) = self.resize(device, size)
        {
            return self.no_frame(Some(("resize buffers", e)));
        }
        if self.view.is_none()
            && let Err(e) = self.create_view(device)
        {
            return self.no_frame(Some(("back buffer view", e)));
        }
        // SAFETY: the waitable object lives as long as the swapchain. A timeout or
        // failure is ignored: the frame is drawn anyway.
        unsafe { WaitForSingleObjectEx(self.waitable, WAIT_TIMEOUT_MS, false) };
        self.drawn = true;
        self.view.clone().map(|view| (view, self.size))
    }

    fn present(&mut self) -> Result<(), String> {
        if let Some((what, e)) = self.error.take() {
            return Err(format!("DXGI {what}: {e}"));
        }
        // Showing a buffer nothing was drawn into would also present without waiting
        // for the queue, unpairing the waits from the presents.
        if !std::mem::take(&mut self.drawn) {
            return Ok(());
        }
        // SAFETY: a live swapchain. DXGI_STATUS_OCCLUDED is a success code.
        unsafe { self.swapchain.Present(0, self.present_flags) }
            .ok()
            .map_err(|e| format!("DXGI present: {e}"))
    }
}

impl<W: NativeWindow> Drop for SwapchainSurface<W> {
    fn drop(&mut self) {
        // SAFETY: the handle came from GetFrameLatencyWaitableObject, which leaves
        // closing it to the caller, and is closed once.
        unsafe { CloseHandle(self.waitable) }.ok();
    }
}

/// The factory of the device's adapter.
fn factory(device: &ID3D11Device) -> windows::core::Result<IDXGIFactory2> {
    // SAFETY: plain queries on a live device and its adapter.
    unsafe { device.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent() }
}

/// Whether presents may tear (Windows 10 1607+ with a supporting driver).
fn allows_tearing(factory: &IDXGIFactory2) -> bool {
    let Ok(factory) = factory.cast::<IDXGIFactory5>() else {
        return false;
    };
    let mut allowed = BOOL(0);
    // SAFETY: `allowed` is a BOOL, the size given.
    let checked = unsafe {
        factory.CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            (&raw mut allowed).cast(),
            size_of::<BOOL>() as u32,
        )
    };
    checked.is_ok() && allowed.as_bool()
}
