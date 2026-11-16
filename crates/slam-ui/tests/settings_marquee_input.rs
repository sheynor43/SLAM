//! Which marquee takes a sideways wheel on the settings view: an open dropdown list takes
//! it over what lies under the list, and a row that is no longer drawn takes none.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::marquee::{Marquee, MarqueeParams};
use slam_ui::screen::Pose;
use slam_ui::settings_screen::{
    RowKind, RowLabels, RowSpec, SettingsInput, SettingsLabels, SettingsLayout, SettingsScreen,
    SettingsView,
};
use slam_ui::theme::Theme;

const LONG: &str = "A text far too long for any column of the settings panel, so that it has to ride to show its end, and then some more words to be sure of it at every size";

struct Rig {
    view: SettingsView,
    screen: SettingsScreen,
    glyphs: GlyphCache,
    shapes: ShapeBatch,
    text: SpriteBatch,
    layout: SettingsLayout,
    theme: Theme,
    now: u64,
}

fn rig() -> Rig {
    let registry = Arc::new(FontRegistry::with_builtin());
    let limits = Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits)).unwrap();
    let theme = Theme::default();
    let spec = |kind| RowSpec {
        section: 0,
        kind,
        restart_required: false,
    };
    let specs = [
        spec(RowKind::Dropdown {
            options: 3,
            selected: 0,
        }),
        spec(RowKind::Toggle { value: false }),
        spec(RowKind::Toggle { value: false }),
    ];
    let row = |label: &str, hint: &str, options: &[&str]| RowLabels {
        label: label.into(),
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        hint: hint.into(),
        ..RowLabels::default()
    };
    let labels = SettingsLabels {
        sections: vec!["One".into()],
        rows: vec![
            row("Choice", "", &[LONG, LONG, LONG]),
            row(LONG, LONG, &[]),
            row("Other", "", &[]),
        ],
        ..SettingsLabels::default()
    };
    let mut screen = SettingsScreen::new(&specs, 1);
    screen.set_hint(1, true);
    let mut view = SettingsView::new(registry, theme, &specs, &labels);
    view.lay_out_for(1280.0, 720.0);
    Rig {
        view,
        screen,
        glyphs,
        shapes: ShapeBatch::with_capacity(256),
        text: SpriteBatch::with_capacity(4096),
        layout: SettingsLayout::new(1280.0, 720.0, &theme),
        theme,
        now: 0,
    }
}

impl Rig {
    /// Draws a frame 50 ms after the last.
    fn frame(&mut self) {
        self.now += 50_000_000;
        self.shapes.clear();
        self.text.clear();
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
        self.view.push(
            &mut self.shapes,
            &mut self.text,
            &mut self.glyphs,
            1280.0,
            720.0,
            &self.screen.snapshot(),
            Pose::SHOWN,
            white,
            self.now,
        );
    }

    /// Where the marquee `ride` shows at the time of the last frame.
    fn offset(&self, ride: &Marquee) -> f32 {
        let params = MarqueeParams::new(&self.theme, self.layout.scale);
        let mut ride = *ride;
        ride.offset(self.now, &params)
    }

    /// Moves the pointer to `(x, y)`, turns the wheel by one notch and draws two frames.
    fn wheel_at(&mut self, x: f32, y: f32) {
        self.screen
            .input(SettingsInput::PointerMove { x, y }, &self.layout);
        self.screen.scroll_text(1.0);
        self.frame();
        self.frame();
    }
}

fn centre(rect: &slam_ui::menu::Rect) -> (f32, f32) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

#[test]
fn an_open_list_takes_the_sideways_wheel_over_the_text_under_it() {
    let mut rig = rig();
    rig.frame();
    // Where row 1's hint rides, before the list opens over it.
    let hint = rig.view.hint_ride(1, 0).unwrap().window();
    assert!(rig.view.hint_ride(1, 0).unwrap().rides());
    let (x, y) = centre(&hint);
    // Open the list of row 0, which covers the hint of row 1.
    rig.screen.input(SettingsInput::Activate, &rig.layout);
    let list = rig.layout.dropdown_at(rig.screen.slot(0), 3);
    let option = list.option_at(x, y).expect("the list covers the hint");
    rig.frame();
    rig.frame();
    rig.wheel_at(x, y);
    let rides = rig.view.dropdown_rides(0).unwrap();
    // Rides 0 is the box's, then one per option.
    let moved = rig.offset(&rides[1 + option]);
    assert!(moved > 0.0, "the option under the pointer did not move");
    for (i, ride) in rides.iter().enumerate() {
        if i != 1 + option {
            assert_eq!(rig.offset(ride), 0.0, "ride {i} moved");
        }
    }
    assert_eq!(rig.offset(rig.view.hint_ride(1, 0).unwrap()), 0.0);
    assert_eq!(rig.offset(rig.view.label_ride(1).unwrap()), 0.0);
}

#[test]
fn a_dropdown_that_is_no_longer_drawn_takes_no_sideways_wheel() {
    let mut rig = rig();
    rig.frame();
    let bounds = rig.layout.dropdown_at(rig.screen.slot(0), 3).bounds;
    let (x, y) = centre(&bounds);
    // Drawn, its box takes the wheel.
    rig.wheel_at(x, y);
    let header = rig.view.dropdown_rides(0).unwrap()[0];
    assert!(header.rides());
    assert!(rig.offset(&header) > 0.0);
    // Hidden, it is not drawn again: its old place takes nothing.
    let mut rig = self::rig();
    rig.frame();
    rig.screen.set_hidden(0, true);
    rig.frame();
    rig.frame();
    rig.wheel_at(x, y);
    let header = rig.view.dropdown_rides(0).unwrap()[0];
    assert_eq!(rig.offset(&header), 0.0);
}
