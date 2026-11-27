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
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::Thread;
use std::time::Duration;

use slam_input::{PointerWatch, WindowScale};
use slam_osu::Vec2;
use slam_play::circles::SkinSprite;
use slam_play::cursor::{
    CursorScale, CursorSkinConfig, CursorSources, CursorTrail, ExpandTimes, MAX_TRAIL_PARTS,
    TRAIL_TEXTURE_FACTOR, rotation,
};
use slam_play::playfield::PlayfieldLayout;
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
use slam_ui::cursor::{CursorStyle, MAX_CURSOR_SHAPES, MenuCursorMotion, push_slam_cursor};
use slam_ui::theme::Theme;

use crate::settings::Settings;

/// Textures of the cursor's loader: `cursor`, `cursormiddle` and `cursortrail`, and the same
/// three of the skin before, until the draw thread lets go.
pub const MAX_CURSOR_TEXTURES: u32 = 6;

/// Shapes and sprites one frame of the menu cursor writes at most: SLAM's pointer or the
/// skin's cursor with its trail.
pub const MAX_MENU_CURSOR_SHAPES: usize = MAX_CURSOR_SHAPES + MAX_TRAIL_PARTS;

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
    /// The skin's `cursortrail`; `None`: it has none, the skin's cursor leaves no trail.
    pub trail: Option<SkinImage>,
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
    /// `general.menu_cursor_size`: the multiplier of SLAM's pointer and the skin's cursor
    /// (ADR-0184).
    pub size: f32,
    /// The update tick that made this frame; the draw thread acknowledges it through
    /// [`CursorShown`] so that textures the frame left out can be released.
    pub seq: u32,
}

impl Default for MenuCursorFrame {
    fn default() -> Self {
        MenuCursorFrame {
            style: CursorStyle::Slam,
            gameplay_cursor: false,
            skin: MenuCursorSkin::default(),
            size: 1.0,
            seq: 0,
        }
    }
}

/// The files of the skin's cursor: where the chain finds them.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorFiles {
    pub cursor: Option<(PathBuf, f32)>,
    pub middle: Option<(PathBuf, f32)>,
    /// The `cursortrail` of the first skin with one, whatever skin has the `cursor`.
    pub trail: Option<(PathBuf, f32)>,
    pub config: CursorSkinConfig,
}

/// The `cursor` of the first skin of `skins` with one, and its `cursormiddle`; none of them
/// when no skin has a `cursor` (SLAM's cursor is drawn then).
pub fn cursor_files(skins: &SkinChain) -> CursorFiles {
    let sources = CursorSources::new(skins);
    let Some(source) = sources.cursor else {
        return CursorFiles {
            cursor: None,
            middle: None,
            trail: None,
            config: CursorSkinConfig::new(skins),
        };
    };
    let skin = skins.skin(source);
    let file = |name: &str| skin.texture(name).map(|t| (t.path, t.scale));
    let trail = sources
        .trail
        .and_then(|source| skins.skin(source).texture("cursortrail"))
        .map(|t| (t.path, t.scale));
    CursorFiles {
        cursor: file("cursor"),
        middle: file("cursormiddle"),
        trail,
        config: CursorSkinConfig::new(skins),
    }
}

