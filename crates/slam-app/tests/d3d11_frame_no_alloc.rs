//! The Direct3D 11 backend's frame through `Backend` must not allocate, with the
//! window-thread check device made and released before the draw side makes its own on
//! the same window (ADR-0080, ADR-0082). Needs a desktop session (the Windows CI runner
//! has one).
#![cfg(windows)]

#[path = "common/backend_frames.rs"]
mod common;

slam_testkit::install_counting_allocator!();

#[test]
fn d3d11_frames_do_not_allocate() {
    common::assert_frames_do_not_allocate(slam_app::backend::Renderer::D3d11);
}
