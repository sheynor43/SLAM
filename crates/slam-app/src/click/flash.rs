//! Draw side of the click test: a plain window that flashes on every press, to see
//! that input arrives. The measurement does not depend on it.

use slam_engine::clock::now_ns;
use slam_engine::{Draw, FrameInfo};
use slam_input::QuitHandle;
use slam_render::{Color, Device};

use crate::backend::Backend;
use crate::bench::QuitOnDrop;

const FLASH_NS: u64 = 60_000_000;

pub struct Flash<B: Backend> {
    surface: Option<B>,
    device: Option<B::Device>,
    seen: u64,
    flash_until_ns: u64,
    _quit: QuitOnDrop,
}

impl<B: Backend> Flash<B> {
    pub fn new(surface: B, quit: QuitHandle) -> Self {
        Self {
            surface: Some(surface),
            device: None,
            seen: 0,
            flash_until_ns: 0,
            _quit: QuitOnDrop(quit),
        }
    }
}

impl<B: Backend> Draw<u64> for Flash<B> {
    fn start(&mut self) {
        let surface = self.surface.take().expect("surface is set before start");
        // The window thread made a device on this surface already.
        self.device = Some(surface.create_device().expect("render device"));
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
        self.surface = B::release(device).expect("release the render device");
    }
}
