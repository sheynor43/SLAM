use std::path::PathBuf;

use slam_skin::Skin;

use super::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("slam-ranking-skin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn with(self, file: &str) -> Self {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
        self
    }

    fn ini(self, text: &str) -> Self {
        std::fs::write(self.0.join("skin.ini"), text).unwrap();
        self
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn chain(user: Option<&TempDir>, default: &TempDir) -> SkinChain {
    let default = Skin::load_default(&default.0).unwrap();
    let user = user.map(|d| Skin::load(&d.0).unwrap());
    SkinChain::new(user, default)
}

fn name(file: &Option<(PathBuf, f32)>) -> Option<String> {
    file.as_ref()
        .map(|(path, _)| path.file_name().unwrap().to_str().unwrap().to_owned())
}

#[test]
fn without_a_panel_there_is_no_stable_screen() {
    let default = TempDir::new("a-default").with("score-0.png");
    let user = TempDir::new("a-user").with("ranking-title.png");
    assert_eq!(ranking_files(&chain(Some(&user), &default)), None);
}

#[test]
fn the_panels_skin_gives_the_images_and_the_settings() {
    let default = TempDir::new("b-default")
        .with("score-0.png")
        .with("score-1.png")
        .with("score-dot.png");
    let user = TempDir::new("b-user")
        .ini("[General]\nVersion: 2.5\n[Fonts]\nScorePrefix: num\nScoreOverlap: 3\nComboOverlap: 1\n")
        .with("ranking-panel@2x.png")
        .with("ranking-S.png")
        .with("ranking-D.png")
        .with("hit300-0.png")
        .with("hit300-1.png")
        .with("hit100.png")
        .with("num-0.png")
        .with("num-percent.png")
        .with("selection-mod-hidden.png");
    let files = ranking_files(&chain(Some(&user), &default)).unwrap();
    assert!(files.modern);
    assert_eq!((files.score_overlap, files.combo_overlap), (3.0, 1.0));
    let images = &files.images;
    let panel = images.panel.as_ref().unwrap();
    assert_eq!(
        (panel.0.file_name().unwrap().to_str().unwrap(), panel.1),
        ("ranking-panel@2x.png", 2.0)
    );
    assert_eq!(name(&images.grades[3]).as_deref(), Some("ranking-S.png"));
    assert_eq!(name(&images.grades[7]).as_deref(), Some("ranking-D.png"));
    assert_eq!(images.grades[0], None);
    // The first frame of an animation, or the static image.
    assert_eq!(name(&images.hits[0]).as_deref(), Some("hit300-0.png"));
    assert_eq!(name(&images.hits[1]).as_deref(), Some("hit100.png"));
    assert_eq!(images.hits[2], None);
    // Digits fall back on `score-N`, symbols do not (the default's `score-dot` is not used).
    assert_eq!(name(&images.score_font[0]).as_deref(), Some("num-0.png"));
    assert_eq!(name(&images.score_font[1]).as_deref(), Some("score-1.png"));
    assert_eq!(images.score_font[Glyph::Dot.index()], None);
    assert!(images.score_font[Glyph::Percent.index()].is_some());
    // The combo font has the default prefix `score`: its `x` is missing.
    assert_eq!(name(&images.combo_font[0]).as_deref(), Some("score-0.png"));
    assert_eq!(images.combo_font[Glyph::X.index()], None);
    let hidden = MODS
        .iter()
        .position(|m| m.0 == "selection-mod-hidden")
        .unwrap();
    assert!(images.mods[hidden].is_some());
}

#[test]
fn a_skin_without_a_version_is_version_one() {
    let default = TempDir::new("c-default");
    let user = TempDir::new("c-user")
        .ini("[General]\nVersion: 1\n")
        .with("ranking-panel.png");
    let files = ranking_files(&chain(Some(&user), &default)).unwrap();
    assert!(!files.modern);
}

#[test]
fn unloaded_or_panelless_skins_resolve_to_the_slam_screen() {
    let view = RankingView::new(None, None);
    assert!(view.resolve(&RankingSkin::default()).is_none());
    let answered = RankingSkin {
        loaded: true,
        ..RankingSkin::default()
    };
    assert!(view.resolve(&answered).is_none());
    // Even a present skin needs uploads to draw.
    let present = RankingSkin {
        present: true,
        ..answered
    };
    assert!(view.resolve(&present).is_none());
}

fn rig() -> (
    RankingLink,
    rtrb::Consumer<Command>,
    rtrb::Producer<Reply>,
    CursorShown,
) {
    let (commands, commands_rx) = rtrb::RingBuffer::new(RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(4);
    let shown = CursorShown::default();
    let link = RankingLink {
        commands,
        replies,
        thread: std::thread::current(),
        shown: shown.share(),
        seq: 0,
        generation: 0,
        loading: false,
        asked: None,
        current: RankingSkin::default(),
        retiring: [None; MAX_RANKING_TEXTURES as usize],
    };
    (link, commands_rx, replies_tx, shown)
}

#[test]
fn a_new_skin_waits_for_the_old_images_to_be_released() {
    let limits = slam_render::hal::Limits {
        max_texture_size: 1024,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let (mut requests, _uploads) = texture_loader(&TextureLoaderConfig {
        max_textures: MAX_RANKING_TEXTURES,
        request_capacity: 512,
        ..TextureLoaderConfig::new(limits)
    })
    .unwrap();
    let (mut link, mut commands, mut replies, shown) = rig();
    let (a, b): (Arc<Path>, Arc<Path>) = (Arc::from(Path::new("a")), Arc::from(Path::new("b")));
    link.tick(Some(Some(&a)));
    let Ok(Command::Load { generation, .. }) = commands.pop() else {
        panic!("a load was asked for");
    };
    let handle = requests.load(vec![0], TextureKind::Standalone).unwrap();
    let mut skin = RankingSkin {
        loaded: true,
        present: true,
        ..RankingSkin::default()
    };
    skin.images.panel = Some(SkinImage { handle, scale: 1.0 });
    replies.push(Reply { generation, skin }).ok().unwrap();
    assert_eq!(link.tick(Some(Some(&a))), skin);
    // The skin changes: the images go off the screen, and are released only after the
    // draw thread moved past this tick; the new load waits for that.
    let shown_with = link.seq();
    assert_eq!(link.tick(Some(Some(&b))), RankingSkin::default());
    let swapped = link.seq();
    assert_eq!(link.retiring(), 1);
    assert!(commands.pop().is_err());
    shown.set(shown_with);
    link.tick(Some(Some(&b)));
    assert!(commands.pop().is_err());
    shown.set(swapped);
    link.tick(Some(Some(&b)));
    assert!(matches!(commands.pop(), Ok(Command::Release(h)) if h == handle));
    assert_eq!(link.retiring(), 0);
    link.tick(Some(Some(&b)));
    assert!(matches!(commands.pop(), Ok(Command::Load { .. })));
}

#[test]
fn letting_go_releases_the_images_after_the_draw_thread_moved_on() {
    let limits = slam_render::hal::Limits {
        max_texture_size: 1024,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let (mut requests, _uploads) = texture_loader(&TextureLoaderConfig {
        max_textures: MAX_RANKING_TEXTURES,
        request_capacity: 512,
        ..TextureLoaderConfig::new(limits)
    })
    .unwrap();
    let (mut link, mut commands, mut replies, shown) = rig();
    let a: Arc<Path> = Arc::from(Path::new("a"));
    link.tick(Some(Some(&a)));
    let Ok(Command::Load { generation, .. }) = commands.pop() else {
        panic!("a load was asked for");
    };
    let handle = requests.load(vec![0], TextureKind::Standalone).unwrap();
    let mut skin = RankingSkin {
        loaded: true,
        present: true,
        ..RankingSkin::default()
    };
    skin.images.panel = Some(SkinImage { handle, scale: 1.0 });
    replies.push(Reply { generation, skin }).ok().unwrap();
    assert_eq!(link.tick(Some(Some(&a))), skin);
    // Away from plays and results: nothing shows, the images go once the frame of this
    // tick was drawn.
    assert_eq!(link.tick(None), RankingSkin::default());
    let released_at = link.seq();
    assert_eq!(link.retiring(), 1);
    link.tick(None);
    assert!(commands.pop().is_err());
    shown.set(released_at);
    link.tick(None);
    assert!(matches!(commands.pop(), Ok(Command::Release(h)) if h == handle));
    // Asked again, the images load anew.
    link.tick(Some(Some(&a)));
    assert!(matches!(commands.pop(), Ok(Command::Load { .. })));
}

#[test]
fn the_slam_results_style_turns_the_stable_look_off_even_with_a_panel() {
    let mut skin = RankingSkin {
        loaded: true,
        present: true,
        ..RankingSkin::default()
    };
    assert!(skin.stable());
    skin.style = crate::settings::ResultsStyle::Slam;
    assert!(!skin.stable());
    assert!(RankingView::new(None, None).resolve(&skin).is_none());
}
