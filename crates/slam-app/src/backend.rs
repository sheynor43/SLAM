//! The render backend, chosen once at start (ADR-0024, ADR-0080, ADR-0104, ADR-0105):
//! OpenGL 3.3 by default, on Windows with Direct3D 11 when OpenGL cannot start; on
//! Windows Direct3D 11 when asked for, from the settings with OpenGL 3.3 when its
//! device or swapchain cannot be made. Scenes are generic over [`Backend`], so
//! a frame runs one backend's code without a `match` or dynamic dispatch.

use slam_engine::gl::SdlGlSurface;
use slam_input::{InputWindow, PriorityOutcome, PriorityRequest};
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{Device, DeviceError, Limits};

/// Which backend to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Renderer {
    /// The platform default: OpenGL 3.3 (ADR-0104), on Windows with the Direct3D 11
    /// fallback (the settings, ADR-0105).
    #[default]
    Auto,
    /// OpenGL 3.3 without the fallback (the command line).
    OpenGl,
    /// Direct3D 11 without the fallback (the command line): failing to make it is an
    /// error.
    D3d11,
    /// Direct3D 11 with the OpenGL fallback (the settings, ADR-0104); OpenGL where there
    /// is no Direct3D 11.
    PreferD3d11,
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
        let mut surface = device.into_surface().map_err(|e| e.to_string())?;
        hand_off(&mut surface)?;
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

