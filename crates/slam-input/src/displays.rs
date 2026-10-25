//! What the settings screen lists of the displays: names, bounds and video mode sizes,
//! collected on the main thread at start and on display events (ADR-0072).

use std::ffi::CStr;

use sdl3_sys::rect::SDL_Rect;
use sdl3_sys::stdinc::SDL_free;
use sdl3_sys::video::{
    SDL_DisplayID, SDL_GetDisplayBounds, SDL_GetDisplayName, SDL_GetDisplayUsableBounds,
    SDL_GetDisplays, SDL_GetFullscreenDisplayModes, SDL_GetPrimaryDisplay,
};

use crate::mode::MAX_DISPLAY;

/// One display, as SDL lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Display {
    pub name: String,
    /// x, y, width, height in points.
    pub bounds: [i32; 4],
    /// The bounds without the OS's panels and docks; the bounds when SDL has none.
    pub usable: [i32; 4],
    /// Sizes of the fullscreen video modes, in SDL's order (largest first), repeats kept.
    pub modes: Vec<[u16; 2]>,
}

/// The displays in SDL's order: index `i` is the `display` setting `i`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Displays {
    pub list: Vec<Display>,
    /// The index of the primary display in `list`; 0 when SDL names none of them.
    pub primary: usize,
    /// The window's windowed size when collected, for the settings screen to keep it
    /// listed even if no mode has it.
    pub windowed_size: [u16; 2],
}

/// Gets every new [`Displays`]. Runs on the main thread between events: it must not
/// block, or input waits.
pub type DisplaysCallback = Box<dyn FnMut(Displays)>;

/// Lists the displays, at most [`MAX_DISPLAY`] + 1 (the `display` setting cannot point
/// further). Allocates: main thread, on display events only.
///
/// # Safety
/// SDL video is initialised on this thread.
pub(crate) unsafe fn collect(windowed_size: [u16; 2]) -> Displays {
    let mut count = 0;
    // SAFETY: guaranteed by the caller; `count` is a valid out pointer. Freed below.
    let ids = unsafe { SDL_GetDisplays(&mut count) };
    if ids.is_null() {
        tracing::warn!("SDL_GetDisplays failed: {}", crate::window::sdl_error());
        return Displays {
            windowed_size,
            ..Displays::default()
        };
    }
    let count = usize::try_from(count)
        .unwrap_or(0)
        .min(usize::from(MAX_DISPLAY) + 1);
    // SAFETY: SDL returns `count` ids (or more, cut above), valid until freed.
    let ids_slice = unsafe { std::slice::from_raw_parts(ids, count) };
    // SAFETY: guaranteed by the caller.
    let primary_id = unsafe { SDL_GetPrimaryDisplay() };
    let primary = ids_slice
        .iter()
        .position(|id| *id == primary_id)
        .unwrap_or(0);
    // SAFETY: guaranteed by the caller, for each id SDL just listed.
    let list = ids_slice.iter().map(|id| unsafe { display(*id) }).collect();
    // SAFETY: SDL allocated the array and leaves it to us; `ids_slice` is not used after.
    unsafe { SDL_free(ids.cast()) };
    Displays {
        list,
        primary,
        windowed_size,
    }
}

/// # Safety
/// As [`collect`]; `id` is a display SDL listed.
unsafe fn display(id: SDL_DisplayID) -> Display {
    let rect = |get: unsafe extern "C" fn(SDL_DisplayID, *mut SDL_Rect) -> bool| {
        let mut rect = SDL_Rect::default();
        // SAFETY: guaranteed by the caller; `rect` is a valid out pointer.
        unsafe { get(id, &mut rect) }.then_some([rect.x, rect.y, rect.w, rect.h])
    };
    let bounds = rect(SDL_GetDisplayBounds).unwrap_or_default();
    let usable = rect(SDL_GetDisplayUsableBounds).unwrap_or(bounds);
    // SAFETY: guaranteed by the caller. The name is SDL's, valid while the display is;
    // copied at once.
    let name = unsafe { SDL_GetDisplayName(id) };
    let name = if name.is_null() {
        String::new()
    } else {
        // SAFETY: a non-null name is a NUL-terminated string.
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    Display {
        name,
        bounds,
        usable,
        // SAFETY: guaranteed by the caller.
        modes: unsafe { mode_sizes(id) },
    }
}

/// # Safety
/// As [`display`].
unsafe fn mode_sizes(id: SDL_DisplayID) -> Vec<[u16; 2]> {
    let mut count = 0;
    // SAFETY: guaranteed by the caller; `count` is a valid out pointer. Freed below.
    let list = unsafe { SDL_GetFullscreenDisplayModes(id, &mut count) };
    if list.is_null() {
        return Vec::new();
    }
    // SAFETY: SDL returns `count` valid mode pointers.
    let modes = unsafe { std::slice::from_raw_parts(list, usize::try_from(count).unwrap_or(0)) };
    let side = |v: i32| v.clamp(0, i32::from(u16::MAX)) as u16;
    let sizes = modes
        .iter()
        .map(|&mode| {
            // SAFETY: each pointer in the list is a valid mode, read before the free.
            let mode = unsafe { &*mode };
            [side(mode.w), side(mode.h)]
        })
        .collect();
    // SAFETY: SDL allocated the list (modes included) as one block and leaves it to us.
    unsafe { SDL_free(list.cast()) };
    sizes
}
