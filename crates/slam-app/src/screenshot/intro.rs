//! Screenshots of the startup intro over the menu (ADR-0114) at fixed times since its
//! start, drawn as the shell draws them, on the same headless OpenGL as the plays'.

use std::sync::Arc;
use std::time::{Duration, Instant};

use slam_engine::gl::SdlGlSurface;
use slam_input::{InputWindow, PriorityRequest};
use slam_render::atlas::{Atlas, AtlasRegion};
use slam_render::gl::{GlDebug, GlDevice};
use slam_render::shape::{SHAPE_LAYOUT, ShapeBatch};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::{Color, Device, Extent, TextureFormat, shaders};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::i18n::Localizer;
use slam_ui::intro::{IntroSnapshot, IntroView};
use slam_ui::menu::{MenuLabels, MenuSnapshot, MenuView};
use slam_ui::screen::Pose;
use slam_ui::theme::Theme;

use super::{Frame, LOAD_TIMEOUT, REFERENCE_RENDERER};

pub(crate) type Gl = GlDevice<SdlGlSurface>;

/// Draws the intro over the menu at each of `times_ms` since its start, on a
/// `size` window, in English. Opens SDL video: one call at a time in a process, on its
/// main thread. `any_renderer` as in [`super::Shots`].
pub fn render_intro(
    times_ms: &[u64],
    size: (u32, u32),
    any_renderer: bool,
) -> Result<Vec<Frame>, String> {
    let times = times_ms.to_vec();
    let extent = Extent::new(size.0, size.1);
    with_headless_gl(size, move |device| {
        draw_on(device, &times, extent, any_renderer)
    })
}

/// Runs `work` on the draw thread with an OpenGL device of a headless window of `size`.
/// Opens SDL video: one call at a time in a process, on its main thread.
pub(crate) fn with_headless_gl<T: Send + 'static>(
    size: (u32, u32),
    work: impl FnOnce(&mut Gl) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (width, height) = size;
    let to_i32 = |v: u32| i32::try_from(v).map_err(|_| format!("size {v} is too large"));
    let mut window =
        InputWindow::new_headless_opengl("SLAM screenshot", to_i32(width)?, to_i32(height)?)
            .map_err(|e| e.to_string())?;
    let context = window
        .create_gl_context_with(PriorityRequest::Default)
        .map_err(|e| e.to_string())?;
    let (result, surface) = std::thread::spawn(move || {
        let mut device = match GlDevice::new(SdlGlSurface(context), GlDebug::Off) {
            Ok(device) => device,
            Err(e) => return (Err(format!("OpenGL: {e}")), None),
        };
        let result = work(&mut device);
        match device.into_surface() {
            Ok(surface) => (result, Some(surface)),
            Err(e) => (result.and(Err(format!("releasing the context: {e}"))), None),
        }
    })
    .join()
    .map_err(|_| "the draw thread panicked".to_owned())?;
    drop(surface);
    drop(window);
    result
}

/// The GPU side: textures and the three sprite renderers the shell draws with.
pub(crate) struct Gpu {
    pub(crate) textures: Textures<<Gl as Device>::Texture>,
    pub(crate) white: AtlasRegion,
    pub(crate) shapes: SpriteRenderer<Gl>,
    pub(crate) text: SpriteRenderer<Gl>,
    pub(crate) outlines: SpriteRenderer<Gl>,
}

impl Gpu {
    pub(crate) fn new(device: &mut Gl) -> Result<Gpu, String> {
        let mut textures = Textures::with_capacity(8);
        let mut atlas = Atlas::new(Extent::new(64, 64), TextureFormat::Rgba8);
        let white = atlas
            .add(device, &mut textures, Extent::new(8, 8), &[255; 8 * 8 * 4])
            .map_err(|e| e.to_string())?;
        let renderer = |device: &mut Gl, shader| {
            SpriteRenderer::with_shader(device, DEFAULT_SLOTS, shader).map_err(|e| e.to_string())
        };
        Ok(Gpu {
            textures,
            white,
            shapes: SpriteRenderer::with_layout(
                device,
                DEFAULT_SLOTS,
                &shaders::UI_SHAPE,
                SHAPE_LAYOUT,
            )
            .map_err(|e| e.to_string())?,
            text: renderer(device, &shaders::MSDF)?,
            outlines: renderer(device, &shaders::MSDF_OUTLINE)?,
        })
    }

