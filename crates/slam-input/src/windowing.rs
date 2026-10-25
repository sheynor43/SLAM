//! Applying a [`WindowState`] to the SDL window, on the main thread (ADR-0070).

use std::mem::MaybeUninit;
use std::ptr;

use sdl3_sys::rect::SDL_Rect;
use sdl3_sys::stdinc::SDL_free;
use sdl3_sys::video::{
    SDL_DisplayID, SDL_DisplayMode, SDL_GetClosestFullscreenDisplayMode, SDL_GetDesktopDisplayMode,
    SDL_GetDisplayBounds, SDL_GetDisplayForWindow, SDL_GetDisplays, SDL_GetFullscreenDisplayModes,
    SDL_GetPrimaryDisplay, SDL_GetWindowFullscreenMode, SDL_RestoreWindow, SDL_SetWindowBordered,
    SDL_SetWindowFullscreen, SDL_SetWindowFullscreenMode, SDL_SetWindowPosition,
    SDL_SetWindowResizable, SDL_SetWindowSize, SDL_Window,
};

use crate::mode::{WindowMode, WindowState, wayland_native_mode, window_position};
use crate::window::sdl_error;

/// Puts `window` into `state`. Failed SDL calls are logged and skipped, as
/// osu-framework's `LogErrorIfFailed`. Allocates (display lists): main thread, on mode
/// changes only.
///
/// # Safety
/// `window` is valid, on the thread that initialised SDL.
// Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3DesktopWindow.cs (UpdateWindowStateAndSize)
// Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3Window.Windowing.cs (UpdateWindowStateAndSize)
pub(crate) unsafe fn apply(window: *mut SDL_Window, state: WindowState, wayland: bool) {
    let display = display_at(state.display);
    let windowed = state.windowed_size.map(i32::from);
    // SAFETY: guaranteed by the caller for every call below.
    unsafe {
        // Needed even between two fullscreen resolutions, or the GL context keeps its
        // old size (an SDL issue osu-framework works around the same way).
        check(SDL_SetWindowBordered(window, true), "SDL_SetWindowBordered");
        check(
            SDL_SetWindowFullscreen(window, false),
            "SDL_SetWindowFullscreen",
        );
        check(SDL_RestoreWindow(window), "SDL_RestoreWindow");
        match state.mode {
            WindowMode::Windowed => {
                check(
                    SDL_SetWindowSize(window, windowed[0], windowed[1]),
                    "SDL_SetWindowSize",
                );
                check(
                    SDL_SetWindowResizable(window, true),
                    "SDL_SetWindowResizable",
                );
                // Saved positions arrive with #245; until then the window is centred.
                move_to(window, display, windowed, [0.5, 0.5]);
            }
            WindowMode::Fullscreen => {
                let Some(mode) = best_fullscreen_mode(window, display, state, wayland) else {
                    // osu-framework throws here; a known size beats whatever the reset
                    // left.
                    tracing::error!("no fullscreen display mode found; staying windowed");
                    check(
                        SDL_SetWindowSize(window, windowed[0], windowed[1]),
                        "SDL_SetWindowSize",
                    );
                    return;
                };
                ensure_on_display(window, display, windowed);
                check(
                    SDL_SetWindowFullscreenMode(window, &mode),
                    "SDL_SetWindowFullscreenMode",
                );
                check(
                    SDL_SetWindowFullscreen(window, true),
                    "SDL_SetWindowFullscreen",
                );
            }
            WindowMode::Borderless => {
                // The Windows variant (a borderless window 1 px wider) comes with #245.
                ensure_on_display(window, display, windowed);
                check(
                    SDL_SetWindowFullscreenMode(window, ptr::null()),
                    "SDL_SetWindowFullscreenMode",
                );
                check(
                    SDL_SetWindowFullscreen(window, true),
                    "SDL_SetWindowFullscreen",
                );
            }
        }
    }
}

fn check(done: bool, call: &str) {
    if !done {
        tracing::warn!("{call} failed: {}", sdl_error());
    }
}

/// The display at `index` in SDL's list, or the primary one when there is no such index,
/// as osu-framework's `Displays.ElementAtOrDefault(index) ?? PrimaryDisplay`.
fn display_at(index: u8) -> SDL_DisplayID {
    let mut count = 0;
    // SAFETY: SDL video is initialised (the caller holds a window); `count` is a valid
    // out pointer. The array is freed below.
    let list = unsafe { SDL_GetDisplays(&mut count) };
    let mut id = None;
    if !list.is_null() {
        if i32::from(index) < count {
            // SAFETY: `index` is below the array's length `count`.
            id = Some(unsafe { *list.add(usize::from(index)) });
        }
        // SAFETY: SDL allocated the array and leaves it to us.
        unsafe { SDL_free(list.cast()) };
    }
    // SAFETY: SDL video is initialised.
    id.unwrap_or_else(|| unsafe { SDL_GetPrimaryDisplay() })
}

fn display_bounds(display: SDL_DisplayID) -> Option<[i32; 4]> {
    let mut rect = SDL_Rect::default();
    // SAFETY: SDL video is initialised; `rect` is a valid out pointer.
    if unsafe { SDL_GetDisplayBounds(display, &mut rect) } {
        Some([rect.x, rect.y, rect.w, rect.h])
    } else {
        tracing::warn!("SDL_GetDisplayBounds failed: {}", sdl_error());
        None
    }
}

/// Centres the window of the windowed size on `display` unless it is already there.
///
/// # Safety
/// As [`apply`].
// Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3Window.Windowing.cs (ensureWindowOnDisplay)
unsafe fn ensure_on_display(window: *mut SDL_Window, display: SDL_DisplayID, size: [i32; 2]) {
    // SAFETY: guaranteed by the caller.
    if unsafe { SDL_GetDisplayForWindow(window) } == display {
        return;
    }
    // SAFETY: guaranteed by the caller.
    unsafe { move_to(window, display, size, [0.5, 0.5]) };
}

