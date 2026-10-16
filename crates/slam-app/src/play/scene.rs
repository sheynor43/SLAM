//! A play's preparation and draw side, shared by the `play` command and the shell
//! (ADR-0045): [`prepare`] runs off the hot paths and gives everything a play needs but the
//! track; [`PlayScene`] owns the play's textures, glyphs and GPU resources while it is shown.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slam_engine::gl::SdlGlSurface;
use slam_osu::{Beatmap, ModSet};
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleVisuals, HitCircles,
};
use slam_play::cursor::{CursorView, MAX_CURSOR_SPRITES};
use slam_play::judgements::JudgementView;
use slam_play::resume::MAX_TARGET_SPRITES;
use slam_play::slider_body::Snaking;
use slam_play::sliders::{
    ArrowStyle, BallStyle, HitSliders, MAX_LIVE_SLIDERS, SliderBodies, SliderVisuals,
};
use slam_play::spinners::{HitSpinners, SpinnerStyle, SpinnerVisuals};
use slam_play::{
    BackgroundSettings, BackgroundView, GameplaySounds, Hitsounds, HudView, PlaySnapshot,
    PlayTextures, PlayView, PlayfieldLayout, SampleCache, SampleSkins, SoundSettings,
};
use slam_render::gl::{GlDevice, GlTexture};
use slam_render::slider::{SliderBuffer, SliderDraw, SliderMesh, SliderRenderer};
use slam_render::sprite::{DEFAULT_SLOTS, Projection, SpriteBatch, SpriteRenderer};
use slam_render::textures::Textures;
use slam_render::upload::{
    TextureHandle, TextureKind, TextureLoaderConfig, TextureRequests, TextureState, TextureUploads,
    texture_loader,
};
use slam_render::{Device, Extent, Limits, shaders};
use slam_skin::{Skin, SkinChain};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};

use super::{default_skin_dir, find_beatmap_file};

/// The mixer's format; the shell's output and the `play` command's use it.
pub(crate) const SAMPLE_RATE: u32 = 48_000;
pub(crate) const CHANNELS: u16 = 2;
/// Sprites of one frame: every live circle's pieces, numbers and approach circle, slider
/// ticks (up to `MAX_LIVE_NESTED`) and repeats, and judgement popups: up to `MAX_POPUPS`, of
/// 150 particles and three sprites each.
const MAX_SPRITES: usize = 32768;
/// Textures of a play: atlas pages of the skin and the glyphs, and images too large for a
/// page.
const MAX_TEXTURES: usize = 64;
/// Glyph sprites of one frame: the HUD's numbers and the skip button.
const MAX_GLYPH_SPRITES: usize = 256;

type Gl = GlDevice<SdlGlSurface>;

/// What a play takes from the player's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaySettings {
    /// User skin folder; without it, the default skin.
    pub skin: Option<PathBuf>,
    /// Take colours from the beatmap's skin (lazer's `BeatmapColours`).
    pub beatmap_colours: bool,
    pub mods: ModSet,
    /// How dark the background is, 0 ..= 1 (lazer's `DimLevel`).
    pub dim: f64,
}

impl Default for PlaySettings {
    fn default() -> Self {
        Self {
            skin: None,
            beatmap_colours: true,
            mods: ModSet::default(),
            dim: BackgroundSettings::default().dim_level,
        }
    }
}

/// The beatmap and skins of a play, read from disk.
pub(crate) struct Loaded {
    pub beatmap: Beatmap,
    pub audio: PathBuf,
    pub beatmap_skin: Skin,
    pub skins: SkinChain,
    /// The mods that built `beatmap`: the same instance goes on to the play, as lazer's
    /// `Player` hands its one clone to the beatmap and the rulesets' processors.
    pub mods: ModSet,
    /// The background image's name and bytes; `None` when the beatmap names none or it
    /// cannot be read.
    pub background: Option<(String, Vec<u8>)>,
}

