//! Tracy profiler integration behind the `tracy` cargo feature. Without it every
//! item here is an empty function or a macro that expands to nothing, so call sites
//! cost nothing.
//!
//! With the feature, call [`start`] at program start and connect the Tracy viewer
//! (localhost only). Before `start`, zones, frame marks and plots are skipped; after
//! it, they are recorded only while a viewer is connected. Each zone site allocates
//! once, on its first entry with the client running (Tracy's lazily built source
//! location), so the first frame or tick of a profiled build allocates.

#[cfg(feature = "tracy")]
#[doc(hidden)]
pub use tracy_client;

/// Starts the Tracy client, which then runs until the process exits; later calls do
/// nothing. Without the `tracy` feature, does nothing.
pub fn start() {
    #[cfg(feature = "tracy")]
    let _ = tracy_client::Client::start();
}

/// Whether profiling is compiled in.
pub const fn enabled() -> bool {
    cfg!(feature = "tracy")
}

/// Marks the end of a frame (the draw thread's main frame set).
#[inline(always)]
pub fn frame_mark() {
    #[cfg(feature = "tracy")]
    if let Some(client) = tracy_client::Client::running() {
        client.frame_mark();
    }
}

/// Opens a Tracy zone named by a string literal until the end of the enclosing block.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! zone {
    ($name:literal) => {
        let _slam_zone = $crate::profiling::tracy_client::Client::running()
            .map(|client| client.span($crate::profiling::tracy_client::span_location!($name), 0));
    };
}

/// Opens a Tracy zone named by a string literal until the end of the enclosing block.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! zone {
    ($name:literal) => {};
}

/// Adds a point to the Tracy plot named by a string literal.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! plot {
    ($name:literal, $value:expr) => {
        if let Some(client) = $crate::profiling::tracy_client::Client::running() {
            client.plot($crate::profiling::tracy_client::plot_name!($name), $value);
        }
    };
}

/// Adds a point to the Tracy plot named by a string literal.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! plot {
    // Type-checked and counted as used, never evaluated.
    ($name:literal, $value:expr) => {
        if false {
            let _: f64 = $value;
        }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn macros_are_usable_without_a_running_client() {
        crate::zone!("test zone");
        crate::plot!("test plot", 1.0);
        super::frame_mark();
        assert_eq!(super::enabled(), cfg!(feature = "tracy"));
    }
}
