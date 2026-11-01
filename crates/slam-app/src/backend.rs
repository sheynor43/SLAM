//! The render backend, chosen once at start (ADR-0024, ADR-0080): Direct3D 11 first on
//! Windows and OpenGL 3.3 when its device or swapchain cannot be made, OpenGL 3.3
//! elsewhere. Scenes are generic over [`Backend`], so a frame runs one backend's code
//! without a `match` or dynamic dispatch.

use slam_engine::gl::SdlGlSurface;
use slam_input::{InputWindow, PriorityOutcome, PriorityRequest};
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{Device, DeviceError, Limits};

/// Which backend to use, from the command line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Renderer {
    /// Direct3D 11 on Windows with the OpenGL fallback, OpenGL elsewhere.
    #[default]
    Auto,
    OpenGl,
    /// Direct3D 11 without the fallback: failing to make it is an error.
    D3d11,
}

impl Renderer {
    pub const NAMES: &str = "auto, opengl, d3d11";

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "auto" => Some(Self::Auto),
            "opengl" => Some(Self::OpenGl),
            "d3d11" => Some(Self::D3d11),
            _ => None,
        }
    }
}

/// The kind of window a backend draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowKind {
    /// [`InputWindow::new_opengl`] and its variants.
    OpenGl,
    /// A window without OpenGL, for a backend with its own swapchain
    /// ([`InputWindow::new_native_in`]).
    Native,
}

/// A backend's surface, made on the window thread, and the device it makes on the draw
/// thread (ADR-0018, ADR-0082).
pub trait Backend: Send + Sized + 'static {
    /// Its resources are `Send` too: scenes holding them are made on other threads,
    /// while the resources exist only on the draw thread.
    type Device: Device<Texture: Send, Buffer: Send, Pipeline: Send, RenderTarget: Send> + Send;

    /// For logs and reports.
    const NAME: &str;
    const WINDOW: WindowKind;

    /// Makes the surface of `window` (of [`Backend::WINDOW`]'s kind) on the window
    /// thread. A device is made on it right away, as the draw thread will make it
    /// (D3D11: hardware adapter, debug layer in debug builds), to fall back before the
    /// draw thread starts and to read its limits, and released again for the draw
    /// thread. A failure here is what `--renderer d3d11` reports.
    fn open(window: &mut InputWindow, priority: PriorityRequest) -> Result<Opened<Self>, String>;

    /// Makes the device on the draw thread.
    fn create_device(self) -> Result<Self::Device, DeviceError>;

    /// Releases the device on the draw thread. Returns the surface when the backend
    /// can make a device on it again (OpenGL hands its context back to be destroyed on
    /// the window thread); a D3D11 swapchain goes with the device.
    fn release(device: Self::Device) -> Result<Option<Self>, DeviceError>;

    /// Adapter and API version for reports. Allocates.
    fn driver_info(device: &Self::Device) -> String;

    /// Waits until the GPU has run the frame's commands (`bench --finish`).
    fn finish(device: &mut Self::Device);
}

/// What [`Backend::open`] made.
pub struct Opened<B> {
    pub surface: B,
    pub limits: Limits,
    /// The drawable size when the surface was made.
    pub size_in_pixels: (u32, u32),
    /// The GPU priority the surface was given (ADR-0075).
    pub priority: PriorityOutcome,
}

impl Backend for SdlGlSurface {
    type Device = GlDevice<SdlGlSurface>;

    const NAME: &str = "OpenGL 3.3";
    const WINDOW: WindowKind = WindowKind::OpenGl;

    fn open(window: &mut InputWindow, priority: PriorityRequest) -> Result<Opened<Self>, String> {
        let context = window
            .create_gl_context_with(priority)
            .map_err(|e| e.to_string())?;
        let priority = context.priority().clone();
        let size_in_pixels = context.size_in_pixels();
        let device = GlDevice::new(SdlGlSurface(context), GlDebug::for_build())
            .map_err(|e| e.to_string())?;
        let limits = device.limits();
        let surface = device.into_surface().map_err(|e| e.to_string())?;
        Ok(Opened {
            surface,
            limits,
            size_in_pixels,
            priority,
        })
    }

