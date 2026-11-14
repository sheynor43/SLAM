//! The cursor of the menus, the pause menu and the results (ADR-0124): which one is drawn,
//! when the OS cursor hides, how the skin's cursor reaches the draw thread, and the draw
//! thread's drawing of it over every layer.
//!
//! Three ends: the update thread's [`SkinCursorLink`] asks for the skin's `cursor` and
//! `cursormiddle` when the menu cursor is the skin's; the `slam-menu-cursor` thread reads the
//! skin and queues the images on its own texture loader; the draw thread's
//! [`MenuCursorView`] draws SLAM's cursor or the skin's at the pointer's last position.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::Thread;
use std::time::Duration;

use slam_input::{PointerWatch, WindowScale};
use slam_play::circles::SkinSprite;
use slam_play::cursor::{CursorSkinConfig, CursorSources, ExpandTimes, rotation};
use slam_play::session::PlayState;
use slam_render::Blend;
use slam_render::atlas::AtlasRegion;
use slam_render::hal::{Color, Device};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::Sprite;
use slam_render::textures::Textures;
use slam_render::upload::{
    TextureHandle, TextureKind, TextureLoaderConfig, TextureRequests, TextureState, TextureUploads,
    texture_loader,
};
use slam_skin::{Skin, SkinChain};
use slam_ui::cursor::{
    CursorStyle, MAX_CURSOR_SHAPES, PressSqueeze, push_slam_cursor, squeeze_for,
};
use slam_ui::theme::Theme;

use crate::settings::Settings;

/// Textures of the cursor's loader: `cursor` and `cursormiddle`, and the same two of the
/// skin before, until the draw thread lets go.
pub const MAX_CURSOR_TEXTURES: u32 = 4;

/// Shapes and sprites one frame of the menu cursor writes at most.
pub const MAX_MENU_CURSOR_SHAPES: usize = MAX_CURSOR_SHAPES;

/// The skin's cursor is drawn as stable draws it: texture pixels at the 768-high
/// reference of the interface.
const MAX_SKIN_CURSOR_SIZE: f32 = 512.0;

/// The gameplay cursor in force: `system` with relative mouse (ADR-0069) is the skin's,
/// as the OS cursor does not follow gameplay's position then.
pub fn game_style(settings: &Settings) -> CursorStyle {
    match settings.gameplay.cursor_style {
        CursorStyle::System if settings.input.high_precision_mouse => CursorStyle::Skin,
        style => style,
    }
}

/// Whether the OS cursor is hidden: while a cursor of our own is shown. `play` is the state
/// of the running play, if any: in the states where gameplay draws its cursor, the gameplay
/// style decides; in the others (paused, failed, results) and outside plays, the menu's.
pub fn os_cursor_hidden(settings: &Settings, play: Option<PlayState>) -> bool {
    let style = match play {
        Some(state) if state.hides_os_cursor() => game_style(settings),
        _ => settings.general.menu_cursor,
    };
    style != CursorStyle::System
}

// ---- Skin images ----

/// An uploaded image of the skin and its density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkinImage {
    pub handle: TextureHandle,
    pub scale: f32,
}

/// The skin's cursor as the draw thread gets it.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct MenuCursorSkin {
    /// Whether the thread answered: until then the images are on their way.
    pub loaded: bool,
    /// `None` after the answer: no skin of the chain has a `cursor`, SLAM's is drawn.
    pub cursor: Option<SkinImage>,
    pub middle: Option<SkinImage>,
    pub config: CursorSkinConfig,
}

/// What the update thread hands the draw thread each tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuCursorFrame {
    /// `general.menu_cursor`.
    pub style: CursorStyle,
    /// Whether gameplay draws its own cursor now, so that this one stays away.
    pub gameplay_cursor: bool,
    pub skin: MenuCursorSkin,
}

impl Default for MenuCursorFrame {
    fn default() -> Self {
        MenuCursorFrame {
            style: CursorStyle::Slam,
            gameplay_cursor: false,
            skin: MenuCursorSkin::default(),
        }
    }
}

/// The files of the skin's cursor: where the chain finds them.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorFiles {
    pub cursor: Option<(PathBuf, f32)>,
    pub middle: Option<(PathBuf, f32)>,
    pub config: CursorSkinConfig,
}

