//! The song select's steady state does not touch the allocator: input and snapshots on the
//! update side, `SongSelectView::push` on the draw side once the rows in view are cached.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::screen::Pose;
use slam_ui::song_select::{
    DifficultyRow, SetRow, SongList, SongSelect, SongSelectInput, SongSelectLabels,
    SongSelectLayout, SongSelectStatus, SongSelectView,
};
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

fn list_of(count: u64) -> Arc<SongList> {
    Arc::new(SongList {
        sets: (0..count)
            .map(|k| SetRow {
                key: k,
                artist: format!("artist {k}"),
                title: format!("title {k}"),
                creator: "mapper".into(),
                difficulties: (0..3)
                    .map(|d| DifficultyRow {
                        version: format!("diff {d}"),
                    })
                    .collect(),
            })
            .collect(),
    })
}

#[test]
fn input_does_not_allocate() {
    let list = list_of(40);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SongSelect::new();
    screen.set_list(list);
    assert_no_alloc(|| {
        for input in [
            SongSelectInput::Down,
            SongSelectInput::Up,
            SongSelectInput::Right,
            SongSelectInput::PageDown,
            SongSelectInput::End,
            SongSelectInput::Home,
            SongSelectInput::Scroll { lines: 2.0 },
            SongSelectInput::PointerMove { x: 900.0, y: 300.0 },
            SongSelectInput::PointerPress { x: 900.0, y: 300.0 },
            SongSelectInput::Activate,
            SongSelectInput::Back,
        ] {
            std::hint::black_box(screen.input(input, &layout));
        }
        std::hint::black_box(screen.snapshot());
    });
}

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

fn white() -> AtlasRegion {
    AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: Extent::new(1, 1),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    }
}

fn labels() -> SongSelectLabels {
    SongSelectLabels {
        no_folder: "no folder".into(),
        failed: "failed".into(),
        loading: "loading".into(),
        empty: "empty".into(),
        hint: "hint".into(),
    }
}

struct Rig {
    view: SongSelectView,
    glyphs: slam_text::GlyphCache,
    shapes: SpriteBatch,
    text: SpriteBatch,
}

impl Rig {
    fn new() -> Rig {
        let registry = Arc::new(FontRegistry::with_builtin());
        let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
        Rig {
            view: SongSelectView::new(registry, Theme::default(), &labels()),
            glyphs,
            shapes: SpriteBatch::with_capacity(256),
            text: SpriteBatch::with_capacity(8192),
        }
    }

    fn frame(&mut self, snapshot: &slam_ui::song_select::SongSelectSnapshot) {
        self.shapes.clear();
        self.text.clear();
        let pose = Pose {
            opacity: 0.5,
            offset: 0.05,
        };
        self.view.push(
            &mut self.shapes,
            &mut self.text,
            &mut self.glyphs,
            1280.0,
            720.0,
            snapshot,
            pose,
            white(),
        );
    }
}

#[test]
fn view_pushes_without_allocating_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    screen.set_list(list_of(40));
    screen.set_status(SongSelectStatus::Scanning { sets: 40 });
    let snapshot = screen.snapshot();
    rig.frame(&snapshot);
    rig.frame(&snapshot);
    assert_no_alloc(|| {
        for _ in 0..10 {
            rig.frame(&snapshot);
        }
    });
    assert!(rig.shapes.len() > 4);
}

#[test]
fn changing_lists_and_scanning_texts_do_not_allocate() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    screen.set_status(SongSelectStatus::Scanning { sets: 1 });
    // Same lengths each time, so the warm buffers suffice. The test keeps every Arc alive,
    // so dropping a snapshot only decrements a count.
    let keep: Vec<_> = (0..6u32)
        .map(|i| {
            screen.set_scanning_text(Arc::from(format!("Scanning songs: {i:04} sets")));
            screen.set_list(list_of(40));
            screen.snapshot()
        })
        .collect();
    for snapshot in &keep {
        rig.frame(snapshot);
    }
    assert_no_alloc(|| {
        for snapshot in keep.iter().cycle().take(30) {
            rig.frame(snapshot);
        }
    });
}

#[test]
fn scrolling_a_long_list_does_not_allocate_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    screen.set_list(list_of(2000));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let pass = |rig: &mut Rig, screen: &mut SongSelect| {
        screen.input(SongSelectInput::Home, &layout);
        for _ in 0..2000 {
            screen.input(SongSelectInput::Scroll { lines: 0.5 }, &layout);
            rig.frame(&screen.snapshot());
        }
    };
    pass(&mut rig, &mut screen);
    slam_testkit::assert_no_alloc(|| pass(&mut rig, &mut screen));
}