    pub(crate) fn destroy(mut self, device: &mut Gl) {
        self.shapes.destroy(device);
        self.text.destroy(device);
        self.outlines.destroy(device);
        for texture in self.textures.drain() {
            device.destroy_texture(texture);
        }
    }
}

/// The views and their batches.
struct Views {
    glyphs: GlyphCache,
    menu: MenuView,
    intro: IntroView,
    shapes: ShapeBatch,
    text: SpriteBatch,
    outlines: SpriteBatch,
}

fn draw_on(
    device: &mut Gl,
    times: &[u64],
    size: Extent,
    any_renderer: bool,
) -> Result<Vec<Frame>, String> {
    let driver = device.driver_info();
    if !any_renderer && !driver.contains(REFERENCE_RENDERER) {
        return Err(format!(
            "the references are drawn on {REFERENCE_RENDERER}, not {driver}"
        ));
    }
    let theme = Theme::default();
    let registry = Arc::new(FontRegistry::with_builtin());
    let localizer = Localizer::builtin().map_err(|e| e.to_string())?;
    let text = |key: &str| localizer.format(key, None).into_owned();
    let labels = MenuLabels {
        title: text("menu-title"),
        buttons: [
            text("menu-play-button"),
            text("menu-settings-button"),
            text("menu-exit-button"),
        ],
    };
    let mut views = Views {
        glyphs: glyph_atlas(
            Arc::clone(&registry),
            &GlyphAtlasConfig::new(device.limits()),
        )
        .map_err(|e| e.to_string())?,
        menu: MenuView::new(Arc::clone(&registry), theme, &labels),
        intro: IntroView::new(registry, theme, &labels.title),
        shapes: ShapeBatch::with_capacity(64),
        text: SpriteBatch::with_capacity(64),
        outlines: SpriteBatch::with_capacity(64),
    };
    let mut gpu = Gpu::new(device)?;
    let frames = times
        .iter()
        .map(|&ms| {
            let snapshot = IntroSnapshot {
                elapsed_ns: ms * 1_000_000,
                exit: 0.0,
                skipped: false,
            };
            settle(
                device,
                &mut gpu,
                &mut views,
                |views| &mut views.glyphs,
                theme.background,
                size,
                |device, gpu, views| draw_frame(device, gpu, views, &snapshot, size),
            )
            .map_err(|e| format!("{ms} ms: {e}"))
        })
        .collect();
    gpu.destroy(device);
    frames
}

