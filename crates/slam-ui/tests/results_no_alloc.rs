//! The results screen's steady state does not touch the allocator: input and snapshots on
//! the update side, `ResultsView::push` on the draw side once the data's texts are laid out.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::results::{
    Rank, Results, ResultsData, ResultsInput, ResultsLabels, ResultsLayout, ResultsMods,
    ResultsStat, ResultsView, StatKind,
};
use slam_ui::screen::Pose;
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

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

fn labels() -> ResultsLabels {
    ResultsLabels {
        score: "Score".into(),
        accuracy: "Accuracy".into(),
        max_combo: "Max combo".into(),
        mods: "Mods".into(),
        no_mods: "None".into(),
        stats: StatKind::ALL.map(|kind| format!("stat {}", kind.index())),
        retry: "Retry".into(),
        back: "Back".into(),
        hint: "hint".into(),
    }
}

fn data(id: u64) -> ResultsData {
    ResultsData {
        id,
        title: "artist - title [diff]".into(),
        score: 1_234_567,
        accuracy: 0.9876,
        rank: Rank::SH,
        stats: StatKind::ALL.map(|_| ResultsStat {
            value: 12,
            maximum: 20,
        }),
        combo: 99,
        max_combo: 120,
        mods: ResultsMods::new(["HD", "DT"]),
    }
}

#[test]
fn input_does_not_allocate() {
    let layout = ResultsLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = Results::new(data(1));
    assert_no_alloc(|| {
        for input in [
            ResultsInput::PointerMove { x: 640.0, y: 680.0 },
            ResultsInput::PointerPress { x: 640.0, y: 680.0 },
            ResultsInput::Retry,
            ResultsInput::Back,
        ] {
            std::hint::black_box(screen.input(input, &layout));
        }
        std::hint::black_box(screen.snapshot());
    });
}

#[test]
fn view_pushes_without_allocating_once_warm() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let mut view = ResultsView::new(registry, Theme::default(), &labels());
    let mut shapes = SpriteBatch::with_capacity(256);
    let mut text = SpriteBatch::with_capacity(8192);
    let screen = Results::new(data(1));
    let snapshot = screen.snapshot();
    let pose = Pose {
        opacity: 0.5,
        offset: 0.05,
    };
    let mut frame = |snapshot: &_| {
        shapes.clear();
        text.clear();
        view.push(
            &mut shapes,
            &mut text,
            &mut glyphs,
            1280.0,
            720.0,
            snapshot,
            pose,
            white(),
        );
    };
    frame(&snapshot);
    frame(&snapshot);
    assert_no_alloc(|| {
        for _ in 0..10 {
            frame(&snapshot);
        }
    });
    assert!(shapes.len() > 8);
}
