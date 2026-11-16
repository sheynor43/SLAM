//! An open dropdown list hides the texts of the rows it lies over, but only those: the
//! rows' names, left of the list, stay.

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::textures::{TextureId, Textures};
use slam_render::{
    BufferDesc, Color, DescError, Device, DeviceError, DrawError, Extent, IndexFormat, Limits,
    Origin, PipelineDesc, RenderTargetDesc, TextureDesc,
};
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::screen::Pose;
use slam_ui::settings_screen::{
    KeyLabels, RowKind, RowLabels, RowSpec, SettingsInput, SettingsLabels, SettingsLayout,
    SettingsScreen, SettingsView,
};
use slam_ui::theme::Theme;

/// Takes texture uploads, so that glyphs get ready; nothing else is reached.
#[derive(Default)]
struct UploadDevice {
    next: u32,
}

impl Device for UploadDevice {
    type Texture = u32;
    type Buffer = ();
    type Pipeline = ();
    type RenderTarget = ();

    fn limits(&self) -> Limits {
        LIMITS
    }
    fn create_texture(&mut self, _: &TextureDesc, _: Option<&[u8]>) -> Result<u32, DeviceError> {
        self.next += 1;
        Ok(self.next)
    }
    fn destroy_texture(&mut self, _: u32) {}
    fn write_texture(&mut self, _: &u32, _: Origin, _: Extent, _: &[u8]) -> Result<(), DescError> {
        Ok(())
    }
    fn create_buffer(&mut self, _: &BufferDesc, _: Option<&[u8]>) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn destroy_buffer(&mut self, _: ()) {
        unimplemented!()
    }
    fn write_buffer(&mut self, _: &(), _: u64, _: &[u8]) -> Result<(), DescError> {
        unimplemented!()
    }
    fn create_pipeline(&mut self, _: &PipelineDesc<'_>) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn destroy_pipeline(&mut self, _: ()) {
        unimplemented!()
    }
    fn create_render_target(&mut self, _: &RenderTargetDesc) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn destroy_render_target(&mut self, _: ()) {
        unimplemented!()
    }
    fn render_target_texture<'a>(&self, _: &'a ()) -> &'a u32 {
        unimplemented!()
    }
    fn begin_frame(&mut self, _: Color) -> Extent {
        unimplemented!()
    }
    fn begin_pass(&mut self, _: Option<&()>, _: Option<Color>) -> Extent {
        unimplemented!()
    }
    fn set_pipeline(&mut self, _: &()) {
        unimplemented!()
    }
    fn set_vertex_buffer(&mut self, _: &(), _: u64) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn set_index_buffer(&mut self, _: &(), _: IndexFormat) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn set_uniform_buffer(&mut self, _: u32, _: &()) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn set_texture(&mut self, _: u32, _: &u32) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn draw(&mut self, _: Range<u32>) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn draw_indexed(&mut self, _: Range<u32>) -> Result<(), DrawError> {
        unimplemented!()
    }
    fn present(&mut self) -> Result<(), DeviceError> {
        unimplemented!()
    }
}

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

fn keys() -> KeyLabels {
    KeyLabels {
        empty: String::new(),
        prompt: String::new(),
        keys: Vec::new(),
        mouse: Default::default(),
        mods: Default::default(),
        conflict_title: String::new(),
        conflict_keep: String::new(),
        conflict_apply: String::new(),
    }
}

/// Text vertices inside `rect`.
fn text_inside(text: &SpriteBatch, rect: &slam_ui::menu::Rect) -> usize {
    text.vertices()
        .iter()
        .filter(|v| rect.contains(v.position[0], v.position[1]))
        .count()
}

#[test]
fn an_open_list_hides_only_the_texts_under_it() {
    let registry = Arc::new(FontRegistry::with_builtin());
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
    let (mut device, mut textures) = (UploadDevice::default(), Textures::with_capacity(16));
    let theme = Theme::default();
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
    let row = |kind| RowSpec {
        section: 0,
        kind,
        restart_required: false,
    };
    let dropdown = row(RowKind::Dropdown {
        options: 3,
        selected: 0,
    });
    let label = |label: &str, unit: &str, options: &[&str]| RowLabels {
        label: label.into(),
        unit: unit.into(),
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        button: String::new(),
        shows_value: false,
        trim_zeros: false,
        zero_text: String::new(),
        hint: String::new(),
    };
    let with_rows = |rows: Vec<RowLabels>| SettingsLabels {
        sections: vec!["Frame".into()],
        rows,
        restart: String::new(),
        search_placeholder: String::new(),
        no_results: String::new(),
        keys: keys(),
    };
    let (width, height) = (1280.0, 720.0);
    let layout = SettingsLayout::new(width, height, &theme);
    // Draws `specs` with the first row's list open; returns the text and the list's rect,
    // with the slots of the rows.
    let mut draw = |specs: &[RowSpec], rows: Vec<RowLabels>| {
        let mut screen = SettingsScreen::new(specs, 1);
        let mut view = SettingsView::new(Arc::clone(&registry), theme, specs, &with_rows(rows));
        screen.input(SettingsInput::Activate, &layout);
        let snapshot = screen.snapshot();
        let slot = usize::from(snapshot.slots[0]);
        let list = layout.dropdown_at(slot, 3).list();
        let (mut shapes, mut text) = (
            ShapeBatch::with_capacity(256),
            SpriteBatch::with_capacity(1024),
        );
        let pose = Pose {
            opacity: 1.0,
            offset: 0.0,
            scale: 1.0,
        };
        // Pushed until the glyphs it asks for are ready.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            shapes.clear();
            text.clear();
            view.push(
                &mut shapes,
                &mut text,
                &mut glyphs,
                width,
                height,
                &snapshot,
                pose,
                white,
                1,
            );
            if glyphs.all_ready() && !text.vertices().is_empty() {
                break;
            }
            glyphs.process(&mut device, &mut textures);
            assert!(Instant::now() < deadline, "glyphs did not get ready");
            std::thread::sleep(Duration::from_millis(1));
        }
        let slots: Vec<usize> = (0..specs.len())
            .map(|i| usize::from(snapshot.slots[i]))
            .collect();
        (text, list, slots)
    };
    let options = label("Rate", "", &["60 Hz", "144 Hz", "Unlimited"]);
    let (alone, list, _) = draw(&[dropdown], vec![options.clone()]);
    let (beside, list_beside, slots) = draw(
        &[
            dropdown,
            row(RowKind::Slider {
                min: 0.0,
                max: 100.0,
                step: 1.0,
                value: 30.0,
            }),
            row(RowKind::Toggle { value: true }),
        ],
        vec![
            options,
            label("Master", "%", &[]),
            label("Animations", "", &[]),
        ],
    );
    assert_eq!(list, list_beside);
    // The rows under the list are under it.
    for slot in &slots[1..] {
        assert!(layout.covered_by_list(slots[0], 3, *slot), "slot {slot}");
    }
    // No text of theirs shows through the list: only its options are there.
    assert!(text_inside(&alone, &list) > 0);
    assert_eq!(text_inside(&beside, &list), text_inside(&alone, &list));
    // Their names, left of the list, stay.
    for slot in &slots[1..] {
        assert!(
            text_inside(&beside, &layout.label(*slot)) > 0,
            "slot {slot}"
        );
    }
}
