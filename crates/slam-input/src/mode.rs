//! Window modes, the wished window state and the window's scale (ADR-0070).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use sdl3_sys::events::{SDL_Event, SDL_PushEvent, SDL_UserEvent};
use sdl3_sys::keycode::{SDL_KMOD_ALT, SDL_KMOD_CTRL, SDL_KMOD_GUI, SDL_KMOD_SHIFT};
use sdl3_sys::scancode::{
    SDL_SCANCODE_F11, SDL_SCANCODE_LALT, SDL_SCANCODE_RALT, SDL_SCANCODE_RETURN,
};

/// Smallest windowed size in points, as `FrameworkSetting.WindowedSize` in osu-framework.
pub const MIN_WINDOWED_SIZE: [u16; 2] = [640, 480];
/// Windowed size in points at first start, as osu-framework.
pub const DEFAULT_WINDOWED_SIZE: [u16; 2] = [1366, 768];
/// Largest side a [`WindowState`] size can hold.
pub const MAX_SIDE: u16 = (1 << SIDE_BITS) - 1;

const SIDE_BITS: u32 = 14;
const DISPLAY_BITS: u32 = 6;
/// Largest display index a [`WindowState`] can hold.
pub const MAX_DISPLAY: u8 = (1 << DISPLAY_BITS) - 1;

/// How the window covers the screen, as osu-framework's `WindowMode`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WindowMode {
    /// A normal resizable window.
    Windowed,
    /// A borderless window covering the display, in the desktop video mode.
    Borderless,
    /// Exclusive fullscreen in a chosen video mode.
    #[default]
    Fullscreen,
}

impl WindowMode {
    /// The mode osu.Game starts in: borderless on macOS, where exclusive fullscreen takes
    /// control away from the OS, fullscreen elsewhere.
    pub const DEFAULT: Self = if cfg!(target_os = "macos") {
        Self::Borderless
    } else {
        Self::Fullscreen
    };

    /// The next mode for Alt+Enter and F11: windowed, borderless, fullscreen, windowed.
    /// Every mode is supported on desktop SDL3, so none is skipped.
    // Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3Window.Windowing.cs (CycleMode)
    pub fn next(self) -> Self {
        match self {
            Self::Windowed => Self::Borderless,
            Self::Borderless => Self::Fullscreen,
            Self::Fullscreen => Self::Windowed,
        }
    }

    fn bits(self) -> u64 {
        match self {
            Self::Windowed => 0,
            Self::Borderless => 1,
            Self::Fullscreen => 2,
        }
    }

    fn from_bits(bits: u64) -> Self {
        match bits & 3 {
            0 => Self::Windowed,
            1 => Self::Borderless,
            _ => Self::Fullscreen,
        }
    }
}

/// Everything the window applies when its mode changes. Sizes are clamped to
/// [`MAX_SIDE`] and the display index to [`MAX_DISPLAY`] when stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowState {
    pub mode: WindowMode,
    /// Index in SDL's display list; an index past its end means the primary display.
    pub display: u8,
    /// Video mode size for [`WindowMode::Fullscreen`] in pixels; `None` means the
    /// display's own size (osu-framework's 9999×9999).
    pub fullscreen_size: Option<[u16; 2]>,
    /// Window size for [`WindowMode::Windowed`] in points.
    pub windowed_size: [u16; 2],
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            mode: WindowMode::DEFAULT,
            display: 0,
            fullscreen_size: None,
            windowed_size: DEFAULT_WINDOWED_SIZE,
        }
    }
}

impl WindowState {
    /// One word, so a reader on another thread never sees half a change. Layout from
    /// the low bits: mode (2), display (6), fullscreen width and height (14 each, 0×0
    /// for the display's size), windowed width and height (14 each).
    pub(crate) fn pack(self) -> u64 {
        let side = |v: u16| u64::from(v.min(MAX_SIDE));
        let [fw, fh] = self.fullscreen_size.map_or([0, 0], |[w, h]| {
            // A zero side would read back as "display size": keep it a real size.
            [side(w).max(1), side(h).max(1)]
        });
        let [ww, wh] = self.windowed_size.map(side);
        self.mode.bits()
            | u64::from(self.display.min(MAX_DISPLAY)) << 2
            | fw << 8
            | fh << (8 + SIDE_BITS)
            | ww << (8 + 2 * SIDE_BITS)
            | wh << (8 + 3 * SIDE_BITS)
    }

