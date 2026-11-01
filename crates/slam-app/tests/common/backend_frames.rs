//! A scene's whole frame through [`Backend`], from `begin_frame` to `present`, for the
//! no-allocation tests of each backend (ADR-0080). The scene is the benchmark's, with
//! the GPU wait of `--finish`. The test binary installs the counting allocator.

use slam_app::backend::{self, Backend, Opened, Renderer, WindowKind, WithBackend};
use slam_app::bench::{Recorder, Scene, ThreadSched};
use slam_engine::limiter::FrameWait;
use slam_engine::{Draw, FrameInfo};
use slam_input::{InputWindow, PriorityRequest};

const FRAMES: u64 = 100;

struct Frames;

impl WithBackend for Frames {
    type Output = ();

    fn run<B: Backend>(self, window: InputWindow, opened: Opened<B>) {
        let recorder = Recorder::new(0, u64::MAX, FRAMES as usize);
        let mut scene = Scene::new(
            opened.surface,
            500,
            0,
            recorder,
            window.quit_handle(),
            true,
            ThreadSched::default(),
        );
        scene.start();
        let frame = |scene: &mut Scene<B>, n: u64| {
            let wait = FrameWait {
                deadline_ns: n * 1_000_000,
                woke_ns: n * 1_000_000 + 3,
                resynced: false,
            };
            scene.frame(&FrameInfo { wait, fresh: true }, &());
        };
        // The driver may set itself up on the first frames.
        for n in 0..3 {
            frame(&mut scene, n);
        }
        let (_, stats) = slam_testkit::count_allocs(|| {
            for n in 3..FRAMES {
                frame(&mut scene, n);
            }
        });
        scene.stop();
        assert_eq!(stats.total(), 0, "{} frame allocated: {stats}", B::NAME);
        drop(scene);
        drop(window);
    }
}

pub fn assert_frames_do_not_allocate(renderer: Renderer) {
    let window = |kind| {
        match kind {
            WindowKind::OpenGl => InputWindow::new_opengl("SLAM backend test", 320, 240),
            WindowKind::Native => InputWindow::new("SLAM backend test", 320, 240),
        }
        .map_err(|e| e.to_string())
    };
    backend::run(renderer, PriorityRequest::Default, window, Frames).expect("backend");
}
