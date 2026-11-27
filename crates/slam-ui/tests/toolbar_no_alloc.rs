//! The toolbar's steady state does not touch the allocator (ADR-0168): its input on the
//! update side and `ToolbarView::push` on the draw side once warm, with a popup of each
//! widget kind open, tooltips and the clock changing.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::settings_screen::{
    RowKind, RowLabels, RowSpec, SettingsLabels, SettingsView, widget_of,
};
use slam_ui::theme::Theme;
use slam_ui::toolbar::{
    NowPlayingFrame, NowPlayingText, Toolbar, ToolbarLabels, ToolbarLayout, ToolbarView,
};

slam_testkit::install_counting_allocator!();

const MS: u64 = 1_000_000;

fn specs() -> Vec<RowSpec> {
    let row = |section, kind| RowSpec {
        section,
        kind,
        restart_required: false,
    };
    vec![
        row(0, RowKind::Toggle { value: true }),
        row(
            1,
            RowKind::Slider {
                min: 0.0,
                max: 100.0,
                step: 1.0,
                value: 30.0,
            },
        ),
        row(
            2,
            RowKind::Dropdown {
                options: 3,
                selected: 1,
            },
        ),
    ]
}

fn labels() -> SettingsLabels {
    let row = |label: &str, options: &[&str]| RowLabels {
        label: label.into(),
        unit: " %".into(),
        options: options.iter().map(|o| (*o).into()).collect(),
        ..RowLabels::default()
    };
    SettingsLabels {
        sections: ["A", "B", "C"].map(String::from).into(),
        rows: vec![
            row("Menu music", &[]),
            row("Volume", &[]),
            row("Cursor", &["Slam", "Classic", "Off"]),
        ],
        ..SettingsLabels::default()
    }
}

#[test]
fn input_and_drawing_do_not_allocate_once_warm() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let limits = Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits)).unwrap();
    let theme = Theme::default();
    let mut rows = SettingsView::new(Arc::clone(&registry), theme, &specs(), &labels());
    let mut view = ToolbarView::new(
        registry,
        theme,
        &ToolbarLabels {
            settings: "Settings".into(),
            home: "Main menu".into(),
            player: "Now playing".into(),
            nothing_playing: "Nothing is playing".into(),
        },
    );
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
        ShapeBatch::with_capacity(512),
        SpriteBatch::with_capacity(2048),
    );
    let (width, height) = (1280.0, 720.0);
    let layout = ToolbarLayout::new(width, height, &theme, 3);
    let mut bar = Toolbar::new();
    bar.set_pins(&[0, 1, 2]);
    let widgets = [0, 1, 2].map(|i| widget_of(specs()[i].kind));
    let mut now = 0_u64;
    // A title too long for the popup rides as a marquee.
    let words = NowPlayingText {
        key: 1,
        title: "A very long title that does not fit the popup of the player at all".into(),
        artist: "Some artist".into(),
    };
    let mut frame = |bar: &Toolbar, clock: u16, glyphs: &mut _, now: u64| {
        shapes.clear();
        text.clear();
        let mut shown = bar.frame(true, clock);
        shown.now_playing = NowPlayingFrame {
            available: !clock.is_multiple_of(7),
            paused: clock.is_multiple_of(2),
            position_ms: (now / MS) as f32,
            length_ms: 90_000.0,
        };
        view.push(
            &mut shapes,
            &mut text,
            glyphs,
            &mut rows,
            &shown,
            &words,
            Some(white),
            width,
            height,
            now,
            white,
        )
    };
    // Warm: every clock digit, every tooltip and every popup, open list included.
    for clock in 0..24 * 60 {
        frame(&bar, clock, &mut glyphs, now);
        now += MS;
    }
    for (pin, widget) in widgets.iter().enumerate() {
        let [x, y] = layout.pins()[pin].centre();
        bar.pointer(x, y, &layout);
        frame(&bar, 61, &mut glyphs, now);
        bar.show_popup(pin, *widget, now);
        frame(&bar, 61, &mut glyphs, now + 50 * MS);
        let [x, y] = layout.row(pin, 0.0).widget(0).centre();
        bar.press(x, y, &layout);
        frame(&bar, 61, &mut glyphs, now + 300 * MS);
        bar.close_popup();
    }
    // The player's popup, with and without a track, a hovered button, a drag.
    bar.show_player(now);
    for clock in [61, 62, 63, 14] {
        frame(&bar, clock, &mut glyphs, now + 400 * MS);
    }
    let rects = layout.player_rects(0.0);
    let [x, y] = rects.buttons[1].centre();
    bar.pointer(x, y, &layout);
    frame(&bar, 61, &mut glyphs, now + 500 * MS);
    bar.close_player();
    // The popup closes by animation: the frames after it are drawn, then stop being.
    for step in 0..40 {
        frame(&bar, 61, &mut glyphs, now + (520 + step * 10) * MS);
    }
    for target in [layout.settings, layout.home, layout.player] {
        let [x, y] = target.centre();
        bar.pointer(x, y, &layout);
        frame(&bar, 61, &mut glyphs, now);
    }
    assert_no_alloc(|| {
        for k in 0..300_u64 {
            let at = now + k * 16 * MS;
            let pin = (k / 20 % 3) as usize;
            // Over a pin, then its popup opens, is dragged, and goes.
            let [x, y] = layout.pins()[pin].centre();
            bar.pointer(x, y, &layout);
            if k % 20 == 2 {
                bar.show_popup(pin, widgets[pin], at);
            }
            if k % 20 == 5 {
                let widget = layout.row(pin, 0.0).widget(0);
                let (x, y) = (
                    widget.x + widget.width * 0.3,
                    widget.y + widget.height / 2.0,
                );
                bar.press(x, y, &layout);
                bar.pointer(x + 20.0, y, &layout);
                bar.release(&layout);
            }
            if k % 20 == 15 {
                bar.back(&layout);
                bar.back(&layout);
            }
            // The player's popup opens, is dragged along its bar, pressed and closed.
            if k % 20 == 16 {
                bar.show_player(at);
                let rects = layout.player_rects(0.0);
                let y = rects.bar.y + rects.bar.height / 2.0;
                bar.press(rects.bar.x + 30.0, y, &layout);
                bar.pointer(rects.bar.x + 90.0, y, &layout);
                bar.release(&layout);
                let [bx, by] = rects.buttons[(k % 3) as usize].centre();
                bar.pointer(bx, by, &layout);
                bar.press(bx, by, &layout);
            }
            if k % 20 == 19 {
                bar.back(&layout);
            }
            frame(&bar, (k * 13 % 1440) as u16, &mut glyphs, at);
        }
    });
}
