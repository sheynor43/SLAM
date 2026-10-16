//! The window with the screens (ADR-0044): the main menu and a song select placeholder.
//! Opened by `slam-app` without arguments. Console output is not localised; UI text is.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use slam_engine::clock::now_ns;
use slam_engine::gl::SdlGlSurface;
use slam_engine::{Draw, Engine, EngineConfig, FrameInfo, TickInfo, Update};
use slam_input::{
    InputEvent, InputKind, InputWindow, MouseButton, TickMapper, event_ring, sdl_ticks_ns,
};
use slam_render::atlas::{Atlas, AtlasRegion};
use slam_render::gl::{GlDebug, GlDevice, GlTexture};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::{Device, Extent, TextureFormat, shaders};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::i18n::Localizer;
use slam_ui::menu::{
    MainMenu, MenuAction, MenuInput, MenuLabels, MenuLayout, MenuSnapshot, MenuView,
    PlaceholderView,
};
use slam_ui::screen::{MAX_LAYERS, Nav, Navigated, Pose, ScreenStack};
use slam_ui::theme::Theme;

use crate::play::{device_limits, pack_size};

pub const USAGE: &str = "usage: slam-app [bench|click|click-analyze|play|i18n] ...\n\
                         without arguments, opens the window with the main menu";

const WINDOW: (i32, i32) = (1280, 720);
const UPDATE_HZ: f64 = 1000.0;
/// Textures of the draw thread: the white square and the glyph pages.
const MAX_TEXTURES: usize = 16;
/// Rectangles of one frame: the buttons of up to two layers.
const MAX_SHAPES: usize = 16;
/// Glyph sprites of one frame: the texts of up to two layers.
const MAX_GLYPH_SPRITES: usize = 512;
/// How long the draw thread waits for the glyphs before the first frame.
const GLYPH_WARM_TIMEOUT: Duration = Duration::from_secs(1);

// USB HID usage ids, as `SDL_Scancode`.
const KEY_ENTER: u16 = 40;
const KEY_ESCAPE: u16 = 41;
const KEY_KEYPAD_ENTER: u16 = 88;
const KEY_DOWN: u16 = 81;
const KEY_UP: u16 = 82;

/// The application's screens, as the update thread holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppScreen {
    Menu(MainMenu),
    SongSelect,
}

/// What the draw thread needs to draw a screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScreenView {
    Menu(MenuSnapshot),
    SongSelect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerSnapshot {
    pub view: ScreenView,
    pub pose: Pose,
}

/// The visible screens, bottom first.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShellSnapshot {
    pub layers: [Option<LayerSnapshot>; MAX_LAYERS],
}

/// The update logic without the window: input to navigation to snapshot.
pub struct ShellState {
    stack: ScreenStack<AppScreen>,
    theme: Theme,
    /// Layout in pixels, the unit the draw thread lays out in.
    layout: MenuLayout,
    /// Last cursor position in pixels.
    cursor: [f32; 2],
    /// Input arrives in window points, layout is in pixels.
    pixels_per_point: f32,
}

impl ShellState {
    pub fn new(theme: Theme, width: f32, height: f32, pixels_per_point: f32) -> ShellState {
        ShellState {
            stack: ScreenStack::new(AppScreen::Menu(MainMenu::new())),
            layout: MenuLayout::new(width, height, &theme),
            theme,
            cursor: [0.0, 0.0],
            pixels_per_point,
        }
    }

    pub fn stack(&self) -> &ScreenStack<AppScreen> {
        &self.stack
    }

