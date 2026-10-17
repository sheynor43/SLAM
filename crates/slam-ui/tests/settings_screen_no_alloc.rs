//! The settings screen's steady state does not touch the allocator: input on the update
//! side and `SettingsView::push` on the draw side once warm.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::screen::Pose;
use slam_ui::settings_screen::{
    RowKind, RowLabels, RowSpec, SettingsInput, SettingsLabels, SettingsLayout, SettingsScreen,
    SettingsView,
};
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

fn rows() -> Vec<RowSpec> {
    let row = |section, kind| RowSpec {
        section,
        kind,
        restart_required: section == 0,
    };
    vec![
        row(
            0,
            RowKind::Slider {
                min: 0.0,
                max: 100.0,
                step: 1.0,
                value: 30.0,
            },
        ),
        row(0, RowKind::Toggle { value: true }),
        row(
            1,
            RowKind::Dropdown {
                options: 3,
                selected: 1,
            },
        ),
    ]
}

fn labels() -> SettingsLabels {
    let row = |label: &str, unit: &str, options: &[&str]| RowLabels {
        label: label.into(),
        unit: unit.into(),
        options: options.iter().map(|o| (*o).to_owned()).collect(),
    };
    SettingsLabels {
        title: "Settings".into(),
        sections: vec!["Audio".into(), "Frame".into()],
        rows: vec![
            row("Master", "%", &[]),
            row("Colours", "", &[]),
            row("Rate", "", &["60 Hz", "144 Hz", "Unlimited"]),
        ],
        restart: "after restart".into(),
    }
}

#[test]
fn settings_input_does_not_allocate() {
    let layout = SettingsLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SettingsScreen::new(&rows(), 2);
    assert_no_alloc(|| {
        for input in [
            SettingsInput::Right,
            SettingsInput::Down,
            SettingsInput::Activate,
            SettingsInput::NextSection,
            SettingsInput::Activate,
            SettingsInput::Down,
            SettingsInput::Back,
            SettingsInput::PointerMove { x: 640.0, y: 300.0 },
            SettingsInput::PointerPress { x: 640.0, y: 300.0 },
            SettingsInput::PointerRelease,
            SettingsInput::PrevSection,
            SettingsInput::Back,
        ] {
            std::hint::black_box(screen.input(input, &layout));
        }
        std::hint::black_box(screen.snapshot());
    });
}

#[test]
fn the_view_pushes_without_allocating_once_warm() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let limits = Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits)).unwrap();
    let theme = Theme::default();
    let specs = rows();
    let mut screen = SettingsScreen::new(&specs, 2);
    let mut view = SettingsView::new(registry, theme, &specs, &labels());
    let white = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: Extent::new(1, 1),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    };
    let (mut shapes, mut text) = (
        SpriteBatch::with_capacity(256),
        SpriteBatch::with_capacity(1024),
    );
    let pose = Pose {
        opacity: 0.5,
        offset: 0.05,
    };
    let layout = SettingsLayout::new(1280.0, 720.0, &theme);
    let mut frame = |screen: &SettingsScreen, width: f32, height: f32, glyphs: &mut _| {
        shapes.clear();
        text.clear();
        view.push(
            &mut shapes,
            &mut text,
            glyphs,
            width,
            height,
            &screen.snapshot(),
            pose,
            white,
        );
    };
    // Warm: every section and an open list, at each size.
    for _ in 0..2 {
        frame(&screen, 1280.0, 720.0, &mut glyphs);
        screen.input(SettingsInput::Right, &layout);
        frame(&screen, 800.0, 600.0, &mut glyphs);
        screen.input(SettingsInput::NextSection, &layout);
        screen.input(SettingsInput::Activate, &layout);
        frame(&screen, 1280.0, 720.0, &mut glyphs);
        screen.input(SettingsInput::Back, &layout);
        screen.input(SettingsInput::PrevSection, &layout);
    }
    assert_no_alloc(|| {
        for i in 0..40 {
            // A slider that changes every frame, sections, an open list.
            screen.input(SettingsInput::Right, &layout);
            if i % 10 == 5 {
                screen.input(SettingsInput::NextSection, &layout);
                screen.input(SettingsInput::Activate, &layout);
            }
            if i % 10 == 9 {
                screen.input(SettingsInput::Back, &layout);
                screen.input(SettingsInput::PrevSection, &layout);
            }
            frame(&screen, 1280.0, 720.0, &mut glyphs);
        }
    });
    assert!(shapes.len() >= 3);
}
