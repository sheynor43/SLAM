//! Blocking the Windows key while a play runs (ADR-0094), as lazer's
//! `GameplayWinKeyBlocker`: on Windows a low-level keyboard hook on the window's thread eats
//! the left and right Windows keys, and nothing else, while the window has focus and the
//! update thread asks for it. On Linux, which cannot block one key, SDL's keyboard grab
//! takes the desktop's shortcuts (Super among them) instead. Elsewhere the wish is kept and
//! nothing happens.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Asks for the Windows key to be blocked, from any thread, without locks. Like
/// [`crate::CursorVisibility`], the window applies the wish before waiting for its next
/// event. The default handle belongs to no window (for tests).
#[derive(Clone, Default)]
pub struct WinKeyBlock {
    wanted: Arc<AtomicBool>,
}

impl WinKeyBlock {
    pub(crate) fn new(wanted: Arc<AtomicBool>) -> WinKeyBlock {
        WinKeyBlock { wanted }
    }

    /// Asks for the Windows key to be blocked (a play runs and the setting is on) or let
    /// through.
    pub fn set_blocked(&self, blocked: bool) {
        self.wanted.store(blocked, Ordering::Relaxed);
    }

    /// The last wish.
    pub fn is_blocked(&self) -> bool {
        self.wanted.load(Ordering::Relaxed)
    }
}

/// Whether the hook should be in place: asked for and the window focused, as lazer's
/// `isActive && disableWinKey && Playing`.
pub(crate) fn should_hook(wanted: bool, focused: bool) -> bool {
    wanted && focused
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Desktop/Windows/WindowsKey.cs
const WM_KEYDOWN: usize = 0x0100;
const WM_SYSKEYUP: usize = 0x0105;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;

/// Whether the hook eats a keyboard message: a key down or up (system or not) of either
/// Windows key. Only the Windows hook calls it; the tests check it everywhere.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn eats(message: usize, virtual_key: u32) -> bool {
    (WM_KEYDOWN..=WM_SYSKEYUP).contains(&message) && matches!(virtual_key, VK_LWIN | VK_RWIN)
}

/// The hook (Windows) or keyboard grab (Linux), set and removed on the window's thread,
/// whose message loop runs the hook.
#[derive(Default)]
pub(crate) struct WinKeyHook {
    #[cfg(windows)]
    hook: Option<windows_sys::Win32::UI::WindowsAndMessaging::HHOOK>,
    #[cfg(not(windows))]
    hooked: bool,
    /// What [`WinKeyHook::set`] last tried: a failure is not tried again, and logged
    /// again, until the wish or the focus changes.
    tried: Option<bool>,
}

impl WinKeyHook {
    /// Installs or removes the hook (grabs or releases `window`'s keyboard on Linux) to
    /// match `on`. A failed install is logged once and tried again only when `on` changes
    /// (the loop calls this before every event). `window` is the valid window of this
    /// thread, or null in tests (nothing is grabbed then).
    pub(crate) fn set(&mut self, on: bool, window: *mut sdl3_sys::video::SDL_Window) {
        if self.tried == Some(on) {
            return;
        }
        self.tried = Some(on);
        if on == self.is_set() {
            return;
        }
        if on {
            self.install(window);
        } else {
            self.remove(window);
        }
    }

    #[cfg(windows)]
    fn is_set(&self) -> bool {
        self.hook.is_some()
    }

    #[cfg(not(windows))]
    fn is_set(&self) -> bool {
        self.hooked
    }