/// Draws a frame with `draw` until every glyph of `glyphs(state)` is in and two frames in
/// a row are the same, and gives that frame.
pub(crate) fn settle<S>(
    device: &mut Gl,
    gpu: &mut Gpu,
    state: &mut S,
    glyphs: fn(&mut S) -> &mut GlyphCache,
    background: Color,
    size: Extent,
    mut draw: impl FnMut(&mut Gl, &mut Gpu, &mut S) -> Result<(), String>,
) -> Result<Frame, String> {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    let mut last: Option<Vec<u8>> = None;
    loop {
        let drawn = device.begin_frame(background);
        if drawn != size {
            return Err(format!("drew {drawn:?}, not {size:?}"));
        }
        glyphs(state).process(device, &mut gpu.textures);
        let loaded = glyphs(state).all_ready();
        draw(device, gpu, state)?;
        let pixels = loaded.then(|| device.read_frame(size));
        device.present().map_err(|e| e.to_string())?;
        if let Some(pixels) = pixels {
            if last.as_ref() == Some(&pixels) {
                return Ok(Frame {
                    width: size.width,
                    height: size.height,
                    pixels,
                });
            }
            last = Some(pixels);
        }
        if Instant::now() > deadline {
            return Err("glyphs did not load or frames keep changing".to_owned());
        }
        if !loaded {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Uploads and draws one layer: its `shapes`, then its `text`.
pub(crate) fn draw_layer(
    device: &mut Gl,
    gpu: &mut Gpu,
    shapes: &ShapeBatch,
    text: &SpriteBatch,
    size: Extent,
) -> Result<(), String> {
    for renderer in [&mut gpu.shapes, &mut gpu.text, &mut gpu.outlines] {
        renderer.begin_frame();
    }
    let projection = Projection::pixels(size);
    let textures = &gpu.textures;
    let texture = |id| textures.get(id);
    let err = |e: slam_render::sprite::SpriteError| e.to_string();
    let uploaded = gpu
        .shapes
        .upload(device, shapes, &projection, texture)
        .map_err(err)?;
    gpu.shapes
        .draw_range(device, &uploaded, shapes, 0..shapes.len() as u32, texture)
        .map_err(err)?;
    let uploaded = gpu
        .text
        .upload(device, text, &projection, texture)
        .map_err(err)?;
    gpu.text
        .draw_range(device, &uploaded, text, 0..text.len() as u32, texture)
        .map(drop)
        .map_err(err)
}

/// The menu, then the intro over it, as the shell's frame draws them.
fn draw_frame(
    device: &mut Gl,
    gpu: &mut Gpu,
    views: &mut Views,
    snapshot: &IntroSnapshot,
    size: Extent,
) -> Result<(), String> {
    let (width, height) = (size.width as f32, size.height as f32);
    views.shapes.clear();
    views.text.clear();
    views.outlines.clear();
    views.menu.set_title_hidden(snapshot.hides_menu_title());
    views.menu.push(
        &mut views.shapes,
        &mut views.text,
        &mut views.glyphs,
        width,
        height,
        &MenuSnapshot { selected: 0 },
        Pose::SHOWN,
        gpu.white,
    );
    let menu = (views.shapes.len() as u32, views.text.len() as u32);
    views.intro.push(
        &mut views.shapes,
        &mut views.text,
        &mut views.outlines,
        &mut views.glyphs,
        width,
        height,
        snapshot,
        gpu.white,
    );
    for renderer in [&mut gpu.shapes, &mut gpu.text, &mut gpu.outlines] {
        renderer.begin_frame();
    }
    let projection = Projection::pixels(size);
    let textures = &gpu.textures;
    let texture = |id| textures.get(id);
    let err = |e: slam_render::sprite::SpriteError| e.to_string();
    let shapes = gpu
        .shapes
        .upload(device, &views.shapes, &projection, texture)
        .map_err(err)?;
    let text = gpu
        .text
        .upload(device, &views.text, &projection, texture)
        .map_err(err)?;
    // Each layer's shapes, then its text: the menu, then the intro.
    let ranges = [
        (0..menu.0, 0..menu.1),
        (
            menu.0..views.shapes.len() as u32,
            menu.1..views.text.len() as u32,
        ),
    ];
    for (shape_range, text_range) in ranges {
        gpu.shapes
            .draw_range(device, &shapes, &views.shapes, shape_range, texture)
            .map_err(err)?;
        gpu.text
            .draw_range(device, &text, &views.text, text_range, texture)
            .map_err(err)?;
    }
    if !views.outlines.is_empty() {
        let outlines = gpu
            .outlines
            .upload(device, &views.outlines, &projection, texture)
            .map_err(err)?;
        gpu.outlines
            .draw_range(
                device,
                &outlines,
                &views.outlines,
                0..views.outlines.len() as u32,
                texture,
            )
            .map_err(err)?;
    }
    Ok(())
}
