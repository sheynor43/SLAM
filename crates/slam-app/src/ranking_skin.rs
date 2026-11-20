//! The skin's results screen images (ADR-0158): which skin shows its own results screen,
//! how its images reach the draw thread, and the draw thread's end that resolves them.
//!
//! Like the menu cursor (ADR-0124) there are three ends: the update thread's
//! [`RankingLink`] asks for the images of the menu skin; the `slam-ranking-skin` thread
//! reads the skin and queues the images on its own texture loader; the draw thread's
//! [`RankingView`] uploads them and turns them into a [`StableSkin`] each frame.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::Thread;
use std::time::Duration;

use slam_render::hal::Device;
use slam_render::textures::Textures;
use slam_render::upload::{
    TextureHandle, TextureKind, TextureLoaderConfig, TextureRequests, TextureState, TextureUploads,
    texture_loader,
};
use slam_skin::{AnimationOptions, SkinChain, SkinVersion};
use slam_ui::results::stable::{
    ArtSize, GLYPHS, GRADE_NAMES, Glyph, HIT_NAMES, MODS, StableArt, StableImages, StableSkin,
    StableSprite,
};

use crate::menu_cursor::{CursorShown, SkinImage, menu_chain, reached};

/// Images of the screen: the loader holds this many at most.
pub const MAX_RANKING_TEXTURES: u32 = 64;

const RING: usize = 160;
const PARK: Duration = Duration::from_millis(100);

/// The skin's images as the draw thread gets them.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RankingSkin {
    /// Whether the thread answered.
    pub loaded: bool,
    /// Whether the skin has a `ranking-panel`: the screen is stable's then.
    pub present: bool,
    /// `Version` above 1.0.
    pub modern: bool,
    pub score_overlap: f32,
    pub combo_overlap: f32,
    pub images: StableImages<Option<SkinImage>>,
    /// The sizes of the images the buttons' places depend on, read from the files' headers
    /// for the update thread's hit testing (ADR-0161). A file whose size is unknown (not a
    /// PNG) counts as the default size.
    pub art: StableArt,
}

impl RankingSkin {
    /// Whether the results are drawn in the skin's look: the one predicate both the update
    /// side's hit testing and the draw side use (ADR-0158).
    pub fn stable(&self) -> bool {
        self.loaded && self.present
    }

    fn handles(&self) -> impl Iterator<Item = TextureHandle> + '_ {
        self.images.iter().flatten().map(|image| image.handle)
    }
}

/// The files of the skin's results screen.
#[derive(Clone, Debug, PartialEq)]
pub struct RankingFiles {
    pub modern: bool,
    pub score_overlap: f32,
    pub combo_overlap: f32,
    pub images: StableImages<Option<(PathBuf, f32)>>,
}

/// The ranking images of `skins`; `None` when no skin has a `ranking-panel`. The skin that
/// has the panel gives the version, the font prefixes and the overlaps.
// Ported from McOsu db2add2: src/App/Osu/OsuSkin.cpp (load)
pub fn ranking_files(skins: &SkinChain) -> Option<RankingFiles> {
    let panel = skins.texture("ranking-panel")?;
    let config = skins.skin(panel.source);
    let file = |name: &str| skins.texture(name).map(|t| (t.value.path, t.value.scale));
    let first_frame = |name: &str| {
        let options = AnimationOptions {
            animatable: true,
            ..AnimationOptions::default()
        };
        let animation = skins.animation(name, options)?;
        animation
            .value
            .frames
            .first()
            .map(|t| (t.path.clone(), t.scale))
    };
    // A font whose prefix has no digit falls back on the default `score-N`; the other
    // characters do not (stable draws no `x` for such skins).
    let font = |prefix: &str, glyphs: &[Glyph]| -> [Option<(PathBuf, f32)>; GLYPHS] {
        std::array::from_fn(|i| {
            let glyph = glyphs.iter().copied().find(|g| g.index() == i)?;
            match glyph {
                Glyph::Digit(d) => {
                    file(&format!("{prefix}-{d}")).or_else(|| file(&format!("score-{d}")))
                }
                Glyph::Dot => file(&format!("{prefix}-dot")),
                Glyph::Percent => file(&format!("{prefix}-percent")),
                Glyph::X => file(&format!("{prefix}-x")),
            }
        })
    };
    let digits = (0..10).map(Glyph::Digit);
    // Stable draws every number of the panel in the score font, the counts' `x` too.
    let score_glyphs: Vec<Glyph> = digits
        .clone()
        .chain([Glyph::Dot, Glyph::Percent, Glyph::X])
        .collect();
    let combo_glyphs: Vec<Glyph> = digits.chain([Glyph::X]).collect();
    Some(RankingFiles {
        modern: config.version() > SkinVersion::V1,
        score_overlap: config.score_overlap(),
        combo_overlap: config.combo_overlap(),
        images: StableImages {
            panel: Some((panel.value.path, panel.value.scale)),
            title: file("ranking-title"),
            max_combo: file("ranking-maxcombo"),
            accuracy: file("ranking-accuracy"),
            perfect: first_frame("ranking-perfect"),
            grades: std::array::from_fn(|i| file(GRADE_NAMES[i])),
            hits: std::array::from_fn(|i| first_frame(HIT_NAMES[i])),
            score_font: font(config.score_prefix(), &score_glyphs),
            combo_font: font(config.combo_prefix(), &combo_glyphs),
            mods: std::array::from_fn(|i| file(MODS[i].0)),
            back: first_frame("menu-back"),
            retry: file("ranking-retry"),
            graph: file("ranking-graph"),
        },
    })
}