    #[cfg(windows)]
    fn install(&mut self, _window: *mut sdl3_sys::video::SDL_Window) {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_KEYBOARD_LL};
        // SAFETY: a null name asks for the executable's own module handle; `keyboard_proc`
        // is a valid hook procedure for the whole program's life; thread 0 makes it a
        // global low-level hook, run by this thread's message loop.
        let hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_proc),
                GetModuleHandleW(std::ptr::null()),
                0,
            )
        };
        if hook.is_null() {
            tracing::warn!(
                error = %std::io::Error::last_os_error(),
                "the Windows key cannot be blocked"
            );
        } else {
            self.hook = Some(hook);
        }
    }

    #[cfg(windows)]
    fn remove(&mut self, _window: *mut sdl3_sys::video::SDL_Window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx;
        if let Some(hook) = self.hook.take() {
            // SAFETY: `hook` came from `SetWindowsHookExW` on this thread and is removed
            // once.
            if unsafe { UnhookWindowsHookEx(hook) } == 0 {
                tracing::warn!(
                    error = %std::io::Error::last_os_error(),
                    "the Windows key hook cannot be removed"
                );
            }
        }
    }

    #[cfg(not(windows))]
    fn install(&mut self, window: *mut sdl3_sys::video::SDL_Window) {
        if grab(window, true) {
            self.hooked = true;
        }
    }

    #[cfg(not(windows))]
    fn remove(&mut self, window: *mut sdl3_sys::video::SDL_Window) {
        // Released or not, it is not retried: the window keeps no grab past its own life.
        grab(window, false);
        self.hooked = false;
    }
}

/// Grabs or releases the keyboard of `window` on Linux: the compositor's shortcuts are
/// inhibited on Wayland, the keyboard grabbed on X11 (ADR-0094). Elsewhere and with a null
/// window does nothing and succeeds. Returns whether it succeeded.
#[cfg(not(windows))]
fn grab(window: *mut sdl3_sys::video::SDL_Window, on: bool) -> bool {
    if !cfg!(target_os = "linux") || window.is_null() {
        return true;
    }
    // SAFETY: `window` is the valid window of this thread, which SDL video runs on.
    let done = unsafe { sdl3_sys::video::SDL_SetWindowKeyboardGrab(window, on) };
    if !done {
        tracing::warn!(error = %crate::window::sdl_error(), on, "the keyboard grab failed");
    }
    done
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Desktop/Windows/WindowsKey.cs (lowLevelKeyboardProc)
#[cfg(windows)]
unsafe extern "system" fn keyboard_proc(
    code: i32,
    message: windows_sys::Win32::Foundation::WPARAM,
    data: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT};
    // Only HC_ACTION carries a key; other codes go on untouched, as the API requires.
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `data` points to the message's KBDLLHOOKSTRUCT.
        let key = unsafe { &*(data as *const KBDLLHOOKSTRUCT) };
        if eats(message, key.vkCode) {
            return 1;
        }
    }
    // SAFETY: passes the message on with the arguments it came with; the hook handle is
    // ignored by the API.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, message, data) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_windows_keys_are_eaten() {
        for message in [0x0100, 0x0101, 0x0104, 0x0105] {
            assert!(eats(message, VK_LWIN), "{message:#x}");
            assert!(eats(message, VK_RWIN), "{message:#x}");
            // Z, Escape, Alt, Tab pass.
            for key in [0x5A, 0x1B, 0x12, 0x09] {
                assert!(!eats(message, key));
            }
        }
        // Other messages pass even for the Windows keys.
        assert!(!eats(0x00FF, VK_LWIN));
        assert!(!eats(0x0106, VK_RWIN));
    }

    #[test]
    fn the_hook_needs_the_wish_and_the_focus() {
        assert!(should_hook(true, true));
        assert!(!should_hook(true, false));
        assert!(!should_hook(false, true));
        assert!(!should_hook(false, false));
    }

    #[test]
    #[cfg(not(windows))]
    fn the_hook_follows_set() {
        let null = std::ptr::null_mut();
        let mut hook = WinKeyHook::default();
        hook.set(true, null);
        assert!(hook.is_set());
        hook.set(true, null);
        hook.set(false, null);
        assert!(!hook.is_set());
    }

    #[test]
    fn the_handle_shares_its_wish() {
        let wanted = Arc::new(AtomicBool::new(false));
        let a = WinKeyBlock::new(Arc::clone(&wanted));
        let b = a.clone();
        b.set_blocked(true);
        assert!(a.is_blocked());
        assert!(wanted.load(Ordering::Relaxed));
        assert!(!WinKeyBlock::default().is_blocked());
    }
}