    /// Lays the screens out for a window of `width` × `height` pixels.
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.layout = MenuLayout::new(width, height, &self.theme);
    }

    fn menu_input(&mut self, kind: InputKind) -> Option<MenuInput> {
        Some(match kind {
            InputKind::Key {
                scancode,
                down: true,
            } => match scancode {
                KEY_UP => MenuInput::Up,
                KEY_DOWN => MenuInput::Down,
                KEY_ENTER | KEY_KEYPAD_ENTER => MenuInput::Activate,
                KEY_ESCAPE => MenuInput::Back,
                _ => return None,
            },
            InputKind::MouseButton {
                button: MouseButton::X1,
                down: true,
            } => MenuInput::Back,
            InputKind::MouseButton {
                button: MouseButton::Left,
                down: true,
            } => MenuInput::PointerPress {
                x: self.cursor[0],
                y: self.cursor[1],
            },
            InputKind::MouseMove { x, y } => {
                self.cursor = [x * self.pixels_per_point, y * self.pixels_per_point];
                MenuInput::PointerMove {
                    x: self.cursor[0],
                    y: self.cursor[1],
                }
            }
            _ => return None,
        })
    }

    /// Gives `events` to the top screen and applies what it asks for at `now_ns`. Stops at
    /// the first exit.
    pub fn handle(&mut self, events: &[InputEvent], now_ns: u64) -> Navigated {
        for event in events {
            let Some(input) = self.menu_input(event.kind) else {
                continue;
            };
            let nav = match self.stack.top_mut() {
                AppScreen::Menu(menu) => match menu.input(input, &self.layout) {
                    Some(MenuAction::Play) => Some(Nav::Push(AppScreen::SongSelect)),
                    Some(MenuAction::Exit) => Some(Nav::Exit),
                    None => None,
                },
                AppScreen::SongSelect => (input == MenuInput::Back).then_some(Nav::Pop),
            };
            if let Some(nav) = nav
                && self.stack.navigate(nav, now_ns) == Navigated::Exit
            {
                return Navigated::Exit;
            }
        }
        Navigated::Stay
    }

    /// One tick: ends a finished transition, handles `events`, rewrites every layer of
    /// `snapshot`. Allocates only when `events` navigate.
    pub fn tick(
        &mut self,
        now_ns: u64,
        events: &[InputEvent],
        snapshot: &mut ShellSnapshot,
    ) -> Navigated {
        self.stack.tick(now_ns);
        let navigated = self.handle(events, now_ns);
        for (slot, layer) in snapshot.layers.iter_mut().zip(self.stack.layers(now_ns)) {
            *slot = layer.map(|(screen, pose)| LayerSnapshot {
                view: match screen {
                    AppScreen::Menu(menu) => ScreenView::Menu(menu.snapshot()),
                    AppScreen::SongSelect => ScreenView::SongSelect,
                },
                pose,
            });
        }
        navigated
    }
}

/// The update thread's side.
struct Shell {
    state: ShellState,
    /// Surface size in pixels the draw thread last drew at, see [`pack_size`].
    surface: Arc<AtomicU64>,
    layout_size: u64,
    /// Set when the menu quits; the window closes then.
    exited: Arc<AtomicBool>,
}

impl Update for Shell {
    type Snapshot = ShellSnapshot;

    fn tick(&mut self, info: &TickInfo, events: &[InputEvent], snapshot: &mut ShellSnapshot) {
        let size = self.surface.load(Ordering::Relaxed);
        // A minimised window may report 0×0: keep the last layout.
        if size != self.layout_size && size >> 32 != 0 && size & 0xffff_ffff != 0 {
            self.layout_size = size;
            self.state
                .set_size((size >> 32) as f32, (size & 0xffff_ffff) as f32);
        }
        // Between the exit and the window closing, input no longer navigates.
        if self.exited.load(Ordering::Relaxed) {
            return;
        }
        if self.state.tick(info.now_ns, events, snapshot) == Navigated::Exit {
            self.exited.store(true, Ordering::Relaxed);
        }
    }
}

struct Resources {
    device: GlDevice<SdlGlSurface>,
    textures: Textures<GlTexture>,
    renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
    /// Draws MSDF glyphs.
    text_renderer: SpriteRenderer<GlDevice<SdlGlSurface>>,
    white: AtlasRegion,
}