pub(crate) fn load(map: &Path, settings: &PlaySettings) -> Result<Loaded, String> {
    let bytes = std::fs::read(map).map_err(|e| format!("{}: {e}", map.display()))?;
    let file = slam_formats::osu::decode(&bytes)
        .map_err(|e| format!("{}: {e}", map.display()))?
        .beatmap;
    let dir = map.parent().unwrap_or(Path::new("."));
    let audio = dir.join(&file.general.audio_filename);
    let background = file.events.background.as_deref().and_then(|name| {
        let bytes = find_beatmap_file(dir, name).and_then(|path| std::fs::read(path).ok());
        if bytes.is_none() {
            tracing::warn!("background {name}: not found or unreadable");
        }
        bytes.map(|bytes| (name.to_owned(), bytes))
    });
    // The play's start time and the visuals take the preempt of the beatmap with mods applied.
    let mut mods = settings.mods.clone();
    let beatmap = mods
        .playable_beatmap(file)
        .map_err(|e| format!("{}: {e}", map.display()))?;
    let beatmap_skin = Skin::load_beatmap(dir, &bytes).map_err(|e| format!("beatmap skin: {e}"))?;
    let default = Skin::load_default(default_skin_dir())
        .map_err(|e| format!("default skin {}: {e}", default_skin_dir().display()))?;
    let user = settings
        .skin
        .as_ref()
        .map(|dir| Skin::load(dir).map_err(|e| format!("skin {}: {e}", dir.display())))
        .transpose()?;
    Ok(Loaded {
        beatmap,
        mods,
        audio,
        beatmap_skin,
        skins: SkinChain::new(user, default),
        background,
    })
}

/// Everything a play needs but its track: the update side's parts and the [`PlayScene`].
pub struct Prepared {
    pub beatmap: Beatmap,
    pub mods: ModSet,
    /// The track to load before the play starts.
    pub audio: PathBuf,
    pub sounds: GameplaySounds,
    pub scene: PlayScene,
    /// The background image's name, for the console.
    pub background_name: Option<String>,
}

/// Reads and builds a play of `map`: beatmap, skins, hitsounds, visuals, slider meshes, and
/// a texture loader of its own that starts loading the play's images. Call it off the hot
/// paths; `limits` are the draw device's.
pub fn prepare(
    map: &Path,
    settings: &PlaySettings,
    limits: Limits,
    registry: Arc<FontRegistry>,
    skip_label: &str,
) -> Result<Prepared, String> {
    let loaded = load(map, settings)?;
    let mods = &loaded.mods;
    let beatmap = &loaded.beatmap;

    let mut cache = SampleCache::new(SAMPLE_RATE, CHANNELS);
    let hitsounds = Hitsounds::new(
        beatmap,
        SampleSkins {
            beatmap: Some(&loaded.beatmap_skin),
            chain: &loaded.skins,
        },
        &mut cache,
    );
    let sounds = GameplaySounds::new(beatmap, hitsounds, SoundSettings::for_mods(mods));
    let circle_mods = CircleMods::new(mods);
    let windows = slam_osu::hit_windows::HitWindows::new(beatmap.difficulty.overall_difficulty);
    let circle_settings = CircleSettings {
        beatmap_colours: settings.beatmap_colours,
        ..CircleSettings::default()
    };
    let view = PlayView {
        circles: HitCircles {
            visuals: CircleVisuals::new(
                beatmap,
                circle_mods,
                circle_settings,
                Some(&loaded.beatmap_skin),
                &loaded.skins,
            ),
            style: CircleStyle::new(circle_mods, &windows, &loaded.skins),
            config: CircleSkinConfig::new(&loaded.skins),
        },
        sliders: HitSliders {
            visuals: SliderVisuals::new(
                beatmap,
                circle_mods,
                circle_settings,
                Some(&loaded.beatmap_skin),
                &loaded.skins,
            ),
            snaking: Snaking::default(),
            arrows: ArrowStyle::new(&loaded.skins),
            ball: BallStyle::new(&loaded.skins),
        },
        spinners: HitSpinners {
            visuals: SpinnerVisuals::new(beatmap),
            style: SpinnerStyle::new(circle_mods, &loaded.skins),
        },
        follow_points: slam_play::follow_points::FollowPoints::new(beatmap),
        judgements: JudgementView::new(
            beatmap,
            circle_settings,
            Some(&loaded.beatmap_skin),
            &loaded.skins,
        ),
        hit_error: slam_play::hit_error::HitErrorView::new(&windows),
        cursor: CursorView::default(),
        resume: slam_play::resume::ResumeView::default(),
    };

    let (mut requests, uploads): (TextureRequests, TextureUploads) =
        texture_loader(&TextureLoaderConfig::new(limits)).map_err(|e| e.to_string())?;
    let textures =
        PlayTextures::request(&loaded.skins, &mut requests).map_err(|e| e.to_string())?;
    let background_name = loaded.background.as_ref().map(|(name, _)| name.clone());
    // The background is optional: a refused request plays without it.
    let background = loaded.background.and_then(|(name, bytes)| {
        requests
            .load(bytes, TextureKind::Standalone)
            .inspect_err(|e| tracing::warn!("background {name}: {e}"))
            .ok()
    });
    let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits))
        .map_err(|e| e.to_string())?;
    let hud = HudView::new(registry, skip_label);
    let scene = PlayScene {
        _requests: requests,
        uploads,
        textures,
        glyphs,
        view,
        hud,
        background,
        background_view: BackgroundView::new(BackgroundSettings {
            dim_level: settings.dim,
            ..BackgroundSettings::default()
        }),
        batch: SpriteBatch::with_capacity(MAX_SPRITES),
        bodies: SliderBodies::new(),
        meshes: SliderVisuals::meshes(beatmap),
        objects: beatmap.hit_objects.len(),
        text: SpriteBatch::with_capacity(MAX_GLYPH_SPRITES),
        cursor: SpriteBatch::with_capacity(MAX_CURSOR_SPRITES + MAX_TARGET_SPRITES),
        gpu: None,
    };
    Ok(Prepared {
        audio: loaded.audio,
        sounds,
        scene,
        background_name,
        beatmap: loaded.beatmap,
        mods: loaded.mods,
    })
}

