//! Draw side of the click test: a plain window that flashes on every press, to see
//! that input arrives. The measurement does not depend on it.

use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::{Draw, FrameInfo};
use slam_input::QuitHandle;
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::{Color, Device};

use crate::bench::QuitOnDrop;

const FLASH_NS: u64 = 60_000_000;

pub struct Flash {
    surface: Option<SdlGlSurface>,
    device: Option<GlDevice<SdlGlSurface>>,
    seen: u64,
    flash_until_ns: u64,
    _quit: QuitOnDrop,
}

impl Flash {
    pub fn new(surface: SdlGlSurface, quit: QuitHandle) -> Self {
        Self {
            surface: Some(surface),
            device: None,
            seen: 0,
            flash_until_ns: 0,
            _quit: QuitOnDrop(quit),
        }
    }
}

impl Draw<u64> for Flash {
    fn start(&mut self) {
        let surface = self.surface.take().expect("surface is set before start");
        self.device = Some(GlDevice::new(surface, GlDebug::for_build()).expect("GL device"));
    }

    fn frame(&mut self, _: &FrameInfo, presses: &u64) {
        let now = now_ns();
        if *presses != self.seen {
            self.seen = *presses;
            self.flash_until_ns = now + FLASH_NS;
        }
        let colour = if now < self.flash_until_ns {
            Color::rgb(0.9, 0.9, 0.95)
        } else {
            Color::rgb(0.05, 0.05, 0.08)
        };
        let device = self.device.as_mut().expect("started");
        device.begin_frame(colour);
        // A lost surface shows as a frozen window; the click test keeps measuring.
        let _ = device.present();
    }

    fn stop(&mut self) {
        let device = self.device.take().expect("started");
        self.surface = Some(device.into_surface().expect("release GL context"));
    }
}