/// Makes the context current on another thread and releases it there, as the draw
/// thread will (ADR-0105): on Windows `wglMakeCurrent` has failed there although the
/// window thread made the context current, and that failure can still fall back.
fn hand_off(surface: &mut SdlGlSurface) -> Result<(), String> {
    let context = &mut surface.0;
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("slam-gl-handoff".into())
            .spawn_scoped(scope, || {
                context.make_current()?;
                context.release_current()
            })
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "the thread panicked".to_owned())?
    })
    .map_err(|e| format!("handing the GL context to another thread: {e}"))
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
/// runs `then` with them. On Windows, with [`Renderer::Auto`] a failure to start OpenGL
/// is logged and its window closed before Direct3D 11's opens; with
/// [`Renderer::PreferD3d11`] the other way round.
pub fn run<W: WithBackend>(
    renderer: Renderer,
    priority: PriorityRequest,
    window: impl Fn(WindowKind) -> Result<InputWindow, String>,
    then: W,
) -> Result<W::Output, String> {
    #[cfg(windows)]
    let d3d11 = || open::<slam_engine::d3d11::SdlNativeWindow>(&window, priority);
    #[cfg(not(windows))]
    let d3d11 = || -> Result<(InputWindow, Opened<SdlGlSurface>), String> {
        Err("Direct3D 11 is only available on Windows".into())
    };
    let opened = choose(renderer, cfg!(windows), d3d11, || {
        open::<SdlGlSurface>(&window, priority)
    })?;
    Ok(match opened {
        Chosen::D3d11((window, opened)) => then.run(window, opened),
        Chosen::OpenGl((window, opened)) => then.run(window, opened),
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

/// Which backend was opened.
#[derive(Debug, PartialEq, Eq)]
enum Chosen<D, G> {
    D3d11(D),
    OpenGl(G),
}

/// Opens the backend `renderer` asks for. `has_d3d11`: the platform has Direct3D 11,
/// which [`Renderer::Auto`] falls back to and [`Renderer::PreferD3d11`] tries first.
fn choose<D, G>(
    renderer: Renderer,
    has_d3d11: bool,
    d3d11: impl FnOnce() -> Result<D, String>,
    opengl: impl FnOnce() -> Result<G, String>,
) -> Result<Chosen<D, G>, String> {
    match renderer {
        Renderer::OpenGl => opengl().map(Chosen::OpenGl),
        Renderer::D3d11 => d3d11().map(Chosen::D3d11),
        Renderer::Auto | Renderer::PreferD3d11 if !has_d3d11 => opengl().map(Chosen::OpenGl),
        Renderer::Auto => match opengl() {
            Ok(opengl) => Ok(Chosen::OpenGl(opengl)),
            Err(e) => {
                tracing::warn!("{e}; falling back to Direct3D 11");
                d3d11().map(Chosen::D3d11)
            }
        },
        Renderer::PreferD3d11 => match d3d11() {
            Ok(d3d11) => Ok(Chosen::D3d11(d3d11)),
            Err(e) => {
                tracing::warn!("{e}; falling back to OpenGL 3.3");
                opengl().map(Chosen::OpenGl)
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

    fn failing() -> Result<u32, String> {
        Err("no context".into())
    }

    #[test]
    fn auto_opens_opengl() {
        for has_d3d11 in [true, false] {
            let (d, g) = (Cell::new(0), Cell::new(0));
            assert_eq!(
                choose(Renderer::Auto, has_d3d11, ok(&d), ok(&g)),
                Ok(Chosen::OpenGl(1))
            );
            assert_eq!(d.get(), 0);
        }
    }

    #[test]
    fn auto_falls_back_to_d3d11_when_opengl_fails() {
        let d = Cell::new(0);
        assert_eq!(
            choose(Renderer::Auto, true, ok(&d), failing),
            Ok(Chosen::D3d11(1))
        );
    }

    #[test]
    fn auto_reports_the_d3d11_failure_when_both_fail() {
        let d3d11 = || -> Result<u32, String> { Err("no swapchain".into()) };
        assert_eq!(
            choose(Renderer::Auto, true, d3d11, failing),
            Err::<Chosen<u32, u32>, _>("no swapchain".into())
        );
    }

    #[test]
    fn auto_without_d3d11_reports_the_opengl_failure() {
        let d = Cell::new(0);
        assert_eq!(
            choose(Renderer::Auto, false, ok(&d), failing),
            Err::<Chosen<u32, u32>, _>("no context".into())
        );
        assert_eq!(d.get(), 0);
    }

    #[test]
    fn prefer_d3d11_opens_d3d11() {
        let (d, g) = (Cell::new(0), Cell::new(0));
        assert_eq!(
            choose(Renderer::PreferD3d11, true, ok(&d), ok(&g)),
            Ok(Chosen::D3d11(1))
        );
        assert_eq!(g.get(), 0);
    }

    #[test]
    fn prefer_d3d11_falls_back_to_opengl_when_d3d11_fails() {
        let g = Cell::new(0);
        assert_eq!(
            choose(Renderer::PreferD3d11, true, failing, ok(&g)),
            Ok(Chosen::OpenGl(1))
        );
    }

    #[test]
    fn prefer_d3d11_uses_opengl_without_d3d11() {
        let (d, g) = (Cell::new(0), Cell::new(0));
        assert_eq!(
            choose(Renderer::PreferD3d11, false, ok(&d), ok(&g)),
            Ok(Chosen::OpenGl(1))
        );
        assert_eq!(d.get(), 0);
    }

    #[test]
    fn an_explicit_backend_does_not_fall_back() {
        let (d, g) = (Cell::new(0), Cell::new(0));
        assert_eq!(
            choose(Renderer::D3d11, true, failing, ok(&g)),
            Err::<Chosen<u32, u32>, _>("no context".into())
        );
        assert_eq!(
            choose(Renderer::OpenGl, true, ok(&d), failing),
            Err::<Chosen<u32, u32>, _>("no context".into())
        );
        assert_eq!((d.get(), g.get()), (0, 0));
    }

    #[test]
    fn renderer_names_parse() {
        assert_eq!(Renderer::parse("auto"), Some(Renderer::Auto));
        assert_eq!(Renderer::parse("opengl"), Some(Renderer::OpenGl));
        assert_eq!(Renderer::parse("d3d11"), Some(Renderer::D3d11));
        assert_eq!(Renderer::parse("vulkan"), None);
    }
}