/// The GPU resources of a play, between [`PlayScene::start`] and [`PlayScene::stop`].
struct SceneGpu {
    textures: Textures<GlTexture>,
    renderer: SpriteRenderer<Gl>,
    /// Draws MSDF glyphs.
    text_renderer: SpriteRenderer<Gl>,
    sliders: SliderRenderer<Gl>,
    /// Slider body meshes on the GPU, by object index.
    slider_buffers: Vec<Option<SliderBuffer<Gl>>>,
}

/// The draw side of a play: its textures, glyphs, views and sprite batches.
pub struct PlayScene {
    /// Kept while the loader may still load the play's images.
    _requests: TextureRequests,
    uploads: TextureUploads,
    textures: PlayTextures,
    glyphs: GlyphCache,
    view: PlayView,
    hud: HudView,
    /// The beatmap's background, if it has one.
    background: Option<TextureHandle>,
    background_view: BackgroundView,
    /// The background, the playfield and the HUD's shapes.
    batch: SpriteBatch,
    /// Slider bodies between the sprites of `batch`.
    bodies: SliderBodies,
    /// Slider body meshes by object index, uploaded at `start`.
    meshes: Vec<(u32, SliderMesh)>,
    /// Number of objects in the beatmap.
    objects: usize,
    /// The HUD's text.
    text: SpriteBatch,
    /// The cursor, on top of the HUD.
    cursor: SpriteBatch,
    gpu: Option<SceneGpu>,
}

impl PlayScene {
    /// Whether [`PlayScene::start`] made the GPU resources.
    pub fn started(&self) -> bool {
        self.gpu.is_some()
    }

    /// Makes the play's GPU resources on the draw thread and waits up to `glyph_wait` for
    /// the HUD's glyphs (a glyph takes several frames to appear; late ones show when
    /// ready). Navigation: allocates and uploads.
    pub fn start(&mut self, device: &mut Gl, glyph_wait: Duration) {
        // The device made these with the same context before; failing now is a driver
        // fault the application does not recover from.
        // Two draws a frame (playfield and HUD, cursor), each with its spare slots.
        let renderer = SpriteRenderer::new(device, 2 * DEFAULT_SLOTS).expect("renderer");
        let text_renderer =
            SpriteRenderer::with_shader(device, DEFAULT_SLOTS, &shaders::MSDF).expect("text");
        let mut sliders = SliderRenderer::new(device, MAX_LIVE_SLIDERS).expect("slider renderer");
        let mut slider_buffers: Vec<_> = (0..self.objects).map(|_| None).collect();
        for (index, mesh) in &self.meshes {
            // A slider that fails to upload is drawn without its body.
            match sliders.upload(device, mesh) {
                Ok(buffer) => slider_buffers[*index as usize] = buffer,
                Err(e) => tracing::warn!("slider {index}: {e}"),
            }
        }
        let mut textures = Textures::with_capacity(MAX_TEXTURES);
        let deadline = Instant::now() + glyph_wait;
        while self.hud.request_glyphs(&mut self.glyphs) > 0 && Instant::now() < deadline {
            self.glyphs.process(device, &mut textures);
            std::thread::sleep(Duration::from_millis(1));
        }
        self.gpu = Some(SceneGpu {
            textures,
            renderer,
            text_renderer,
            sliders,
            slider_buffers,
        });
    }