/// The skin chain of the menus: the chosen skin over the default one.
pub(crate) fn menu_chain(user: Option<&Path>) -> Result<SkinChain, String> {
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
        trail: read_image(requests, files.trail),
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
                for image in [reply.skin.cursor, reply.skin.middle, reply.skin.trail]
                    .into_iter()
                    .flatten()
                {
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

/// The draw thread's acknowledgement: the newest snapshot it began drawing.
#[derive(Clone, Debug, Default)]
pub struct CursorShown(Arc<AtomicU32>);

impl CursorShown {
    /// The shared counter, for the update thread's end of a loader link.
    pub(crate) fn share(&self) -> Arc<AtomicU32> {
        self.0.clone()
    }

    /// Records that the snapshot `seq` is being drawn.
    pub fn set(&self, seq: u32) {
        self.0.store(seq, Ordering::Release);
    }
}

/// Whether the draw thread, at `shown`, has got to snapshot `seq` or later.
pub(crate) fn reached(shown: u32, seq: u32) -> bool {
    shown.wrapping_sub(seq) < u32::MAX / 2
}

/// Images waiting to be let go of.
const RETIRING: usize = 8;

/// A texture the loader may free once `after` (`None`: at once) has been drawn.
#[derive(Clone, Copy)]
struct Retiring {
    handle: TextureHandle,
    after: Option<u32>,
}

/// The update thread's end. Neither blocks nor allocates (a skin folder's path is shared).
///
/// Textures are let go of by the draw thread's say (ADR-0066): the images a snapshot
/// stopped showing are released only after the draw thread acknowledged a later snapshot,
/// and the old cursor stays on until the new one's images have come in.
pub struct SkinCursorLink {
    commands: rtrb::Producer<Command>,
    replies: rtrb::Consumer<Reply>,
    thread: Thread,
    shown: Arc<AtomicU32>,
    /// Tick counter: the `seq` of the frame of this tick.
    seq: u32,
    /// The newest generation asked for; older answers are released unseen.
    generation: u32,
    /// Whether a load was sent and its answer has not come: one at a time, so that the
    /// loader never holds more than the shown images and one pair on their way.
    loading: bool,
    /// The skin folder asked for last; `None` outer: nothing asked.
    asked: Option<Option<Arc<Path>>>,
    current: MenuCursorSkin,
    /// Images to release, retried until the command ring takes them.
    retiring: [Option<Retiring>; RETIRING],
}

/// Starts the loader thread `slam-menu-cursor` with its own texture loader and returns the
/// update thread's end, the draw thread's uploads and acknowledgement, and the thread's
/// handle (it ends once the link is dropped; join it after that).
pub fn spawn(
    config: &TextureLoaderConfig,
) -> std::io::Result<(
    SkinCursorLink,
    TextureUploads,
    CursorShown,
    std::thread::JoinHandle<()>,
)> {
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
    let shown = CursorShown::default();
    Ok((
        SkinCursorLink::new(commands, replies, thread.thread().clone(), shown.0.clone()),
        uploads,
        shown,
        thread,
    ))
}

impl SkinCursorLink {
    fn new(
        commands: rtrb::Producer<Command>,
        replies: rtrb::Consumer<Reply>,
        thread: Thread,
        shown: Arc<AtomicU32>,
    ) -> Self {
        SkinCursorLink {
            commands,
            replies,
            thread,
            shown,
            seq: 0,
            generation: 0,
            loading: false,
            asked: None,
            current: MenuCursorSkin::default(),
            retiring: [None; RETIRING],
        }
    }

    /// The `seq` the frame of the last [`tick`](Self::tick) must carry.
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// Follows the wish: `Some(skin)` loads the cursor of that skin folder (`None` inside:
    /// the default skin), `None` lets go of the images. Returns what the draw thread shows.
    pub fn tick(&mut self, wanted: Option<Option<&Arc<Path>>>) -> MenuCursorSkin {
        self.seq = self.seq.wrapping_add(1);
        while let Ok(reply) = self.replies.pop() {
            self.loading = false;
            if reply.generation == self.generation && self.asked.is_some() {
                self.retire_current();
                self.current = reply.skin;
            } else {
                // Never shown: nothing to wait for.
                self.retire_images(reply.skin, None);
            }
        }
        self.flush();
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
                // A new load waits for the previous images to be let go of and for the
                // answer in flight: the loader has room for the three shown images and three new ones.
                let idle = !self.loading && self.retiring.iter().all(Option::is_none);
                if !same && idle {
                    let generation = self.generation.wrapping_add(1);
                    let command = Command::Load {
                        generation,
                        skin: skin.cloned(),
                    };
                    if self.commands.push(command).is_ok() {
                        self.generation = generation;
                        self.loading = true;
                        self.asked = Some(skin.cloned());
                        self.thread.unpark();
                    }
                }
            }
        }
        self.current
    }

    /// Sends the releases whose snapshot the draw thread has got past.
    fn flush(&mut self) {
        let shown = self.shown.load(Ordering::Acquire);
        for slot in &mut self.retiring {
            if let Some(Retiring { handle, after }) = *slot
                && after.is_none_or(|seq| reached(shown, seq))
                && self.commands.push(Command::Release(handle)).is_ok()
            {
                *slot = None;
            }
        }
        self.thread.unpark();
    }

    /// The current images stop being shown with this tick's frame.
    fn retire_current(&mut self) {
        let old = std::mem::take(&mut self.current);
        self.retire_images(old, Some(self.seq));
    }

    fn retire_images(&mut self, skin: MenuCursorSkin, after: Option<u32>) {
        for image in [skin.cursor, skin.middle, skin.trail].into_iter().flatten() {
            if let Some(slot) = self.retiring.iter_mut().find(|s| s.is_none()) {
                *slot = Some(Retiring {
                    handle: image.handle,
                    after,
                });
            }
        }
    }

    /// Images waiting for the draw thread to let go of them.
    pub fn retiring(&self) -> usize {
        self.retiring.iter().flatten().count()
    }
}