// ---- Loader thread ----

enum Command {
    Load {
        generation: u32,
        skin: Option<Arc<Path>>,
    },
    Release(TextureHandle),
}

struct Reply {
    generation: u32,
    skin: RankingSkin,
}

/// The pixel size of a PNG from its header.
fn png_size(bytes: &[u8]) -> Option<[f32; 2]> {
    if bytes.len() < 24 || bytes[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
        return None;
    }
    let be =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Some([be(16) as f32, be(20) as f32])
}

/// The size of the image in `file`, if it is a PNG (reads its header only).
fn file_size(file: &Option<(PathBuf, f32)>) -> Option<ArtSize> {
    use std::io::Read;
    let (path, density) = file.as_ref()?;
    let mut header = [0_u8; 24];
    std::fs::File::open(path)
        .ok()?
        .read_exact(&mut header)
        .ok()?;
    Some(ArtSize {
        size: png_size(&header)?,
        density: *density,
    })
}

pub(crate) fn read_image(
    requests: &mut TextureRequests,
    file: &Option<(PathBuf, f32)>,
) -> Option<SkinImage> {
    let (path, scale) = file.as_ref()?;
    let bytes = std::fs::read(path)
        .inspect_err(|e| tracing::warn!("ranking image {}: {e}", path.display()))
        .ok()?;
    let handle = requests
        .load(bytes, TextureKind::Standalone)
        .inspect_err(|e| tracing::warn!("ranking image {}: {}", path.display(), e.reason))
        .ok()?;
    Some(SkinImage {
        handle,
        scale: *scale,
    })
}

fn load_skin(requests: &mut TextureRequests, user: Option<&Path>) -> RankingSkin {
    match menu_chain(user) {
        Ok(chain) => load_chain(requests, &chain),
        Err(e) => {
            tracing::warn!("ranking skin: {e}");
            RankingSkin {
                loaded: true,
                ..RankingSkin::default()
            }
        }
    }
}

/// Queues the ranking images of `chain` on `requests`; the handles come back at once.
pub(crate) fn load_chain(requests: &mut TextureRequests, chain: &SkinChain) -> RankingSkin {
    let Some(files) = ranking_files(chain) else {
        return RankingSkin {
            loaded: true,
            ..RankingSkin::default()
        };
    };
    let art = StableArt {
        title: file_size(&files.images.title),
        back: file_size(&files.images.back),
        retry: file_size(&files.images.retry),
    };
    let images = files.images.map(|file| read_image(requests, file));
    RankingSkin {
        loaded: true,
        present: true,
        modern: files.modern,
        score_overlap: files.score_overlap,
        combo_overlap: files.combo_overlap,
        images,
        art,
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
                for handle in reply.skin.handles() {
                    let _ = requests.release(handle);
                }
            }
        }
        if commands.is_abandoned() {
            return;
        }
        std::thread::park_timeout(PARK);
    }
}

/// A texture the loader may free once `after` (`None`: at once) has been drawn.
#[derive(Clone, Copy)]
struct Retiring {
    handle: TextureHandle,
    after: Option<u32>,
}

/// The update thread's end. Neither blocks nor allocates (a skin folder's path is shared).
///
/// Textures are let go of by the draw thread's say (ADR-0066): a skin's images are released
/// only after the draw thread acknowledged a snapshot without them. A new skin's images are
/// asked for once the old ones are gone, so that the loader never holds two sets.
pub struct RankingLink {
    commands: rtrb::Producer<Command>,
    replies: rtrb::Consumer<Reply>,
    thread: Thread,
    shown: Arc<AtomicU32>,
    seq: u32,
    generation: u32,
    loading: bool,
    /// The skin folder asked for last; `None` outer: nothing asked.
    asked: Option<Option<Arc<Path>>>,
    current: RankingSkin,
    retiring: [Option<Retiring>; MAX_RANKING_TEXTURES as usize],
}