    pub(crate) fn unpack(word: u64) -> Self {
        let side = |shift: u32| ((word >> shift) & u64::from(MAX_SIDE)) as u16;
        let (fw, fh) = (side(8), side(8 + SIDE_BITS));
        Self {
            mode: WindowMode::from_bits(word),
            display: ((word >> 2) & u64::from(MAX_DISPLAY)) as u8,
            fullscreen_size: (fw != 0 || fh != 0).then_some([fw, fh]),
            windowed_size: [side(8 + 2 * SIDE_BITS), side(8 + 3 * SIDE_BITS)],
        }
    }
}

/// Sets the window's mode, display and sizes from any thread, without locks on reads
/// (ADR-0070). The window applies a new state before waiting for its next event, and
/// [`WindowSettings::set`] wakes it, so the change does not wait for input. Alt+Enter and
/// F11 change the mode here too, and the window reports it as
/// [`crate::InputKind::WindowMode`]. The default handle belongs to no window (for
/// tests): it keeps the state and wakes nothing.
#[derive(Clone)]
pub struct WindowSettings {
    shared: Arc<Shared>,
}

struct Shared {
    /// [`WindowState::pack`].
    state: AtomicU64,
    /// Whether SDL is up, as [`crate::QuitHandle`]; `None` for a handle of no window.
    alive: Option<Arc<Mutex<bool>>>,
    /// The registered SDL event type that wakes the window; 0 when SDL had none left.
    wake_event: u32,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self::new(WindowState::default(), None, 0)
    }
}

impl WindowSettings {
    pub(crate) fn new(
        state: WindowState,
        alive: Option<Arc<Mutex<bool>>>,
        wake_event: u32,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: AtomicU64::new(state.pack()),
                alive,
                wake_event,
            }),
        }
    }

    /// Asks for `state`. When it differs from the last wish, wakes the window, which
    /// takes a lock and may fail silently if SDL's queue is full (the state is then
    /// applied after the next event): call it on changes, not every tick.
    pub fn set(&self, state: WindowState) {
        let word = state.pack();
        if self.shared.state.swap(word, Ordering::AcqRel) != word {
            self.wake();
        }
    }

    /// Replaces the wish with `new` only if it still is `current`, and wakes the window.
    /// Returns the stored wish: `new` (clamped), or on failure the wish found, which the
    /// window changed meanwhile (Alt+Enter, F11). The update thread keeps its settings in
    /// step with the window this way, without losing a switch to a race or to a full
    /// event ring. Locks as [`WindowSettings::set`].
    pub fn compare_and_set(
        &self,
        current: WindowState,
        new: WindowState,
    ) -> Result<WindowState, WindowState> {
        let (current, new) = (current.pack(), new.pack());
        match self
            .shared
            .state
            .compare_exchange(current, new, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                if new != current {
                    self.wake();
                }
                Ok(WindowState::unpack(new))
            }
            Err(found) => Err(WindowState::unpack(found)),
        }
    }

    /// The last wish, as stored (clamped).
    pub fn get(&self) -> WindowState {
        WindowState::unpack(self.word())
    }

    pub(crate) fn word(&self) -> u64 {
        self.shared.state.load(Ordering::Acquire)
    }

    /// Moves the wish to the next mode (Alt+Enter, F11) and returns the new state. A
    /// concurrent [`WindowSettings::set`] is not lost: the cycle applies on top of it.
    pub(crate) fn cycle_mode(&self) -> WindowState {
        let mut next = WindowState::default();
        let _ = self
            .shared
            .state
            .try_update(Ordering::AcqRel, Ordering::Acquire, |word| {
                let mut state = WindowState::unpack(word);
                state.mode = state.mode.next();
                next = state;
                Some(state.pack())
            });
        next
    }

    fn wake(&self) {
        let Some(alive) = &self.shared.alive else {
            return;
        };
        if self.shared.wake_event == 0 {
            return;
        }
        let alive = alive.lock().unwrap_or_else(PoisonError::into_inner);
        if !*alive {
            return;
        }
        let mut event = SDL_Event {
            user: SDL_UserEvent {
                r#type: self.shared.wake_event,
                ..Default::default()
            },
        };
        // SAFETY: SDL is initialised while `alive` is set, and the window's drop waits
        // for this lock before shutting SDL down. SDL_PushEvent is thread-safe. A full
        // queue only delays the change until the next event.
        unsafe { SDL_PushEvent(&mut event) };
    }
}