// ---- Draw thread ----

/// The draw thread's end: the pointer, the press and drag animation and the skin's textures.
pub struct MenuCursorView {
    pointer: PointerWatch,
    scale: WindowScale,
    uploads: Option<TextureUploads>,
    shown: Option<CursorShown>,
    motion: MenuCursorMotion,
    expand: ExpandTimes,
    trail: MenuTrail,
}

/// The skin's cursor trail in the menus: lazer's [`CursorTrail`] fed with the pointer, in a
/// layout where an osu!pixel is `ui * 1.6` window pixels so that the parts have the size of
/// the texture's display units, as the menu cursor has (ADR-0184).
pub struct MenuTrail {
    trail: CursorTrail,
    /// The image and the interface scale the parts were laid with: another one starts anew.
    image: Option<SkinImage>,
    ui: f32,
    /// Whether this frame fed the trail; a frame that did not clears it.
    fed: bool,
    /// The part scale of the last frame: this frame's moves leave parts with it, as
    /// gameplay's trail does (lazer: input before the frame's update).
    last_part_scale: Option<f32>,
}

impl Default for MenuTrail {
    fn default() -> Self {
        MenuTrail::new()
    }
}

impl MenuTrail {
    /// An empty trail. Allocates its parts once.
    pub fn new() -> Self {
        MenuTrail {
            trail: CursorTrail::new(),
            image: None,
            ui: 0.0,
            fed: false,
            last_part_scale: None,
        }
    }

    /// Forgets the parts: the cursor hid, or the skin or scale changed.
    pub fn idle(&mut self) {
        if self.image.take().is_some() {
            self.trail.reset();
        }
        self.last_part_scale = None;
    }

    /// Takes the pointer at `tip` and writes the parts under the cursor.
    /// Allocation-free once `shapes` has room for [`MAX_TRAIL_PARTS`].
    #[allow(clippy::too_many_arguments)]
    pub fn feed(
        &mut self,
        shapes: &mut ShapeBatch,
        texture: &SkinSprite,
        image: SkinImage,
        config: &CursorSkinConfig,
        tip: [f32; 2],
        ui: f32,
        size: f32,
        part_scale: f32,
        degrees: f32,
        now_ns: u64,
        window: [f32; 2],
    ) {
        if self.image != Some(image) || (self.ui - ui).abs() > 1e-3 {
            self.trail.reset();
            self.image = Some(image);
            self.ui = ui;
            self.last_part_scale = None;
        }
        self.fed = true;
        let layout = PlayfieldLayout::linear(ui * TRAIL_TEXTURE_FACTOR, window);
        self.trail
            .set_cursor_scale(CursorScale { size, scale: size });
        let at = Vec2::new(tip[0] / layout.scale(), tip[1] / layout.scale());
        let display_width = texture.size.x / TRAIL_TEXTURE_FACTOR;
        self.trail.update(
            now_ns,
            std::iter::once(at),
            config.disjoint,
            display_width,
            self.last_part_scale.unwrap_or(part_scale),
            part_scale,
        );
        self.last_part_scale = Some(part_scale);
        self.trail
            .push_to(&layout, texture, config, degrees, |sprite| {
                shapes.push(sprite)
            });
    }
}