/// Starts the loader thread `slam-ranking-skin` and returns the update thread's end, the
/// draw thread's uploads and acknowledgement, and the thread's handle (it ends once the
/// link is dropped).
pub fn spawn(
    config: &TextureLoaderConfig,
) -> std::io::Result<(
    RankingLink,
    TextureUploads,
    CursorShown,
    std::thread::JoinHandle<()>,
)> {
    let config = TextureLoaderConfig {
        max_textures: MAX_RANKING_TEXTURES,
        request_capacity: RING,
        job_capacity: 2,
        ..*config
    };
    let (requests, uploads) = texture_loader(&config)?;
    let (commands, commands_rx) = rtrb::RingBuffer::new(RING);
    let (replies_tx, replies) = rtrb::RingBuffer::new(4);
    let thread = std::thread::Builder::new()
        .name("slam-ranking-skin".into())
        .spawn(move || run(requests, commands_rx, replies_tx))?;
    let shown = CursorShown::default();
    Ok((
        RankingLink {
            commands,
            replies,
            thread: thread.thread().clone(),
            shown: shown.share(),
            seq: 0,
            generation: 0,
            loading: false,
            asked: None,
            current: RankingSkin::default(),
            retiring: [None; MAX_RANKING_TEXTURES as usize],
        },
        uploads,
        shown,
        thread,
    ))
}

impl RankingLink {
    /// The `seq` the frame of the last [`tick`](Self::tick) must carry.
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// Follows the wish: `Some(skin)` loads the images of that menu skin (`None` inside: the
    /// default skin alone), `None` lets go of them. Returns what the draw thread shows:
    /// nothing until the images are in.
    pub fn tick(&mut self, wanted: Option<Option<&Arc<Path>>>) -> RankingSkin {
        self.seq = self.seq.wrapping_add(1);
        while let Ok(reply) = self.replies.pop() {
            self.loading = false;
            if reply.generation == self.generation && self.asked.is_some() {
                self.current = reply.skin;
            } else {
                // Never shown: nothing to wait for.
                self.retire(reply.skin, None);
            }
        }
        self.flush();
        let Some(skin) = wanted else {
            if self.asked.take().is_some() {
                // An answer still on its way is released as it comes.
                self.generation = self.generation.wrapping_add(1);
                let old = std::mem::take(&mut self.current);
                self.retire(old, Some(self.seq));
            }
            return self.current;
        };
        let same = self
            .asked
            .as_ref()
            .is_some_and(|asked| asked.as_deref() == skin.map(|s| &**s));
        if !same {
            if self.asked.take().is_some() {
                // An answer still on its way is released as it comes.
                self.generation = self.generation.wrapping_add(1);
                let old = std::mem::take(&mut self.current);
                self.retire(old, Some(self.seq));
            }
            let idle = !self.loading && self.retiring.iter().all(Option::is_none);
            if idle {
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
        self.current
    }

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

    fn retire(&mut self, skin: RankingSkin, after: Option<u32>) {
        for handle in skin.handles() {
            if let Some(slot) = self.retiring.iter_mut().find(|s| s.is_none()) {
                *slot = Some(Retiring { handle, after });
            }
        }
    }

    /// Images waiting for the draw thread to let go of them.
    pub fn retiring(&self) -> usize {
        self.retiring.iter().flatten().count()
    }
}

// ---- Draw thread ----

/// The draw thread's end: the skin's uploaded textures.
pub struct RankingView {
    uploads: Option<TextureUploads>,
    shown: Option<CursorShown>,
}

impl RankingView {
    /// `uploads` of [`spawn`]; without them the screen is SLAM's.
    pub fn new(uploads: Option<TextureUploads>, shown: Option<CursorShown>) -> Self {
        RankingView { uploads, shown }
    }

    /// Uploads the images that came in; once per frame before drawing.
    pub fn process<D: Device>(&mut self, device: &mut D, textures: &mut Textures<D::Texture>) {
        if let Some(uploads) = &mut self.uploads {
            uploads.process(device, textures);
        }
    }

    /// Acknowledges the snapshot `seq`.
    pub fn acknowledge(&self, seq: u32) {
        if let Some(shown) = &self.shown {
            shown.set(seq);
        }
    }

    /// The skin to draw the results with: `None` (SLAM's screen) unless the skin has a
    /// panel and it is uploaded. Allocation-free.
    pub fn resolve(&self, skin: &RankingSkin) -> Option<StableSkin> {
        if !skin.stable() {
            return None;
        }
        let uploads = self.uploads.as_ref()?;
        let images = skin.images.map(|image| {
            let image = (*image)?;
            match uploads.get(image.handle) {
                TextureState::Ready(region) => Some(StableSprite {
                    texture: region.texture,
                    uv: region.uv,
                    size: [region.size.width as f32, region.size.height as f32],
                    density: image.scale,
                }),
                TextureState::Pending | TextureState::Failed(_) => None,
            }
        });
        Some(StableSkin {
            modern: skin.modern,
            score_overlap: skin.score_overlap,
            combo_overlap: skin.combo_overlap,
            images,
        })
    }
}

#[cfg(test)]
mod tests;