    fn create_device(self) -> Result<Self::Device, DeviceError> {
        GlDevice::new(self, GlDebug::for_build())
    }

    fn release(device: Self::Device) -> Result<Option<Self>, DeviceError> {
        device.into_surface().map(Some)
    }

    fn driver_info(device: &Self::Device) -> String {
        device.driver_info()
    }

    fn finish(device: &mut Self::Device) {
        device.finish();
    }
}

#[cfg(windows)]
mod d3d11 {
    use slam_engine::d3d11::SdlNativeWindow;
    use slam_input::{InputWindow, PriorityOutcome, PriorityRequest};
    use slam_render::d3d11::{D3d11Adapter, D3d11Debug, D3d11Device, SwapchainSurface};
    use slam_render::{Device, DeviceError};

    use super::{Backend, Opened, WindowKind};

    fn create(
        window: SdlNativeWindow,
    ) -> Result<<SdlNativeWindow as Backend>::Device, DeviceError> {
        D3d11Device::new(D3d11Adapter::Hardware, D3d11Debug::for_build(), |device| {
            SwapchainSurface::new(device, window)
        })
    }

    impl Backend for SdlNativeWindow {
        type Device = D3d11Device<SwapchainSurface<SdlNativeWindow>>;

        const NAME: &str = "Direct3D 11";
        const WINDOW: WindowKind = WindowKind::Native;

        fn open(window: &mut InputWindow, _: PriorityRequest) -> Result<Opened<Self>, String> {
            let handle = window.native_handle().map_err(|e| e.to_string())?;
            let size_in_pixels = handle.size_in_pixels();
            let device = create(SdlNativeWindow(handle)).map_err(|e| e.to_string())?;
            let limits = device.limits();
            // Releases the swapchain and its handle (`Drop` flushes the release), so the
            // draw thread can make its own on the window.
            drop(device);
            let handle = window.native_handle().map_err(|e| e.to_string())?;
            Ok(Opened {
                surface: SdlNativeWindow(handle),
                limits,
                size_in_pixels,
                // GPU priority is only asked of EGL contexts (ADR-0075).
                priority: PriorityOutcome::Unsupported,
            })
        }

        fn create_device(self) -> Result<Self::Device, DeviceError> {
            create(self)
        }

        fn release(device: Self::Device) -> Result<Option<Self>, DeviceError> {
            drop(device);
            Ok(None)
        }

        fn driver_info(device: &Self::Device) -> String {
            device.driver_info()
        }

        fn finish(device: &mut Self::Device) {
            device.finish();
        }
    }
}

/// Code to run with the chosen backend, monomorphised for it.
pub trait WithBackend {
    type Output;

    fn run<B: Backend>(self, window: InputWindow, opened: Opened<B>) -> Self::Output;
}

/// Opens a window with `window` and its surface for the backend `renderer` picks, and
/// runs `then` with them. With [`Renderer::Auto`] on Windows, a failure to make the
/// Direct3D 11 device or swapchain is logged and its window closed before OpenGL's
/// opens.
pub fn run<W: WithBackend>(
    renderer: Renderer,
    priority: PriorityRequest,
    window: impl Fn(WindowKind) -> Result<InputWindow, String>,
    then: W,
) -> Result<W::Output, String> {
    #[cfg(windows)]
    let primary = || open::<slam_engine::d3d11::SdlNativeWindow>(&window, priority);
    #[cfg(not(windows))]
    let primary = || -> Result<(InputWindow, Opened<SdlGlSurface>), String> {
        Err("Direct3D 11 is only available on Windows".into())
    };
    let opened = choose(renderer, cfg!(windows), primary, || {
        open::<SdlGlSurface>(&window, priority)
    })?;
    Ok(match opened {
        Chosen::Primary((window, opened)) => then.run(window, opened),
        Chosen::Fallback((window, opened)) => then.run(window, opened),
    })
}