    /// Draws the play into the frame `device` began at `size`. Does nothing before
    /// [`PlayScene::start`].
    pub fn draw(&mut self, device: &mut Gl, size: Extent, now_ns: u64, snapshot: &PlaySnapshot) {
        let Some(r) = self.gpu.as_mut() else {
            return;
        };
        self.uploads.process(device, &mut r.textures);
        self.glyphs.process(device, &mut r.textures);
        let textures = self.textures.resolve(&self.uploads);
        let (width, height) = (size.width as f32, size.height as f32);
        let layout = PlayfieldLayout::new(width, height);
        self.hud.update(width, height, &snapshot.hud);
        self.batch.clear();
        self.text.clear();
        self.cursor.clear();
        let background = self
            .background
            .and_then(|handle| match self.uploads.get(handle) {
                TextureState::Ready(region) => Some(region),
                _ => None,
            });
        self.background_view
            .push(&mut self.batch, width, height, snapshot, now_ns, background);
        self.view.push(
            &mut self.batch,
            &mut self.bodies,
            &layout,
            snapshot,
            now_ns,
            &textures,
        );
        if let Some(white) = textures.white {
            self.hud.push_shapes(&mut self.batch, snapshot, white);
        }
        self.view
            .push_hit_error(&mut self.batch, width, height, snapshot, now_ns, &textures);
        self.hud
            .push_text(&mut self.text, snapshot, &mut self.glyphs);
        self.view
            .push_cursor(&mut self.cursor, &layout, snapshot, now_ns, &textures);
        r.renderer.begin_frame();
        r.text_renderer.begin_frame();
        let gpu_textures = &r.textures;
        let projection = Projection::pixels(size);
        // A failed draw loses one frame; the next one tries again.
        let texture = |id| gpu_textures.get(id);
        let _ = r.sliders.begin_frame(device, size);
        // The playfield's sprites in parts, with the slider bodies between them.
        if let Ok(uploaded) = r.renderer.upload(device, &self.batch, &projection, texture) {
            let mut from = 0;
            for body in self.bodies.as_slice() {
                let _ = r.renderer.draw_range(
                    device,
                    &uploaded,
                    &self.batch,
                    from..body.before_sprite,
                    texture,
                );
                from = body.before_sprite;
                let Some(Some(buffer)) = r.slider_buffers.get(body.index as usize) else {
                    continue;
                };
                let origin = layout.to_window(body.position);
                let path = projection.scaled(layout.scale(), [origin.x, origin.y]);
                let _ = r.sliders.draw(
                    device,
                    buffer,
                    &SliderDraw {
                        projection: &path,
                        radius: body.radius,
                        range: body.range,
                        tint: body.tint,
                        gradient: self.view.sliders.visuals.gradient(body.gradient),
                    },
                    None,
                );
            }
            let _ = r
                .renderer
                .draw_range(device, &uploaded, &self.batch, from..u32::MAX, texture);
        }
        let _ = r
            .text_renderer
            .draw(device, &self.text, &projection, texture);
        let _ = r.renderer.draw(device, &self.cursor, &projection, texture);
    }

    /// Destroys the play's GPU resources. Navigation: frees.
    pub fn stop(&mut self, device: &mut Gl) {
        let Some(SceneGpu {
            mut textures,
            renderer,
            text_renderer,
            mut sliders,
            slider_buffers,
        }) = self.gpu.take()
        else {
            return;
        };
        for buffer in slider_buffers.into_iter().flatten() {
            sliders.release(device, buffer);
        }
        sliders.destroy(device);
        renderer.destroy(device);
        text_renderer.destroy(device);
        for texture in textures.drain() {
            device.destroy_texture(texture);
        }
    }
}