impl MenuCursorView {
    /// `uploads` of [`spawn`]; without them the skin's cursor shows as SLAM's.
    pub fn new(pointer: PointerWatch, scale: WindowScale, uploads: Option<TextureUploads>) -> Self {
        MenuCursorView {
            pointer,
            scale,
            uploads,
            shown: None,
            motion: MenuCursorMotion::default(),
            expand: ExpandTimes::default(),
            trail: MenuTrail::new(),
        }
    }

    /// As [`with_shown`](Self::with_shown); without one, nothing is acknowledged.
    pub fn with_shown_opt(self, shown: Option<CursorShown>) -> Self {
        match shown {
            Some(shown) => self.with_shown(shown),
            None => self,
        }
    }

    /// Acknowledges each frame's `seq` through `shown` (of [`spawn`]).
    pub fn with_shown(mut self, shown: CursorShown) -> Self {
        self.shown = Some(shown);
        self
    }

    /// The pointer's freshest position in window pixels, `None` before it first moved and
    /// while it is outside the window; gameplay draws its cursor there (ADR-0162).
    pub fn pointer_pixels(&self) -> Option<[f32; 2]> {
        let ppp = self.scale.pixels_per_point();
        self.pointer.position().map(|[x, y]| [x * ppp, y * ppp])
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
        self.trail.fed = false;
        self.push_cursor(shapes, theme, white, size, now_ns, frame);
        if !self.trail.fed {
            self.trail.idle();
        }
    }

    fn push_cursor(
        &mut self,
        shapes: &mut ShapeBatch,
        theme: &Theme,
        white: AtlasRegion,
        size: [f32; 2],
        now_ns: u64,
        frame: &MenuCursorFrame,
    ) {
        if let Some(shown) = &self.shown {
            shown.set(frame.seq);
        }
        let now_ms = now_ns as f64 / 1e6;
        let ppp = self.scale.pixels_per_point();
        let ui = theme.scale(size[1]);
        // The animations follow the buttons even when nothing is drawn, so that a cursor
        // coming back does not start mid-press.
        let was_held = self.motion.held();
        let pointer = self.pointer.position().map(|[x, y]| [x * ppp, y * ppp]);
        let held = self.pointer.pressed();
        self.motion.update(theme, ui, pointer, held, now_ns);
        if held != was_held {
            if held {
                self.expand.expanded_at = now_ms;
                self.expand.contracted_at = f64::NEG_INFINITY;
            } else {
                self.expand.contracted_at = now_ms;
            }
        }
        if frame.gameplay_cursor || frame.style == CursorStyle::System {
            return;
        }
        let Some(tip) = pointer else {
            return;
        };
        let slam = |shapes: &mut ShapeBatch| {
            let motion = self.motion.sample(theme, now_ns);
            push_slam_cursor(
                shapes,
                theme,
                white,
                tip,
                theme.cursor_size * ui * frame.size,
                &motion,
            );
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
                let expand = self.expand.scale(now_ms, skin.config.expand);
                let degrees = rotation(now_ms, skin.config.rotate);
                if let Some(image) = skin.trail
                    && let Some(texture) = sprite(image).ok().flatten()
                {
                    self.trail.feed(
                        shapes,
                        &texture,
                        image,
                        &skin.config,
                        tip,
                        ui,
                        frame.size,
                        expand,
                        degrees,
                        now_ns,
                        size,
                    );
                }
                push_skin_cursor(
                    shapes,
                    &cursor,
                    middle.as_ref(),
                    &skin.config,
                    tip,
                    ui * frame.size,
                    expand,
                    degrees,
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
