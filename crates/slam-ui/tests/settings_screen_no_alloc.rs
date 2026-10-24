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
    KeyLabels, KeyValue, MouseKey, RowKind, RowLabels, RowSpec, SettingsAction, SettingsInput,
    SettingsLabels, SettingsLayout, SettingsScreen, SettingsView,
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
        row(
            2,
            RowKind::Key {
                value: KeyValue::Key(29),
            },
        ),
        row(
            2,
            RowKind::Key {
                value: KeyValue::Empty,
            },
        ),
        row(2, RowKind::Button),
    ]
}

fn labels() -> SettingsLabels {
    let row = |label: &str, unit: &str, options: &[&str]| RowLabels {
        label: label.into(),
        unit: unit.into(),
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        button: String::new(),
        shows_value: false,
    };
    SettingsLabels {
        title: "Settings".into(),
        sections: vec!["Audio".into(), "Frame".into(), "Keys".into()],
        rows: vec![
            row("Master", "%", &[]),
            row("Colours", "", &[]),
            row("Rate", "", &["60 Hz", "144 Hz", "Unlimited"]),
            row("Left (1)", "", &[]),
            row("Left (2)", "", &[]),
            RowLabels {
                button: "Not selected".into(),
                shows_value: true,
                ..row("Songs folder", "", &[])
            },
        ],
        restart: "after restart".into(),
        keys: KeyLabels {
            empty: "None".into(),
            prompt: "Press a key...".into(),
            keys: (0..512).map(|i| format!("Key {i}")).collect(),
            mouse: ["Left", "Middle", "Right", "X1", "X2"].map(String::from),
            conflict_title: "Already bound:".into(),
            conflict_keep: "Keep".into(),
            conflict_apply: "Apply".into(),
        },
    }
}

#[test]
fn settings_input_does_not_allocate() {
    let layout = SettingsLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SettingsScreen::new(&rows(), 3);
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

/// Listening, capture, the conflict question and the button, on the keys section (row 3 is
/// the first key row).
fn key_flow(screen: &mut SettingsScreen, layout: &SettingsLayout) {
    use SettingsInput::*;
    screen.input(NextSection, layout);
    screen.input(NextSection, layout);
    screen.input(Activate, layout);
    std::hint::black_box(screen.is_listening());
    let action = screen.input(Captured(KeyValue::Key(30)), layout);
    if let Some(SettingsAction::KeyCaptured { row, key }) = action {
        screen.set_key(row, key);
    }
    screen.input(Down, layout);
    screen.input(Activate, layout);
    screen.input(Back, layout);
    screen.input(Activate, layout);
    screen.input(Clear, layout);
    screen.input(Activate, layout);
    let action = screen.input(Captured(KeyValue::Mouse(MouseKey::Left)), layout);
    if let Some(SettingsAction::KeyCaptured { row, key }) = action {
        screen.ask_conflict(row, key, Some(3));
    }
    std::hint::black_box(screen.snapshot());
    screen.input(Right, layout);
    screen.input(Left, layout);
    screen.input(Right, layout);
    screen.input(Activate, layout);
    screen.input(Down, layout);
    std::hint::black_box(screen.input(Activate, layout));
    screen.input(PrevSection, layout);
    screen.input(PrevSection, layout);
}

#[test]
fn key_input_does_not_allocate() {
    let layout = SettingsLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SettingsScreen::new(&rows(), 3);
    assert_no_alloc(|| {
        key_flow(&mut screen, &layout);
        key_flow(&mut screen, &layout);
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
    let mut screen = SettingsScreen::new(&specs, 3);
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
    // Warm the keys section: listening and the question, and the button that shows a path,
    // short and longer than the button.
    let paths: [Arc<str>; 3] = [
        Arc::from("D:/osu!/Songs"),
        Arc::from(
            "/home/player/.local/share/osu-wine/osu!/drive_c/users/steamuser/Games/osu/Songs/with/an/\u{00e9}normously/long/path/that/never/fits",
        ),
        Arc::from(""),
    ];
    for _ in 0..2 {
        screen.input(SettingsInput::NextSection, &layout);
        screen.input(SettingsInput::NextSection, &layout);
        for path in &paths {
            screen.set_value_text(Arc::clone(path));
            frame(&screen, 1280.0, 720.0, &mut glyphs);
            frame(&screen, 800.0, 600.0, &mut glyphs);
            frame(&screen, 1280.0, 720.0, &mut glyphs);
        }
        screen.input(SettingsInput::PrevSection, &layout);
        screen.input(SettingsInput::PrevSection, &layout);
    }
    for _ in 0..2 {
        screen.input(SettingsInput::NextSection, &layout);
        screen.input(SettingsInput::NextSection, &layout);
        screen.input(SettingsInput::Activate, &layout);
        frame(&screen, 1280.0, 720.0, &mut glyphs);
        screen.input(SettingsInput::Back, &layout);
        screen.ask_conflict(3, KeyValue::Mouse(MouseKey::X2), Some(4));
        frame(&screen, 1280.0, 720.0, &mut glyphs);
        screen.input(SettingsInput::Back, &layout);
        frame(&screen, 800.0, 600.0, &mut glyphs);
        screen.input(SettingsInput::PrevSection, &layout);
        screen.input(SettingsInput::PrevSection, &layout);
    }
    // A resize lays everything out again, allocating; end on the measured size.
    frame(&screen, 1280.0, 720.0, &mut glyphs);
    assert_no_alloc(|| {
        for i in 0..40 {
            key_flow(&mut screen, &layout);
            screen.set_value_text(Arc::clone(&paths[i % 3]));
            frame(&screen, 1280.0, 720.0, &mut glyphs);
            // The frames of each state of the keys section.
            screen.input(SettingsInput::NextSection, &layout);
            screen.input(SettingsInput::NextSection, &layout);
            screen.input(SettingsInput::Activate, &layout);
            frame(&screen, 1280.0, 720.0, &mut glyphs);
            screen.input(SettingsInput::Back, &layout);
            screen.ask_conflict(3, KeyValue::Key(300), None);
            frame(&screen, 1280.0, 720.0, &mut glyphs);
            screen.input(SettingsInput::Back, &layout);
            frame(&screen, 1280.0, 720.0, &mut glyphs);
            screen.input(SettingsInput::PrevSection, &layout);
            screen.input(SettingsInput::PrevSection, &layout);
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
