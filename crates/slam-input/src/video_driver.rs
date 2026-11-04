//! The SDL video driver the window starts with on Linux (ADR-0097).
//!
//! SDL picks the driver itself and under some Wayland compositors picks X11 through
//! Xwayland, where the shortcut block (ADR-0094) does not reach the compositor. The
//! player's choice is stored with [`set_video_driver`] before the window is made and
//! read once at SDL initialisation. `SDL_VIDEO_DRIVER` in the environment wins over it.

use std::ffi::CStr;
use std::sync::atomic::{AtomicU8, Ordering};

/// Environment variable SDL reads for its driver; set, it wins over the setting.
pub const VIDEO_DRIVER_ENV: &str = "SDL_VIDEO_DRIVER";

/// SDL2's name of [`VIDEO_DRIVER_ENV`], which SDL3 still reads when the new one is unset.
const LEGACY_VIDEO_DRIVER_ENV: &str = "SDL_VIDEODRIVER";

/// Whether the environment sets SDL's driver. SDL refuses a hint while either variable
/// is present, even empty, so presence is what counts.
pub(crate) fn env_sets_driver() -> bool {
    [VIDEO_DRIVER_ENV, LEGACY_VIDEO_DRIVER_ENV]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

/// Which SDL video driver to ask for. Only Linux has a choice; elsewhere every value
/// behaves as [`VideoDriver::Auto`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoDriver {
    /// SDL's own choice.
    #[default]
    Auto,
    Wayland,
    X11,
}

impl VideoDriver {
    pub const ALL: [VideoDriver; 3] = [VideoDriver::Auto, VideoDriver::Wayland, VideoDriver::X11];

    /// Name in `slam.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            VideoDriver::Auto => "auto",
            VideoDriver::Wayland => "wayland",
            VideoDriver::X11 => "x11",
        }
    }

    /// Inverse of [`VideoDriver::as_str`].
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }

    /// SDL's driver name, `None` for SDL's own choice.
    fn sdl_name(self) -> Option<&'static CStr> {
        match self {
            VideoDriver::Auto => None,
            VideoDriver::Wayland => Some(c"wayland"),
            VideoDriver::X11 => Some(c"x11"),
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => VideoDriver::Wayland,
            2 => VideoDriver::X11,
            _ => VideoDriver::Auto,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            VideoDriver::Auto => 0,
            VideoDriver::Wayland => 1,
            VideoDriver::X11 => 2,
        }
    }
}

static PREFERRED: AtomicU8 = AtomicU8::new(0);

/// Asks the next SDL initialisation for `driver`. Called once at startup, before the
/// window is made; a change later takes effect only after a restart.
pub fn set_video_driver(driver: VideoDriver) {
    PREFERRED.store(driver.to_u8(), Ordering::Relaxed);
}

/// The driver stored by [`set_video_driver`].
pub fn preferred_video_driver() -> VideoDriver {
    VideoDriver::from_u8(PREFERRED.load(Ordering::Relaxed))
}

/// The driver hint to set before `SDL_Init`, if any: none for [`VideoDriver::Auto`],
/// off Linux, or when the environment sets the driver ([`env_sets_driver`]).
pub(crate) fn driver_hint(setting: VideoDriver, env_set: bool) -> Option<&'static CStr> {
    if !cfg!(target_os = "linux") || env_set {
        return None;
    }
    setting.sdl_name()
}

/// Runs `init` with `hint`; when a requested driver fails to start, logs a warning and
/// runs `init` again with SDL's own choice. `init` must leave SDL shut down on failure.
pub(crate) fn init_with_fallback<E: std::fmt::Display>(
    hint: Option<&CStr>,
    mut init: impl FnMut(Option<&CStr>) -> Result<(), E>,
) -> Result<(), E> {
    match (init(hint), hint) {
        (Err(e), Some(name)) => {
            tracing::warn!(
                "video driver {:?} failed to start: {e}; falling back to SDL's choice",
                name
            );
            init(None)
        }
        (result, _) => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for d in VideoDriver::ALL {
            assert_eq!(VideoDriver::parse(d.as_str()), Some(d));
            assert_eq!(VideoDriver::from_u8(d.to_u8()), d);
        }
        assert_eq!(VideoDriver::parse("X11"), None);
        assert_eq!(VideoDriver::parse("kmsdrm"), None);
    }

    #[test]
    fn stored_choice_reads_back() {
        set_video_driver(VideoDriver::X11);
        assert_eq!(preferred_video_driver(), VideoDriver::X11);
        set_video_driver(VideoDriver::Auto);
        assert_eq!(preferred_video_driver(), VideoDriver::Auto);
    }

    #[test]
    fn auto_sets_no_hint() {
        assert_eq!(driver_hint(VideoDriver::Auto, false), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn setting_names_the_driver_on_linux() {
        assert_eq!(driver_hint(VideoDriver::Wayland, false), Some(c"wayland"));
        assert_eq!(driver_hint(VideoDriver::X11, false), Some(c"x11"));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn setting_is_ignored_off_linux() {
        assert_eq!(driver_hint(VideoDriver::Wayland, false), None);
        assert_eq!(driver_hint(VideoDriver::X11, false), None);
    }

    #[test]
    fn environment_wins_over_the_setting() {
        for d in VideoDriver::ALL {
            assert_eq!(driver_hint(d, true), None);
        }
    }

    #[test]
    fn failed_driver_falls_back_to_auto() {
        let mut tried = Vec::new();
        let result = init_with_fallback(Some(c"wayland"), |hint| {
            tried.push(hint.map(CStr::to_owned));
            if hint.is_some() {
                Err("no display")
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Ok(()));
        assert_eq!(tried, [Some(c"wayland".to_owned()), None]);
    }

    #[test]
    fn auto_failure_is_not_retried() {
        let mut calls = 0;
        let result = init_with_fallback(None, |_| {
            calls += 1;
            Err("no display")
        });
        assert_eq!(result, Err("no display"));
        assert_eq!(calls, 1);
    }

    #[test]
    fn working_driver_runs_once() {
        let mut calls = 0;
        let result: Result<(), &str> = init_with_fallback(Some(c"x11"), |_| {
            calls += 1;
            Ok(())
        });
        assert_eq!(result, Ok(()));
        assert_eq!(calls, 1);
    }
}