/// The `cursor` of the first skin of `skins` with one, and its `cursormiddle`; none of them
/// when no skin has a `cursor` (SLAM's cursor is drawn then).
pub fn cursor_files(skins: &SkinChain) -> CursorFiles {
    let Some(source) = CursorSources::new(skins).cursor else {
        return CursorFiles {
            cursor: None,
            middle: None,
            config: CursorSkinConfig::new(skins),
        };
    };
    let skin = skins.skin(source);
    let file = |name: &str| skin.texture(name).map(|t| (t.path, t.scale));
    CursorFiles {
        cursor: file("cursor"),
        middle: file("cursormiddle"),
        config: CursorSkinConfig::new(skins),
    }
}

/// The skin chain of the menus: the chosen skin over the default one.
fn menu_chain(user: Option<&Path>) -> Result<SkinChain, String> {
    let default_dir = crate::default_skin::dir()?;
    let default = Skin::load_default(&default_dir).map_err(|e| e.to_string())?;
    let user = user.and_then(|dir| {
        Skin::load(dir)
            .inspect_err(|e| tracing::warn!("skin {}: {e}, using the default skin", dir.display()))
            .ok()
    });
    Ok(SkinChain::new(user, default))
}

// ---- Loader thread ----

const RING: usize = 4;
const PARK: Duration = Duration::from_millis(100);

enum Command {
    Load {
        generation: u32,
        skin: Option<Arc<Path>>,
    },
    Release(TextureHandle),
}

struct Reply {
    generation: u32,
    skin: MenuCursorSkin,
}

fn read_image(requests: &mut TextureRequests, file: Option<(PathBuf, f32)>) -> Option<SkinImage> {
    let (path, scale) = file?;
    let bytes = std::fs::read(&path)
        .inspect_err(|e| tracing::warn!("cursor {}: {e}", path.display()))
        .ok()?;
    let handle = requests
        .load(bytes, TextureKind::Standalone)
        .inspect_err(|e| tracing::warn!("cursor {}: {}", path.display(), e.reason))
        .ok()?;
    Some(SkinImage { handle, scale })
}

fn load_skin(requests: &mut TextureRequests, user: Option<&Path>) -> MenuCursorSkin {
    let files = match menu_chain(user) {
        Ok(chain) => cursor_files(&chain),
        Err(e) => {
            tracing::warn!("menu cursor: {e}");
            return MenuCursorSkin {
                loaded: true,
                ..MenuCursorSkin::default()
            };
        }
    };
    MenuCursorSkin {
        loaded: true,
        cursor: read_image(requests, files.cursor),
        middle: read_image(requests, files.middle),
        config: files.config,
    }
}

fn run(
    mut requests: TextureRequests,
    mut commands: rtrb::Consumer<Command>,
    mut replies: rtrb::Producer<Reply>,
) {
    loop {
        let mut load = None;
        while let Ok(command) = commands.pop() {
            match command {
                Command::Release(handle) => {
                    // A full queue loses the release: the texture stays until the exit.
                    let _ = requests.release(handle);
                }
                Command::Load { generation, skin } => load = Some((generation, skin)),
            }
        }
        if let Some((generation, skin)) = load {
            let skin = load_skin(&mut requests, skin.as_deref());
            if let Err(rtrb::PushError::Full(reply)) = replies.push(Reply { generation, skin }) {
                for image in [reply.skin.cursor, reply.skin.middle].into_iter().flatten() {
                    let _ = requests.release(image.handle);
                }
            }
        }
        if commands.is_abandoned() {
            return;
        }
        std::thread::park_timeout(PARK);
    }
}

/// The update thread's end. Neither blocks nor allocates (a skin folder's path is shared).
pub struct SkinCursorLink {
    commands: rtrb::Producer<Command>,
    replies: rtrb::Consumer<Reply>,
    thread: Thread,
    /// The newest generation asked for; older answers are released unseen.
    generation: u32,
    /// The skin folder asked for last; `None` outer: nothing asked.
    asked: Option<Option<Arc<Path>>>,
    current: MenuCursorSkin,
    /// Releases the full command ring did not take, retried on later ticks.
    unsent: [Option<TextureHandle>; 8],
}