/// The draw thread's side. The device and its resources exist between `start` and `stop`.
struct View {
    surface: Option<SdlGlSurface>,
    resources: Option<Resources>,
    theme: Theme,
    glyphs: GlyphCache,
    menu: MenuView,
    placeholder: PlaceholderView,
    shapes: SpriteBatch,
    text: SpriteBatch,
    /// Read by the update thread for the layout.
    surface_size: Arc<AtomicU64>,
    _quit: crate::bench::QuitOnDrop,
}

impl Draw<ShellSnapshot> for View {
    fn start(&mut self) {
        // The window thread made the context and released it (ADR-0018).
        let surface = self.surface.take().expect("surface is set before start");
        // The same context made a device in `device_limits` already; failing now is a
        // driver fault this developer shell does not recover from.
        let mut device = GlDevice::new(surface, GlDebug::for_build()).expect("GL device");
        let renderer = SpriteRenderer::new(&mut device, DEFAULT_SLOTS).expect("renderer");
        let text_renderer = SpriteRenderer::with_shader(&mut device, DEFAULT_SLOTS, &shaders::MSDF)
            .expect("text renderer");
        let mut textures = Textures::with_capacity(MAX_TEXTURES);
        let mut atlas = Atlas::new(Extent::new(64, 64), TextureFormat::Rgba8);
        let white = atlas
            .add(
                &mut device,
                &mut textures,
                Extent::new(8, 8),
                &[255; 8 * 8 * 4],
            )
            .expect("white square");
        // Glyphs are made before the first frame: a glyph takes several frames to appear.
        let deadline = Instant::now() + GLYPH_WARM_TIMEOUT;
        while self.request_glyphs() > 0 && Instant::now() < deadline {
            self.glyphs.process(&mut device, &mut textures);
            std::thread::sleep(Duration::from_millis(1));
        }
        self.resources = Some(Resources {
            device,
            textures,
            renderer,
            text_renderer,
            white,
        });
    }

    fn frame(&mut self, _: &FrameInfo, snapshot: &ShellSnapshot) {
        let r = self.resources.as_mut().expect("started");
        let size = r.device.begin_frame(self.theme.background);
        self.surface_size
            .store(pack_size(size.width, size.height), Ordering::Relaxed);
        self.glyphs.process(&mut r.device, &mut r.textures);
        let (width, height) = (size.width as f32, size.height as f32);
        self.shapes.clear();
        self.text.clear();
        // Where each layer's sprites end: layers are drawn one after another, shapes then
        // text, so a screen coming in covers the text of the one leaving.
        let mut ends = [(0_u32, 0_u32); MAX_LAYERS];
        let mut layers = 0;
        for layer in snapshot.layers.iter().flatten() {
            match &layer.view {
                ScreenView::Menu(menu) => self.menu.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    menu,
                    layer.pose,
                    r.white,
                ),
                ScreenView::SongSelect => self.placeholder.push(
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    layer.pose,
                ),
            }
            ends[layers] = (self.shapes.len() as u32, self.text.len() as u32);
            layers += 1;
        }
        r.renderer.begin_frame();
        r.text_renderer.begin_frame();
        let gpu_textures = &r.textures;
        let projection = Projection::pixels(size);
        let texture = |id| gpu_textures.get(id);
        // A failed draw or present loses one frame; the next one tries again.
        let shapes = r
            .renderer
            .upload(&mut r.device, &self.shapes, &projection, texture);
        let text = r
            .text_renderer
            .upload(&mut r.device, &self.text, &projection, texture);
        let mut from = (0, 0);
        for &(shapes_end, text_end) in &ends[..layers] {
            if let Ok(uploaded) = &shapes {
                let _ = r.renderer.draw_range(
                    &mut r.device,
                    uploaded,
                    &self.shapes,
                    from.0..shapes_end,
                    texture,
                );
            }
            if let Ok(uploaded) = &text {
                let _ = r.text_renderer.draw_range(
                    &mut r.device,
                    uploaded,
                    &self.text,
                    from.1..text_end,
                    texture,
                );
            }
            from = (shapes_end, text_end);
        }
        let _ = r.device.present();
    }

    fn stop(&mut self) {
        let Resources {
            mut device,
            mut textures,
            renderer,
            text_renderer,
            ..
        } = self.resources.take().expect("started");
        renderer.destroy(&mut device);
        text_renderer.destroy(&mut device);
        for texture in textures.drain() {
            device.destroy_texture(texture);
        }
        self.surface = Some(device.into_surface().expect("release GL context"));
    }
}