/// The window's pixels per point (drawable width over window width), published by the
/// window on every resize, as osu-framework's `Window.Scale`. Read it with the surface
/// size to turn input points into surface pixels. 1 until the window reports one; the
/// default handle stays 1 (for tests).
#[derive(Clone)]
pub struct WindowScale {
    /// `f32` bits.
    bits: Arc<AtomicU32>,
}

impl Default for WindowScale {
    fn default() -> Self {
        Self {
            bits: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        }
    }
}

impl WindowScale {
    /// A handle of no window that keeps `pixels_per_point` (for tests); 1 if it is not
    /// positive and finite.
    pub fn fixed(pixels_per_point: f32) -> Self {
        let scale = Self::default();
        if pixels_per_point.is_finite() && pixels_per_point > 0.0 {
            scale
                .bits
                .store(pixels_per_point.to_bits(), Ordering::Relaxed);
        }
        scale
    }

    /// Pixels per point, always positive and finite.
    pub fn pixels_per_point(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    /// Stores `pixels / points` widths; ignores zero sizes (a minimised window on
    /// Windows), as osu-framework's `fetchWindowSize`.
    pub(crate) fn store(&self, pixels: i32, points: i32) {
        if pixels <= 0 || points <= 0 {
            return;
        }
        let scale = pixels as f32 / points as f32;
        self.bits.store(scale.to_bits(), Ordering::Relaxed);
    }
}

/// Whether a key press may cycle the window mode, by the modifiers SDL put on the event:
/// Alt+Enter with Alt alone, or F11 with no modifier. Lock modifiers (Num, Caps, Scroll,
/// AltGr's Mode) do not count. The full check adds [`only_held`].
pub(crate) fn cycles_mode(scancode: u16, mods: u16) -> bool {
    const RETURN: u16 = SDL_SCANCODE_RETURN.0 as u16;
    const F11: u16 = SDL_SCANCODE_F11.0 as u16;
    const SHIFT: u16 = SDL_KMOD_SHIFT.0;
    const CTRL: u16 = SDL_KMOD_CTRL.0;
    const ALT: u16 = SDL_KMOD_ALT.0;
    const GUI: u16 = SDL_KMOD_GUI.0;
    let held = |bits: u16| mods & bits != 0;
    let others = |except: u16| {
        [SHIFT, CTRL, ALT, GUI]
            .iter()
            .any(|&b| b != except && held(b))
    };
    match scancode {
        RETURN => held(ALT) && !others(ALT),
        F11 => !others(0),
        _ => false,
    }
}

/// Whether no key but `key` (and Alt for Enter) is held, given SDL's keyboard state
/// indexed by scancode: osu-framework matches its `ToggleFullscreen` bindings with
/// `KeyCombinationMatchingMode.Exact`, so F11 does nothing while Z is held. Held mouse
/// buttons are checked by the caller.
// Ported from osu-framework 2026.921.1: osu.Framework/Input/FrameworkActionContainer.cs
pub(crate) fn only_held(key: u16, pressed: &[bool]) -> bool {
    const LALT: usize = SDL_SCANCODE_LALT.0 as usize;
    const RALT: usize = SDL_SCANCODE_RALT.0 as usize;
    let alt_allowed = key == SDL_SCANCODE_RETURN.0 as u16;
    pressed.iter().enumerate().all(|(scancode, &down)| {
        !down
            || scancode == usize::from(key)
            || (alt_allowed && (scancode == LALT || scancode == RALT))
    })
}

/// The video mode the default fullscreen size picks on Wayland: the one with most pixels
/// among modes at pixel density 1, if it has more than `bounds` (the display's bounds in
/// points). Compositors expect fullscreen sizes in pixels but windowed ones in logical
/// units. `modes` are `(w, h, pixel_density)`.
// Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3Window.Windowing.cs (getBestFullscreenDisplayMode)
pub(crate) fn wayland_native_mode(
    bounds: [i32; 2],
    modes: impl IntoIterator<Item = (i32, i32, f32)>,
) -> Option<usize> {
    let mut target = bounds;
    let mut best = None;
    for (i, (w, h, density)) in modes.into_iter().enumerate() {
        if density != 1.0 {
            continue;
        }
        if i64::from(w) * i64::from(h) > i64::from(target[0]) * i64::from(target[1]) {
            target = [w, h];
            best = Some(i);
        }
    }
    best
}

/// Top-left corner of a window of `size` centred around `position` (each in
/// `-0.5..=1.5`) of the free space on a display at `bounds` (`[x, y, w, h]`).
// Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3Window.Windowing.cs (moveWindowTo)
pub(crate) fn window_position(bounds: [i32; 4], size: [i32; 2], position: [f32; 2]) -> [i32; 2] {
    let [x, y, w, h] = bounds;
    let place = |free: i32, at: f32| (free as f32 * at).round_ties_even() as i32;
    [
        x + place(w - size[0], position[0]),
        y + place(h - size[1], position[1]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_goes_windowed_borderless_fullscreen() {
        assert_eq!(WindowMode::Windowed.next(), WindowMode::Borderless);
        assert_eq!(WindowMode::Borderless.next(), WindowMode::Fullscreen);
        assert_eq!(WindowMode::Fullscreen.next(), WindowMode::Windowed);
    }

    #[test]
    fn state_round_trips_through_one_word() {
        for mode in [
            WindowMode::Windowed,
            WindowMode::Borderless,
            WindowMode::Fullscreen,
        ] {
            for fullscreen_size in [None, Some([1920, 1080]), Some([MAX_SIDE, 1])] {
                let state = WindowState {
                    mode,
                    display: 5,
                    fullscreen_size,
                    windowed_size: [1366, 768],
                };
                assert_eq!(WindowState::unpack(state.pack()), state);
            }
        }
        assert_eq!(
            WindowState::unpack(WindowState::default().pack()),
            WindowState::default()
        );
    }

    #[test]
    fn packing_clamps_out_of_range_values() {
        let state = WindowState {
            mode: WindowMode::Windowed,
            display: 200,
            fullscreen_size: Some([20_000, 0]),
            windowed_size: [u16::MAX, 600],
        };
        let back = WindowState::unpack(state.pack());
        assert_eq!(back.display, MAX_DISPLAY);
        assert_eq!(back.fullscreen_size, Some([MAX_SIDE, 1]));
        assert_eq!(back.windowed_size, [MAX_SIDE, 600]);
    }

    #[test]
    fn default_state_is_lazer_first_start() {
        let state = WindowState::default();
        let mode = if cfg!(target_os = "macos") {
            WindowMode::Borderless
        } else {
            WindowMode::Fullscreen
        };
        assert_eq!(state.mode, mode);
        assert_eq!(state.fullscreen_size, None);
        assert_eq!(state.windowed_size, [1366, 768]);
    }

    #[test]
    fn handle_of_no_window_keeps_wishes_and_cycles() {
        let settings = WindowSettings::default();
        let other = settings.clone();
        let state = WindowState {
            mode: WindowMode::Windowed,
            display: 1,
            fullscreen_size: Some([1280, 720]),
            windowed_size: [800, 600],
        };
        settings.set(state);
        assert_eq!(other.get(), state);
        let next = other.cycle_mode();
        assert_eq!(next.mode, WindowMode::Borderless);
        assert_eq!(next.windowed_size, [800, 600]);
        assert_eq!(settings.get(), next);
    }

    #[test]
    fn scale_ignores_zero_sizes() {
        let scale = WindowScale::default();
        assert_eq!(scale.pixels_per_point(), 1.0);
        scale.store(2560, 1280);
        assert_eq!(scale.pixels_per_point(), 2.0);
        scale.store(0, 1280);
        scale.store(2560, 0);
        assert_eq!(scale.pixels_per_point(), 2.0);
    }

    #[test]
    fn mode_keys_match_exactly() {
        const RETURN: u16 = 40;
        const F11: u16 = 68;
        const LALT: u16 = 0x0100;
        const RALT: u16 = 0x0200;
        const LCTRL: u16 = 0x0040;
        const LSHIFT: u16 = 0x0001;
        const NUM: u16 = 0x1000;
        const CAPS: u16 = 0x2000;
        assert!(cycles_mode(RETURN, LALT));
        assert!(cycles_mode(RETURN, RALT | NUM | CAPS));
        assert!(!cycles_mode(RETURN, 0));
        assert!(!cycles_mode(RETURN, LALT | LCTRL));
        assert!(!cycles_mode(RETURN, LALT | LSHIFT));
        assert!(cycles_mode(F11, 0));
        assert!(cycles_mode(F11, NUM));
        assert!(!cycles_mode(F11, LCTRL));
        assert!(!cycles_mode(F11, LALT));
        assert!(!cycles_mode(F11, 0x0002)); // right Shift
        assert!(!cycles_mode(F11, 0x0080)); // right Ctrl
        assert!(!cycles_mode(F11, 0x0400)); // left GUI
        assert!(!cycles_mode(RETURN, LALT | 0x0800)); // right GUI
        // Keypad Enter is a different key in osu-framework.
        assert!(!cycles_mode(88, LALT));
    }

    #[test]
    fn mode_keys_need_every_other_key_up() {
        const RETURN: usize = 40;
        const F11: usize = 68;
        const LALT: usize = 226;
        const RALT: usize = 230;
        const Z: usize = 29;
        let held = |keys: &[usize]| {
            let mut pressed = [false; 512];
            for &k in keys {
                pressed[k] = true;
            }
            pressed
        };
        assert!(only_held(RETURN as u16, &held(&[LALT, RETURN])));
        assert!(only_held(RETURN as u16, &held(&[RALT, RETURN])));
        assert!(!only_held(RETURN as u16, &held(&[LALT, RETURN, Z])));
        assert!(only_held(F11 as u16, &held(&[F11])));
        // A stream key held during a play: no switch, as in lazer.
        assert!(!only_held(F11 as u16, &held(&[F11, Z])));
        assert!(!only_held(F11 as u16, &held(&[F11, LALT])));
        // SDL may not have the key down yet when the event is read early.
        assert!(only_held(F11 as u16, &held(&[])));
    }

    #[test]
    fn compare_and_set_loses_to_a_switch_of_the_window() {
        let settings = WindowSettings::default();
        let start = settings.get();
        let windowed = WindowState {
            mode: WindowMode::Windowed,
            ..start
        };
        assert_eq!(settings.compare_and_set(start, windowed), Ok(windowed));
        let switched = settings.cycle_mode();
        let borderless_wish = WindowState {
            windowed_size: [800, 600],
            ..windowed
        };
        assert_eq!(
            settings.compare_and_set(windowed, borderless_wish),
            Err(switched)
        );
        assert_eq!(settings.get(), switched);
    }

    #[test]
    fn fixed_scale_keeps_only_valid_values() {
        assert_eq!(WindowScale::fixed(1.5).pixels_per_point(), 1.5);
        assert_eq!(WindowScale::fixed(0.0).pixels_per_point(), 1.0);
        assert_eq!(WindowScale::fixed(f32::NAN).pixels_per_point(), 1.0);
    }

    #[test]
    fn wayland_picks_largest_unscaled_mode_above_bounds() {
        // A 2× display: bounds in points are 1280×720, its pixel mode is 2560×1440.
        let modes = [
            (1280, 720, 2.0),
            (2560, 1440, 1.0),
            (1920, 1080, 1.0),
            (3840, 2160, 0.5),
        ];
        assert_eq!(wayland_native_mode([1280, 720], modes), Some(1));
        // Nothing larger than the bounds: keep them.
        assert_eq!(wayland_native_mode([2560, 1440], modes), None);
        assert_eq!(wayland_native_mode([1280, 720], []), None);
    }

    #[test]
    fn window_is_placed_in_free_space() {
        let bounds = [1920, 0, 2560, 1440];
        assert_eq!(
            window_position(bounds, [1366, 768], [0.5, 0.5]),
            [1920 + 597, 336]
        );
        assert_eq!(
            window_position(bounds, [1366, 768], [0.0, 1.0]),
            [1920, 672]
        );
        // Round half to even, as .NET's Math.Round.
        assert_eq!(window_position([0, 0, 101, 0], [0, 0], [0.5, 0.0]), [50, 0]);
    }
}
