//! The widgets' steady state does not touch the allocator: input on the update side and
//! `View::push` on the draw side once warm, including a slider whose text changes.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::menu::Rect;
use slam_ui::theme::Theme;
use slam_ui::widgets::{
    Dropdown, DropdownInput, DropdownLayout, DropdownView, KeyBind, KeyBindInput, KeyBindLayout,
    KeyBindView, Slider, SliderInput, SliderLayout, SliderView, Toggle, ToggleInput, ToggleLayout,
    ToggleView,
};

slam_testkit::install_counting_allocator!();

const BOUNDS: Rect = Rect {
    x: 100.0,
    y: 100.0,
    width: 400.0,
    height: 40.0,
};

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

#[test]
fn widget_input_does_not_allocate() {
    let theme = Theme::default();
    let mut toggle = Toggle::new(false);
    let mut slider = Slider::new(0.0, 100.0, 0.5, 10.0);
    let mut dropdown = Dropdown::new(3, 0);
    let mut keybind = KeyBind::new();
    let tl = ToggleLayout::new(BOUNDS, &theme, 1.0);
    let sl = SliderLayout::new(BOUNDS, &theme, 1.0);
    let dl = DropdownLayout::new(BOUNDS, 3, 1.0);
    let kl = KeyBindLayout::new(BOUNDS, 1.0);
    assert_no_alloc(|| {
        for input in [
            ToggleInput::Activate,
            ToggleInput::PointerPress { x: 110.0, y: 110.0 },
        ] {
            std::hint::black_box(toggle.input(input, &tl));
        }
        for input in [
            SliderInput::Increase,
            SliderInput::DecreaseCoarse,
            SliderInput::PointerPress { x: 150.0, y: 110.0 },
            SliderInput::PointerMove { x: 300.0, y: 110.0 },
            SliderInput::PointerRelease,
        ] {
            std::hint::black_box(slider.input(input, &sl));
        }
        for input in [
            DropdownInput::Activate,
            DropdownInput::Down,
            DropdownInput::PointerMove { x: 150.0, y: 150.0 },
            DropdownInput::PointerPress { x: 150.0, y: 150.0 },
            DropdownInput::Back,
        ] {
            std::hint::black_box(dropdown.input(input, &dl));
        }
        for input in [
            KeyBindInput::Activate,
            KeyBindInput::Captured(7_u32),
            KeyBindInput::PointerPress { x: 110.0, y: 110.0 },
            KeyBindInput::Back,
        ] {
            std::hint::black_box(keybind.input(input, &kl));
        }
    });
}

#[test]
fn widget_views_push_without_allocating_once_warm() {
    let theme = Theme::default();
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let toggle_view = ToggleView::new(theme);
    let mut slider_view = SliderView::new(Arc::clone(&registry), theme, 2, " ms");
    let mut dropdown_view =
        DropdownView::new(Arc::clone(&registry), theme, &["One", "Two", "Three"]);
    let mut keybind_view = KeyBindView::new(Arc::clone(&registry), theme, "Press a key", "Z");

    let tl = ToggleLayout::new(BOUNDS, &theme, 1.0);
    let sl = SliderLayout::new(BOUNDS, &theme, 1.0);
    let dl = DropdownLayout::new(BOUNDS, 3, 1.0);
    let kl = KeyBindLayout::new(BOUNDS, 1.0);
    let toggle = Toggle::new(true);
    let mut slider = Slider::new(-100.0, 100.0, 0.01, 0.0);
    let mut dropdown = Dropdown::new(3, 1);
    let mut keybind = KeyBind::new();
    slider_view.request_glyphs(&mut glyphs);
    dropdown_view.request_glyphs(&mut glyphs);
    keybind_view.request_glyphs(&mut glyphs);
    let (mut shapes, mut text) = (
        SpriteBatch::with_capacity(256),
        SpriteBatch::with_capacity(1024),
    );

    let mut frame = |slider: &Slider, dropdown: &Dropdown, keybind: &KeyBind, glyphs: &mut _| {
        shapes.clear();
        text.clear();
        toggle_view.push(&mut shapes, &toggle, &tl, 0.5, white());
        slider_view.push(&mut shapes, &mut text, glyphs, slider, &sl, 0.5, white());
        dropdown_view.push(&mut shapes, &mut text, glyphs, dropdown, &dl, 0.5, white());
        keybind_view.push(&mut shapes, &mut text, glyphs, keybind, &kl, 0.5, white());
    };

    // Warm: lay texts out at this scale, show the longest value and request glyphs.
    slider.set(-100.0);
    frame(&slider, &dropdown, &keybind, &mut glyphs);
    dropdown.input(DropdownInput::Activate, &dl);
    keybind.input(KeyBindInput::<u32>::Activate, &kl);
    frame(&slider, &dropdown, &keybind, &mut glyphs);
    frame(&slider, &dropdown, &keybind, &mut glyphs);

    assert_no_alloc(|| {
        for _ in 0..5 {
            frame(&slider, &dropdown, &keybind, &mut glyphs);
        }
        // The slider's text changes every frame.
        for i in 0..50 {
            slider.set(f64::from(i) * 3.37 - 100.0);
            frame(&slider, &dropdown, &keybind, &mut glyphs);
        }
    });
    assert!(shapes.len() > 6);
}