fn open<B: Backend>(
    window: &impl Fn(WindowKind) -> Result<InputWindow, String>,
    priority: PriorityRequest,
) -> Result<(InputWindow, Opened<B>), String> {
    let mut window = window(B::WINDOW)?;
    let opened = B::open(&mut window, priority).map_err(|e| format!("{}: {e}", B::NAME))?;
    tracing::info!("render backend: {}", B::NAME);
    Ok((window, opened))
}

/// Which of the platform's primary backend and the OpenGL fallback was opened.
#[derive(Debug, PartialEq, Eq)]
enum Chosen<P, F> {
    Primary(P),
    Fallback(F),
}

/// Opens the backend `renderer` asks for. `has_primary`: the platform's primary backend
/// is not OpenGL, so [`Renderer::Auto`] tries it first.
fn choose<P, F>(
    renderer: Renderer,
    has_primary: bool,
    primary: impl FnOnce() -> Result<P, String>,
    fallback: impl FnOnce() -> Result<F, String>,
) -> Result<Chosen<P, F>, String> {
    match renderer {
        Renderer::OpenGl => fallback().map(Chosen::Fallback),
        Renderer::D3d11 => primary().map(Chosen::Primary),
        Renderer::Auto if !has_primary => fallback().map(Chosen::Fallback),
        Renderer::Auto => match primary() {
            Ok(primary) => Ok(Chosen::Primary(primary)),
            Err(e) => {
                tracing::warn!("{e}; falling back to OpenGL 3.3");
                fallback().map(Chosen::Fallback)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn ok(calls: &Cell<u32>) -> impl FnOnce() -> Result<u32, String> + '_ {
        move || {
            calls.set(calls.get() + 1);
            Ok(calls.get())
        }
    }

    #[test]
    fn auto_prefers_the_primary_backend() {
        let (p, f) = (Cell::new(0), Cell::new(0));
        assert_eq!(
            choose(Renderer::Auto, true, ok(&p), ok(&f)),
            Ok(Chosen::Primary(1))
        );
        assert_eq!(f.get(), 0);
    }

    #[test]
    fn auto_falls_back_when_the_primary_fails() {
        let f = Cell::new(0);
        let failing = || -> Result<u32, String> { Err("no swapchain".into()) };
        assert_eq!(
            choose(Renderer::Auto, true, failing, ok(&f)),
            Ok(Chosen::Fallback(1))
        );
    }

    #[test]
    fn auto_uses_opengl_without_a_primary_backend() {
        let (p, f) = (Cell::new(0), Cell::new(0));
        assert_eq!(
            choose(Renderer::Auto, false, ok(&p), ok(&f)),
            Ok(Chosen::Fallback(1))
        );
        assert_eq!(p.get(), 0);
    }

    #[test]
    fn an_explicit_backend_does_not_fall_back() {
        let f = Cell::new(0);
        let failing = || -> Result<u32, String> { Err("no swapchain".into()) };
        assert_eq!(
            choose(Renderer::D3d11, true, failing, ok(&f)),
            Err::<Chosen<u32, u32>, _>("no swapchain".into())
        );
        assert_eq!(f.get(), 0);
        let p = Cell::new(0);
        assert_eq!(
            choose(Renderer::OpenGl, true, ok(&p), ok(&f)),
            Ok(Chosen::Fallback(1))
        );
        assert_eq!(p.get(), 0);
    }

    #[test]
    fn renderer_names_parse() {
        assert_eq!(Renderer::parse("auto"), Some(Renderer::Auto));
        assert_eq!(Renderer::parse("opengl"), Some(Renderer::OpenGl));
        assert_eq!(Renderer::parse("d3d11"), Some(Renderer::D3d11));
        assert_eq!(Renderer::parse("vulkan"), None);
    }
}