/// Starts the loader thread `slam-menu-cursor` with its own texture loader and returns the
/// update thread's end, the draw thread's uploads and the thread's handle (joined by dropping
/// the link and waiting).
pub fn spawn(
    config: &TextureLoaderConfig,
) -> std::io::Result<(SkinCursorLink, TextureUploads, std::thread::JoinHandle<()>)> {
    let config = TextureLoaderConfig {
        max_textures: MAX_CURSOR_TEXTURES,
        request_capacity: MAX_CURSOR_TEXTURES as usize,
        job_capacity: 2,
        ..*config
    };
    let (requests, uploads) = texture_loader(&config)?;
    let (commands, commands_rx) = rtrb::RingBuffer::new(RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(RING);
    let thread = std::thread::Builder::new()
        .name("slam-menu-cursor".into())
        .spawn(move || run(requests, commands_rx, replies_tx))?;
    Ok((
        SkinCursorLink {
            commands,
            replies,
            thread: thread.thread().clone(),
            generation: 0,
            asked: None,
            current: MenuCursorSkin::default(),
            unsent: [None; 8],
        },
        uploads,
        thread,
    ))
}

impl SkinCursorLink {
    /// Follows the wish: `Some(skin)` loads the cursor of that skin folder (`None` inside:
    /// the default skin), `None` lets go of the images. Returns what the draw thread shows.
    pub fn tick(&mut self, wanted: Option<Option<&Arc<Path>>>) -> MenuCursorSkin {
        while let Ok(reply) = self.replies.pop() {
            if reply.generation == self.generation && self.asked.is_some() {
                self.retire_current();
                self.current = reply.skin;
            } else {
                self.release_images(reply.skin);
            }
        }
        for slot in &mut self.unsent {
            if let Some(handle) = *slot
                && self.commands.push(Command::Release(handle)).is_ok()
            {
                *slot = None;
            }
        }
        match wanted {
            None => {
                if self.asked.take().is_some() {
                    // Answers still on their way are released as they come.
                    self.generation = self.generation.wrapping_add(1);
                    self.retire_current();
                    self.current = MenuCursorSkin::default();
                }
            }
            Some(skin) => {
                let same = self
                    .asked
                    .as_ref()
                    .is_some_and(|asked| asked.as_deref() == skin.map(|s| &**s));
                if !same {
                    let generation = self.generation.wrapping_add(1);
                    let command = Command::Load {
                        generation,
                        skin: skin.cloned(),
                    };
                    if self.commands.push(command).is_ok() {
                        self.generation = generation;
                        self.asked = Some(skin.cloned());
                        self.thread.unpark();
                    }
                }
            }
        }
        self.current
    }

    fn retire_current(&mut self) {
        let old = std::mem::take(&mut self.current);
        self.release_images(old);
    }

    fn release_images(&mut self, skin: MenuCursorSkin) {
        for image in [skin.cursor, skin.middle].into_iter().flatten() {
            if self.commands.push(Command::Release(image.handle)).is_err()
                && let Some(slot) = self.unsent.iter_mut().find(|s| s.is_none())
            {
                *slot = Some(image.handle);
            }
        }
        self.thread.unpark();
    }
}

// ---- Draw thread ----

/// The draw thread's end: the pointer, the press animation and the skin's textures.
pub struct MenuCursorView {
    pointer: PointerWatch,
    scale: WindowScale,
    uploads: Option<TextureUploads>,
    press: PressSqueeze,
    expand: ExpandTimes,
}

impl MenuCursorView {
    /// `uploads` of [`spawn`]; without them the skin's cursor shows as SLAM's.
    pub fn new(pointer: PointerWatch, scale: WindowScale, uploads: Option<TextureUploads>) -> Self {
        MenuCursorView {
            pointer,
            scale,
            uploads,
            press: PressSqueeze::default(),
            expand: ExpandTimes::default(),
        }
    }

    /// Uploads the skin's cursor images that came in; once per frame before drawing.
    pub fn process<D: Device>(&mut self, device: &mut D, textures: &mut Textures<D::Texture>) {
        if let Some(uploads) = &mut self.uploads {
            uploads.process(device, textures);
        }
    }

    /// Writes the cursor of `frame` at the pointer's last position on a surface `size`
    /// pixels big, drawn at engine time `now_ns`: nothing for the OS cursor, before the
    /// pointer first moved, or while gameplay draws its own. Allocation-free once `shapes`
    /// has room for [`MAX_MENU_CURSOR_SHAPES`].
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        theme: &Theme,
        white: AtlasRegion,
        size: [f32; 2],
        now_ns: u64,
        frame: &MenuCursorFrame,
    ) {
        let now_ms = now_ns as f64 / 1e6;
        // The animations follow the buttons even when nothing is drawn, so that a cursor
        // coming back does not start mid-press.
        if let Some(pressed) = self.press.update(theme, self.pointer.pressed(), now_ns) {
            if pressed {
                self.expand.expanded_at = now_ms;
                self.expand.contracted_at = f64::NEG_INFINITY;
            } else {
                self.expand.contracted_at = now_ms;
            }
        }
        if frame.gameplay_cursor || frame.style == CursorStyle::System {
            return;
        }
        let Some([x, y]) = self.pointer.position() else {
            return;
        };
        let ppp = self.scale.pixels_per_point();
        let tip = [x * ppp, y * ppp];
        let ui = theme.scale(size[1]);
        let slam = |shapes: &mut ShapeBatch| {
            let squeeze = squeeze_for(theme, &self.press, now_ns);
            push_slam_cursor(shapes, theme, white, tip, theme.cursor_size * ui, squeeze);
        };
        if frame.style == CursorStyle::Slam {
            slam(shapes);
            return;
        }
        let skin = &frame.skin;
        let Some(image) = skin.cursor else {
            // Loaded without a `cursor` in the chain: SLAM's. Not yet loaded: nothing.
            if skin.loaded {
                slam(shapes);
            }
            return;
        };
        let sprite = |image: SkinImage| -> Result<Option<SkinSprite>, ()> {
            let uploads = self.uploads.as_ref().ok_or(())?;
            match uploads.get(image.handle) {
                TextureState::Ready(region) => Ok(Some(SkinSprite::new(
                    region,
                    image.scale,
                    MAX_SKIN_CURSOR_SIZE,
                ))),
                TextureState::Pending => Ok(None),
                TextureState::Failed(_) => Err(()),
            }
        };
        match sprite(image) {
            Err(()) => slam(shapes),
            Ok(None) => {}
            Ok(Some(cursor)) => {
                let middle = skin.middle.and_then(|m| sprite(m).ok().flatten());
                push_skin_cursor(
                    shapes,
                    &cursor,
                    middle.as_ref(),
                    &skin.config,
                    tip,
                    ui,
                    self.expand.scale(now_ms, skin.config.expand),
                    rotation(now_ms, skin.config.rotate),
                );
            }
        }
    }
}

/// Writes the skin's `cursor` (which expands and turns with the skin's configuration) and
/// `cursormiddle` at `at`, `ui` pixels per display unit of the textures.
#[allow(clippy::too_many_arguments)]
pub fn push_skin_cursor(
    shapes: &mut ShapeBatch,
    cursor: &SkinSprite,
    middle: Option<&SkinSprite>,
    config: &CursorSkinConfig,
    at: [f32; 2],
    ui: f32,
    expand: f32,
    rotation_degrees: f32,
) {
    let origin = if config.centre { 0.5 } else { 0.0 };
    let mut piece = |texture: &SkinSprite, scale: f32, degrees: f32| {
        let size = [texture.size.x * ui * scale, texture.size.y * ui * scale];
        shapes.push(&Sprite {
            position: at,
            size,
            origin: [size[0] * origin, size[1] * origin],
            rotation: degrees.to_radians(),
            colour: Color::WHITE,
            texture: texture.texture,
            uv: texture.uv,
            blend: Blend::Alpha,
        });
    };
    piece(cursor, expand, rotation_degrees);
    if let Some(middle) = middle {
        piece(middle, 1.0, 0.0);
    }
}

#[cfg(test)]
mod tests;