/// # Safety
/// As [`apply`].
// Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3Window.Windowing.cs (moveWindowTo)
unsafe fn move_to(
    window: *mut SDL_Window,
    display: SDL_DisplayID,
    size: [i32; 2],
    position: [f32; 2],
) {
    let Some(bounds) = display_bounds(display) else {
        return;
    };
    let [x, y] = window_position(bounds, size, position);
    // SAFETY: guaranteed by the caller.
    check(
        unsafe { SDL_SetWindowPosition(window, x, y) },
        "SDL_SetWindowPosition",
    );
}

/// The video mode for [`WindowMode::Fullscreen`]: the closest to the wished size (the
/// display's size by default, in pixels on Wayland), then to the display's bounds, then
/// the desktop mode, then the window's current fullscreen mode.
///
/// The refresh rate asked for is the display's desktop rate. osu-framework asks for the
/// rate of the window's current mode, which is the same unless the window already was in
/// exclusive fullscreen at another rate.
///
/// # Safety
/// As [`apply`].
// Ported from osu-framework 2026.921.1: osu.Framework/Platform/SDL3/SDL3Window.Windowing.cs (getBestFullscreenDisplayMode)
unsafe fn best_fullscreen_mode(
    window: *mut SDL_Window,
    display: SDL_DisplayID,
    state: WindowState,
    wayland: bool,
) -> Option<SDL_DisplayMode> {
    let bounds = display_bounds(display).unwrap_or_default();
    let bounds_size = [bounds[2], bounds[3]];
    // SAFETY: SDL video is initialised; the result is SDL's, valid while the display is.
    let desktop = unsafe { SDL_GetDesktopDisplayMode(display) };
    // SAFETY: checked for null; read at once.
    let mut refresh = unsafe { desktop.as_ref() }.map_or(0.0, |mode| mode.refresh_rate);
    let target = match state.fullscreen_size {
        Some([w, h]) => [i32::from(w), i32::from(h)],
        None if wayland => {
            // SAFETY: as [`unscaled_native_mode`].
            match unsafe { unscaled_native_mode(display, bounds_size) } {
                Some(mode) => {
                    refresh = mode.refresh_rate;
                    [mode.w, mode.h]
                }
                None => bounds_size,
            }
        }
        None => bounds_size,
    };
    // sdl3-sys modes cannot be built or copied in Rust: SDL fills them, `ptr::read`
    // copies them (plain C data; `internal` points into SDL's display, not owned).
    let mut mode = MaybeUninit::<SDL_DisplayMode>::zeroed();
    // Without the display's bounds (an SDL failure, logged) a 0×0 target would pick the
    // smallest mode: go to the desktop mode instead.
    let known = |[w, h]: [i32; 2]| w > 0 && h > 0;
    // SAFETY: SDL video is initialised; `mode` is a valid out pointer, initialised by a
    // successful call.
    if known(target)
        && unsafe {
            SDL_GetClosestFullscreenDisplayMode(
                display,
                target[0],
                target[1],
                refresh,
                true,
                mode.as_mut_ptr(),
            )
        }
    {
        // SAFETY: initialised by the successful call.
        return Some(unsafe { mode.assume_init() });
    }
    tracing::warn!(
        "no display mode near {}x{}@{refresh}: {}",
        target[0],
        target[1],
        sdl_error()
    );
    // SAFETY: as above.
    if known(bounds_size)
        && unsafe {
            SDL_GetClosestFullscreenDisplayMode(
                display,
                bounds_size[0],
                bounds_size[1],
                0.0,
                true,
                mode.as_mut_ptr(),
            )
        }
    {
        // SAFETY: initialised by the successful call.
        return Some(unsafe { mode.assume_init() });
    }
    tracing::warn!(
        "no display mode near the display's {}x{}: {}",
        bounds_size[0],
        bounds_size[1],
        sdl_error()
    );
    if !desktop.is_null() {
        // SAFETY: a valid mode, copied at once.
        return Some(unsafe { ptr::read(desktop) });
    }
    // SAFETY: guaranteed by the caller.
    let current = unsafe { SDL_GetWindowFullscreenMode(window) };
    // SAFETY: checked for null; a valid mode, copied at once.
    (!current.is_null()).then(|| unsafe { ptr::read(current) })
}

/// On Wayland, the display's largest mode at pixel density 1 when it is larger than the
/// display's bounds in points.
///
/// # Safety
/// SDL video is initialised on this thread.
unsafe fn unscaled_native_mode(
    display: SDL_DisplayID,
    bounds: [i32; 2],
) -> Option<SDL_DisplayMode> {
    let mut count = 0;
    // SAFETY: guaranteed by the caller; `count` is a valid out pointer. The array is
    // freed below.
    let list = unsafe { SDL_GetFullscreenDisplayModes(display, &mut count) };
    if list.is_null() {
        return None;
    }
    // SAFETY: SDL returns `count` valid mode pointers.
    let modes = unsafe { std::slice::from_raw_parts(list, usize::try_from(count).unwrap_or(0)) };
    let pick = wayland_native_mode(
        bounds,
        modes.iter().map(|&mode| {
            // SAFETY: each pointer in the list is a valid mode.
            let mode = unsafe { &*mode };
            (mode.w, mode.h, mode.pixel_density)
        }),
    )
    // SAFETY: `i` indexes `modes`, each a valid mode; copied before the list is freed.
    .map(|i| unsafe { ptr::read(modes[i]) });
    // SAFETY: SDL allocated the list (modes included) as one block and leaves it to us.
    unsafe { SDL_free(list.cast()) };
    pick
}
