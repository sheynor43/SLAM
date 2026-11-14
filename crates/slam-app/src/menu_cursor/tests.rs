use std::path::PathBuf;

use slam_render::Origin;
use slam_render::atlas::UvRect;
use slam_render::hal::Extent;
use slam_render::textures::TextureId;

use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("slam-menu-cursor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn with(self, file: &str) -> Self {
        std::fs::write(self.0.join(file), b"x").unwrap();
        self
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn white() -> AtlasRegion {
    AtlasRegion {
        texture: TextureId(1),
        origin: Origin { x: 0, y: 0 },
        size: Extent::new(1, 1),
        uv: UvRect::FULL,
    }
}

fn settings(menu: CursorStyle, game: CursorStyle, relative: bool) -> Settings {
    let mut s = Settings::default();
    s.general.menu_cursor = menu;
    s.gameplay.cursor_style = game;
    s.input.high_precision_mouse = relative;
    s
}

#[test]
fn the_os_cursor_hides_with_a_cursor_of_ours_in_the_menus_and_in_gameplay() {
    let menus = |menu, play| os_cursor_hidden(&settings(menu, CursorStyle::Skin, false), play);
    assert!(menus(CursorStyle::Slam, None));
    assert!(menus(CursorStyle::Skin, None));
    assert!(!menus(CursorStyle::System, None));
    // Paused, failed and the results are menus: the menu cursor decides.
    for state in [PlayState::Paused, PlayState::Failed, PlayState::Results] {
        assert!(!menus(CursorStyle::System, Some(state)), "{state:?}");
        assert!(menus(CursorStyle::Slam, Some(state)), "{state:?}");
    }
    // Gameplay: its own setting decides, whatever the menu's.
    for (game, hidden) in [
        (CursorStyle::Skin, true),
        (CursorStyle::Slam, true),
        (CursorStyle::System, false),
    ] {
        for state in [PlayState::Waiting, PlayState::Playing, PlayState::Passed] {
            let s = settings(CursorStyle::System, game, false);
            assert_eq!(os_cursor_hidden(&s, Some(state)), hidden, "{game:?}");
            let s = settings(CursorStyle::Slam, game, false);
            assert_eq!(os_cursor_hidden(&s, Some(state)), hidden, "{game:?}");
        }
    }
}

#[test]
fn relative_mouse_makes_the_system_game_cursor_the_skins() {
    let s = settings(CursorStyle::System, CursorStyle::System, true);
    assert_eq!(game_style(&s), CursorStyle::Skin);
    assert!(os_cursor_hidden(&s, Some(PlayState::Playing)));
    // The menus are not affected.
    assert!(!os_cursor_hidden(&s, None));
    let s = settings(CursorStyle::Slam, CursorStyle::System, false);
    assert_eq!(game_style(&s), CursorStyle::System);
    let s = settings(CursorStyle::Slam, CursorStyle::Slam, true);
    assert_eq!(game_style(&s), CursorStyle::Slam);
}

fn chain(user: Option<&TempDir>, default: &TempDir) -> SkinChain {
    let default = Skin::load_default(&default.0).unwrap();
    let user = user.map(|d| Skin::load(&d.0).unwrap());
    SkinChain::new(user, default)
}

#[test]
fn the_skins_cursor_comes_from_the_first_skin_with_one() {
    let default = TempDir::new("default-a")
        .with("cursor@2x.png")
        .with("cursormiddle.png");
    let user = TempDir::new("user-a").with("cursor.png");
    let files = cursor_files(&chain(Some(&user), &default));
    let (cursor, scale) = files.cursor.unwrap();
    assert_eq!(
        (cursor.file_name().unwrap().to_str().unwrap(), scale),
        ("cursor.png", 1.0)
    );
    // `cursormiddle` is the cursor skin's own: the user's skin has none.
    assert_eq!(files.middle, None);
    let files = cursor_files(&chain(None, &default));
    assert_eq!(files.cursor.unwrap().1, 2.0);
    assert!(files.middle.is_some());
}

#[test]
fn a_chain_without_a_cursor_has_none_to_load() {
    let default = TempDir::new("default-b");
    let user = TempDir::new("user-b");
    let files = cursor_files(&chain(Some(&user), &default));
    assert_eq!((files.cursor, files.middle), (None, None));
}

fn sprite(size: f32) -> SkinSprite {
    SkinSprite {
        texture: TextureId(2),
        uv: UvRect::FULL,
        size: slam_osu::Vec2::new(size, size),
    }
}

#[test]
fn the_skin_cursor_expands_and_the_middle_stays() {
    let mut shapes = ShapeBatch::with_capacity(MAX_MENU_CURSOR_SHAPES);
    let config = CursorSkinConfig::default();
    push_skin_cursor(
        &mut shapes,
        &sprite(100.0),
        Some(&sprite(40.0)),
        &config,
        [200.0, 100.0],
        1.0,
        1.3,
        0.0,
    );
    assert_eq!(shapes.len(), 2);
    let width = |from: usize| {
        let v = &shapes.vertices()[from..from + 4];
        v[1].position[0] - v[0].position[0]
    };
    assert!((width(0) - 130.0).abs() < 1e-3);
    assert!((width(4) - 40.0).abs() < 1e-3);
    // Centred on the pointer.
    let v = &shapes.vertices()[0..4];
    assert!((v[0].position[0] - 135.0).abs() < 1e-3);
}

fn view(pointer: &PointerWatch) -> MenuCursorView {
    MenuCursorView::new(pointer.clone(), WindowScale::fixed(2.0), None)
}

fn draw(view: &mut MenuCursorView, frame: &MenuCursorFrame, now_ns: u64) -> ShapeBatch {
    let mut shapes = ShapeBatch::with_capacity(MAX_MENU_CURSOR_SHAPES);
    view.push(
        &mut shapes,
        &Theme::default(),
        white(),
        [1024.0, 768.0],
        now_ns,
        frame,
    );
    shapes
}

#[test]
fn nothing_is_drawn_before_the_pointer_moves_for_the_os_cursor_or_over_gameplay() {
    let pointer = PointerWatch::default();
    let mut view = view(&pointer);
    let slam = MenuCursorFrame::default();
    assert!(draw(&mut view, &slam, 0).is_empty());
    pointer.record_move(10.0, 20.0);
    assert!(!draw(&mut view, &slam, 1).is_empty());
    let system = MenuCursorFrame {
        style: CursorStyle::System,
        ..slam
    };
    assert!(draw(&mut view, &system, 2).is_empty());
    let gameplay = MenuCursorFrame {
        gameplay_cursor: true,
        ..slam
    };
    assert!(draw(&mut view, &gameplay, 3).is_empty());
}

#[test]
fn the_slam_cursor_follows_the_freshest_position_in_pixels() {
    let pointer = PointerWatch::default();
    let mut view = view(&pointer);
    let frame = MenuCursorFrame::default();
    pointer.record_move(10.0, 20.0);
    pointer.record_move(30.0, 40.0);
    let shapes = draw(&mut view, &frame, 0);
    // Points to pixels at 2 pixels per point: the tip at (60, 80).
    let min_x = shapes
        .vertices()
        .iter()
        .map(|v| v.position[0])
        .fold(f32::MAX, f32::min);
    let min_y = shapes
        .vertices()
        .iter()
        .map(|v| v.position[1])
        .fold(f32::MAX, f32::min);
    // The pointer leans to the left: its tip is the top, a little of it reaches left.
    assert!(
        (min_x - 60.0).abs() < 25.0 && (min_y - 80.0).abs() < 8.0,
        "{min_x} {min_y}"
    );
}

#[test]
fn the_skin_cursor_falls_back_to_slams_when_the_chain_has_none() {
    let pointer = PointerWatch::default();
    pointer.record_move(5.0, 5.0);
    let mut view = view(&pointer);
    let mut frame = MenuCursorFrame {
        style: CursorStyle::Skin,
        ..MenuCursorFrame::default()
    };
    // Not answered yet: nothing.
    assert!(draw(&mut view, &frame, 0).is_empty());
    // Answered without a cursor: SLAM's.
    frame.skin.loaded = true;
    assert!(!draw(&mut view, &frame, 1).is_empty());
}

#[test]
fn a_skin_cursor_whose_loader_is_missing_shows_as_slams() {
    let pointer = PointerWatch::default();
    pointer.record_move(5.0, 5.0);
    let mut view = view(&pointer);
    let frame = MenuCursorFrame {
        style: CursorStyle::Skin,
        skin: MenuCursorSkin {
            loaded: true,
            cursor: Some(SkinImage {
                handle: dangling_handle(),
                scale: 1.0,
            }),
            ..MenuCursorSkin::default()
        },
        ..MenuCursorFrame::default()
    };
    assert!(!draw(&mut view, &frame, 0).is_empty());
}

fn dangling_handle() -> TextureHandle {
    // A handle of a loader that is gone: requested from one that is dropped at once.
    let limits = slam_render::hal::Limits {
        max_texture_size: 1024,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let (mut requests, _uploads) = texture_loader(&TextureLoaderConfig::new(limits)).unwrap();
    requests.load(vec![0], TextureKind::Standalone).unwrap()
}

/// How far the pointer's own shapes (not the glow after them) reach from `tip`.
fn reach(shapes: &ShapeBatch, tip: [f32; 2]) -> f32 {
    shapes
        .vertices()
        .iter()
        .take(6 * 4)
        .map(|v| (v.position[0] - tip[0]).hypot(v.position[1] - tip[1]))
        .fold(0.0, f32::max)
}

#[test]
fn a_press_shrinks_the_slam_cursor_only_with_effects() {
    let pointer = PointerWatch::default();
    pointer.record_move(100.0, 100.0);
    let frame = MenuCursorFrame::default();
    let tip = [200.0, 200.0];
    let mut view = view(&pointer);
    let rest = reach(&draw(&mut view, &frame, 0), tip);
    pointer.record_button(slam_input::MouseButton::Left, true);
    let _ = draw(&mut view, &frame, 1_000);
    let shrunk = reach(&draw(&mut view, &frame, 1_000_000_000), tip);
    assert!(shrunk < rest * 0.95, "{shrunk} {rest}");
}

#[test]
fn a_right_drag_turns_the_slam_cursor_and_releasing_returns_it() {
    let pointer = PointerWatch::default();
    pointer.record_move(100.0, 100.0);
    let frame = MenuCursorFrame::default();
    let mut view = view(&pointer);
    let tip = |shapes: &ShapeBatch| shapes.vertices()[0].position;
    let rest = tip(&draw(&mut view, &frame, 0));
    pointer.record_button(slam_input::MouseButton::Right, true);
    let _ = draw(&mut view, &frame, 1_000_000);
    // A far drag to the right.
    pointer.record_move(400.0, 100.0);
    let _ = draw(&mut view, &frame, 2_000_000);
    let turned = tip(&draw(&mut view, &frame, 1_500_000_000));
    assert!(
        (turned[0] - rest[0]).abs() > 1.0 || (turned[1] - rest[1]).abs() > 1.0,
        "{rest:?} {turned:?}"
    );
    pointer.record_button(slam_input::MouseButton::Right, false);
    pointer.record_move(100.0, 100.0);
    let _ = draw(&mut view, &frame, 1_600_000_000);
    let back = tip(&draw(&mut view, &frame, 4_000_000_000));
    assert!(
        (back[0] - rest[0]).abs() < 0.5 && (back[1] - rest[1]).abs() < 0.5,
        "{rest:?} {back:?}"
    );
}

#[test]
fn nothing_is_drawn_while_the_pointer_is_outside_the_window() {
    let pointer = PointerWatch::default();
    pointer.record_move(10.0, 10.0);
    let mut view = view(&pointer);
    let frame = MenuCursorFrame::default();
    assert!(!draw(&mut view, &frame, 0).is_empty());
    pointer.record_outside(true);
    assert!(draw(&mut view, &frame, 1).is_empty());
}

// ---- Release protocol ----

/// The loader thread's side, played by the test: real handles of a four-texture loader.
struct Loader {
    requests: TextureRequests,
    commands: rtrb::Consumer<Command>,
    replies: rtrb::Producer<Reply>,
    /// Handles released so far.
    released: Vec<TextureHandle>,
    /// Loads that found no free handle.
    starved: u32,
}

fn rig() -> (SkinCursorLink, Loader, CursorShown) {
    let limits = slam_render::hal::Limits {
        max_texture_size: 1024,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let (requests, _uploads) = texture_loader(&TextureLoaderConfig {
        max_textures: MAX_CURSOR_TEXTURES,
        request_capacity: 512,
        ..TextureLoaderConfig::new(limits)
    })
    .unwrap();
    let (commands, commands_rx) = rtrb::RingBuffer::new(RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(RING);
    let shown = CursorShown::default();
    let link = SkinCursorLink::new(commands, replies, std::thread::current(), shown.0.clone());
    (
        link,
        Loader {
            requests,
            commands: commands_rx,
            replies: replies_tx,
            released: Vec::new(),
            starved: 0,
        },
        shown,
    )
}

impl Loader {
    /// Handles every command: a load answers with two images.
    fn serve(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Release(handle) => {
                    self.requests.release(handle).unwrap();
                    self.released.push(handle);
                }
                Command::Load { generation, .. } => {
                    let image = |requests: &mut TextureRequests| {
                        requests
                            .load(vec![0], TextureKind::Standalone)
                            .ok()
                            .map(|handle| SkinImage { handle, scale: 1.0 })
                    };
                    let cursor = image(&mut self.requests);
                    let middle = image(&mut self.requests);
                    if cursor.is_none() || middle.is_none() {
                        self.starved += 1;
                    }
                    let skin = MenuCursorSkin {
                        loaded: true,
                        cursor,
                        middle,
                        config: CursorSkinConfig::default(),
                    };
                    self.replies.push(Reply { generation, skin }).ok().unwrap();
                }
            }
        }
    }
}

fn skin(name: &str) -> Arc<Path> {
    Arc::from(Path::new(name))
}

#[test]
fn the_old_cursor_stays_until_the_new_one_is_in_and_goes_after_the_draw_moved_on() {
    let (mut link, mut loader, shown) = rig();
    let (a, b) = (skin("a"), skin("b"));
    link.tick(Some(Some(&a)));
    loader.serve();
    let first = link.tick(Some(Some(&a)));
    assert!(first.loaded && first.cursor.is_some());
    // A different skin: until its answer the first stays.
    assert_eq!(link.tick(Some(Some(&b))), first);
    assert_eq!(link.tick(Some(Some(&b))), first);
    loader.serve();
    let second = link.tick(Some(Some(&b)));
    assert_ne!(second.cursor, first.cursor);
    let swapped_at = link.seq();
    // The draw thread may still be on a frame with the first images.
    shown.set(swapped_at - 1);
    link.tick(Some(Some(&b)));
    loader.serve();
    assert!(loader.released.is_empty());
    assert_eq!(link.retiring(), 2);
    shown.set(swapped_at);
    link.tick(Some(Some(&b)));
    loader.serve();
    assert_eq!(
        loader.released,
        vec![first.cursor.unwrap().handle, first.middle.unwrap().handle]
    );
    assert_eq!(link.retiring(), 0);
}

#[test]
fn going_back_to_the_os_cursor_releases_after_the_draw_too() {
    let (mut link, mut loader, shown) = rig();
    let a = skin("a");
    link.tick(Some(Some(&a)));
    loader.serve();
    let loaded = link.tick(Some(Some(&a)));
    assert!(loaded.cursor.is_some());
    // The frame of this tick no longer has the images; the draw thread is not there yet.
    assert_eq!(link.tick(None), MenuCursorSkin::default());
    let left_at = link.seq();
    loader.serve();
    assert!(loader.released.is_empty());
    shown.set(left_at);
    link.tick(None);
    loader.serve();
    assert_eq!(loader.released.len(), 2);
}

#[test]
fn rapid_skin_switching_never_runs_out_of_handles() {
    let (mut link, mut loader, shown) = rig();
    let skins: Vec<Arc<Path>> = (0..40).map(|i| skin(&format!("skin{i}"))).collect();
    let mut last = MenuCursorSkin::default();
    for (i, wish) in skins.iter().enumerate() {
        // Several ticks per switch, the draw thread acknowledging one tick late.
        for _ in 0..3 {
            let shown_skin = link.tick(Some(Some(wish)));
            if shown_skin.loaded {
                last = shown_skin;
            }
            shown.set(link.seq().wrapping_sub(u32::from(i % 2 == 0)));
            loader.serve();
        }
    }
    assert_eq!(loader.starved, 0);
    assert!(last.cursor.is_some() && last.middle.is_some());
}
