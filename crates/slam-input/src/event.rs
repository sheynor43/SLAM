/// One input event, stamped in the engine clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputEvent {
    /// Time the OS (or SDL, where the OS gives no timestamp) observed the event, in
    /// nanoseconds of the engine's monotonic clock. Events from different devices may
    /// arrive slightly out of timestamp order.
    pub time_ns: u64,
    pub kind: InputKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputKind {
    /// Physical key (USB HID usage id, as `SDL_Scancode`). OS key repeats are dropped.
    Key {
        scancode: u16,
        down: bool,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// Cursor position in window points (not pixels: the window is high-DPI aware,
    /// scale by `SDL_GetWindowPixelDensity` for pixels). Includes motion SDL
    /// synthesises from touch and pen, so tablets in absolute mode work as mice.
    MouseMove {
        x: f32,
        y: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    X1,
    X2,
}
