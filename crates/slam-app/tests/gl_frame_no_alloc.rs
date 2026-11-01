//! The OpenGL backend's frame through `Backend` must not allocate (ADR-0080).

#[path = "common/backend_frames.rs"]
mod common;

slam_testkit::install_counting_allocator!();

#[test]
#[ignore = "needs a display and an OpenGL 3.3 driver"]
fn opengl_frames_do_not_allocate() {
    common::assert_frames_do_not_allocate(slam_app::backend::Renderer::OpenGl);
}
