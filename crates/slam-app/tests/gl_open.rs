//! Opening OpenGL through `Backend` hands the context to another thread and back
//! (ADR-0105): the draw thread can then make its device on it.

use slam_app::backend::{self, Backend, Opened, Renderer, WindowKind, WithBackend};
use slam_input::{InputWindow, PriorityRequest};

struct DeviceOnAnotherThread;

impl WithBackend for DeviceOnAnotherThread {
    type Output = &'static str;

    fn run<B: Backend>(self, window: InputWindow, opened: Opened<B>) -> &'static str {
        let surface = opened.surface;
        std::thread::spawn(move || {
            let device = surface.create_device().expect("device on the draw thread");
            B::release(device).expect("release the device");
        })
        .join()
        .expect("draw thread");
        drop(window);
        B::NAME
    }
}

#[test]
#[ignore = "needs a display and an OpenGL 3.3 driver"]
fn auto_opens_opengl_and_hands_its_context_off() {
    let window = |kind| match kind {
        WindowKind::OpenGl => {
            InputWindow::new_opengl("SLAM backend test", 320, 240).map_err(|e| e.to_string())
        }
        WindowKind::Native => Err("Auto must not fall back here".to_owned()),
    };
    let name = backend::run(
        Renderer::Auto,
        PriorityRequest::Default,
        window,
        DeviceOnAnotherThread,
    )
    .expect("backend");
    assert_eq!(name, "OpenGL 3.3");
}