impl View {
    fn request_glyphs(&mut self) -> usize {
        self.menu.request_glyphs(&mut self.glyphs)
            + self.placeholder.request_glyphs(&mut self.glyphs)
    }
}

/// Opens the window with the main menu and runs until it is closed.
pub fn run() -> Result<(), String> {
    let theme = Theme::default();
    let mut window =
        InputWindow::new_opengl("SLAM", WINDOW.0, WINDOW.1).map_err(|e| e.to_string())?;
    let surface = SdlGlSurface(window.create_gl_context().map_err(|e| e.to_string())?);
    let (pixels_w, pixels_h) = surface.0.size_in_pixels();
    let pixels_per_point = if pixels_w > 0 {
        pixels_w as f32 / WINDOW.0 as f32
    } else {
        1.0
    };
    let initial_size = pack_size(pixels_w, pixels_h);
    let surface_size = Arc::new(AtomicU64::new(initial_size));
    let (surface, limits) = device_limits(surface)?;
    let registry = Arc::new(FontRegistry::with_builtin());
    let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits))
        .map_err(|e| e.to_string())?;
    // English until the language setting exists (#98).
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
    let menu = MenuView::new(Arc::clone(&registry), theme, &labels);
    let placeholder = PlaceholderView::new(registry, theme, &text("song-select-placeholder"));

    let exited = Arc::new(AtomicBool::new(false));
    let shell = Shell {
        state: ShellState::new(theme, pixels_w as f32, pixels_h as f32, pixels_per_point),
        surface: Arc::clone(&surface_size),
        layout_size: initial_size,
        exited: Arc::clone(&exited),
    };
    let draw = View {
        surface: Some(surface),
        resources: None,
        theme,
        glyphs,
        menu,
        placeholder,
        shapes: SpriteBatch::with_capacity(MAX_SHAPES),
        text: SpriteBatch::with_capacity(MAX_GLYPH_SPRITES),
        surface_size,
        _quit: crate::bench::QuitOnDrop(window.quit_handle()),
    };
    let (mut sink, source) = event_ring(1024);
    let engine = Engine::spawn(
        EngineConfig {
            update_hz: UPDATE_HZ,
            ..EngineConfig::default()
        },
        shell,
        draw,
        ShellSnapshot::default(),
        source,
    )
    .map_err(|e| e.to_string())?;
    sink.set_notify(engine.waker().as_notify());

    // The update thread must not take the window's lock: a watcher closes the window
    // once the menu quits.
    let closed = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (quit, exited, closed) = (
            window.quit_handle(),
            Arc::clone(&exited),
            Arc::clone(&closed),
        );
        std::thread::spawn(move || {
            while !closed.load(Ordering::Relaxed) {
                // A full event queue refuses the request: try again.
                if exited.load(Ordering::Relaxed) && quit.request() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let mut mapper = TickMapper::new(now_ns, sdl_ticks_ns);
    let ran = window.run(&mut sink, &mut mapper);
    closed.store(true, Ordering::Relaxed);
    let _ = watcher.join();

    let slam_engine::Stopped { draw, .. } = engine.shutdown();
    // The GL context goes before the window.
    drop(draw);
    drop(window);
    ran.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
