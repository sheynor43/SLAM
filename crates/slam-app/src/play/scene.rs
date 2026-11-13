//! A play's preparation and draw side, shared by the `play` command and the shell
//! (ADR-0045): [`prepare`] runs off the hot paths and gives everything a play needs but the
//! track; [`PlayScene`] owns the play's textures, glyphs and GPU resources while it is shown.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slam_osu::{Beatmap, ModSet};
use slam_play::circles::{
    CircleMods, CircleSettings, CircleSkinConfig, CircleStyle, CircleVisuals, HitCircles,
};
use slam_play::cursor::{CursorScale, CursorView, MAX_CURSOR_SPRITES};
use slam_play::fail::FailFrame;
use slam_play::gameplay_sounds::DEFAULT_POSITIONAL_LEVEL;
use slam_play::hud::OFFSET_PLACEHOLDER;
use slam_play::judgements::JudgementView;
use slam_play::resume::MAX_TARGET_SPRITES;
use slam_play::slider_body::Snaking;
use slam_play::sliders::{
    ArrowStyle, BallStyle, HitSliders, MAX_LIVE_SLIDERS, SliderBodies, SliderVisuals,
};
use slam_play::spinners::{HitSpinners, SpinnerStyle, SpinnerVisuals};
use slam_play::{
    BackgroundSettings, BackgroundView, FpsLabels, FpsView, GameplaySounds, Hitsounds, HudLabels,
    HudSetup, HudView, MenuLabels, MenuView, PlaySnapshot, PlayTextures, PlayView, PlayfieldLayout,
    SampleCache, SampleSkins, SoundSettings,
};
use slam_render::slider::{SliderBuffer, SliderDraw, SliderMesh, SliderRenderer};
use slam_render::sprite::{
    Affine, DEFAULT_SLOTS, Projection, Sprite, SpriteBatch, SpriteRenderer, SpriteStats, tint_srgb,
};
use slam_render::textures::Textures;
use slam_render::upload::{
    TextureHandle, TextureKind, TextureLoaderConfig, TextureRequests, TextureUploads,
    texture_loader,
};
use slam_render::{Color, Device, Extent, Limits, shaders};
use slam_skin::{Skin, SkinChain};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::hud_layout::{Builtin, LayoutElement};
use slam_ui::i18n::{FluentArgs, Localizer};

use super::find_beatmap_file;
use super::fps::{FpsOverlay, MAX_FPS_GLYPHS};

/// The mixer's channels; the shell's output and the `play` command's use them. The rate
/// is the device's (ADR-0102), [`PlaySettings::sample_rate`].
pub(crate) const CHANNELS: u16 = 2;
/// Sprites of one frame: every live circle's pieces, numbers and approach circle (a ring
/// of `RING_QUADS` quads), slider ticks (up to `MAX_LIVE_NESTED`) and repeats, and judgement
/// popups: up to `MAX_POPUPS`, of 150 particles and three sprites each. The worst case is
/// about 28k with the rings; past the capacity the batch would allocate in the frame.
const MAX_SPRITES: usize = 40960;
const _: () = assert!(slam_play::hud::MAX_HUD_SHAPES < MAX_SPRITES / 4);
/// Room in the texture loader's queue for every texture of a play at once: a skin with
/// nearly every element requests about 260, and a full queue fails the preparation if the
/// loader thread has not drained some yet.
const TEXTURE_REQUEST_CAPACITY: usize = 1024;
/// Textures of a play: atlas pages of the skin and the glyphs, and images too large for a
/// page.
const MAX_TEXTURES: usize = 64;
/// Glyph sprites of one frame: the HUD's numbers and the skip button.
const MAX_GLYPH_SPRITES: usize = 256;
/// The menu's background and buttons, the fail menu's footer, save button and its hover,
/// and the FPS counter's box.
const MAX_MENU_SPRITES: usize = 5 + slam_play::menu_overlay::MAX_BUTTONS;

/// What a play takes from the player's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaySettings {
    /// User skin folder; without it, the default skin.
    pub skin: Option<PathBuf>,
    /// Use the beatmap's own skin textures and configuration (lazer's `BeatmapSkins`).
    pub beatmap_skins: bool,
    /// Take colours from the beatmap's skin (lazer's `BeatmapColours`).
    pub beatmap_colours: bool,
    /// Play the beatmap's own hitsound samples (lazer's `BeatmapHitsounds`).
    pub beatmap_hitsounds: bool,
    /// Play the combo break sound on the first break of a play even when the break happens while
    /// the combo is already low (lazer's `OsuSetting.AlwaysPlayFirstComboBreak`, on by default).
    pub always_play_first_combo_break: bool,
    pub mods: ModSet,
    /// How dark the background is, 0 ..= 1 (lazer's `DimLevel`).
    pub dim: f64,
    /// The gameplay elements of the active HUD layout (ADR-0109).
    pub hud_layout: Vec<LayoutElement>,
    /// The in-game interface scale the HUD's elements take (ADR-0118).
    pub game_scale: f32,
    /// Snaking of slider bodies (ADR-0085), off by default unlike lazer.
    pub snaking: Snaking,
    /// Lazer's `HitLighting`.
    pub hit_lighting: bool,
    /// Lazer's `ShowCursorTrail`.
    pub cursor_trail: bool,
    /// Lazer's `GameplayCursorSize` (ADR-0086).
    pub cursor_size: f32,
    /// Lazer's `AutoCursorSize`: the cursor's size follows the beatmap's circle size.
    pub auto_cursor_size: bool,
    /// Lazer's `PositionalHitsoundsLevel` (ADR-0087).
    pub positional_hitsounds_level: f32,
    /// Rate of the mixer the samples are prepared for (ADR-0102).
    pub sample_rate: u32,
    /// The key overlay's animation; `None`: no key overlay (ADR-0113).
    pub key_overlay: Option<slam_play::key_overlay::KeyAnimation>,
    /// Rolling counters, the combo's pops and the health bar's easing (on by default).
    pub hud_animations: bool,
}

impl Default for PlaySettings {
    fn default() -> Self {
        Self {
            skin: None,
            beatmap_skins: true,
            beatmap_colours: true,
            beatmap_hitsounds: true,
            always_play_first_combo_break: true,
            mods: ModSet::default(),
            dim: BackgroundSettings::default().dim_level,
            hud_layout: Builtin::Stable.layout().gameplay,
            game_scale: 1.0,
            snaking: Snaking::default(),
            hit_lighting: true,
            cursor_trail: true,
            cursor_size: 1.0,
            auto_cursor_size: false,
            positional_hitsounds_level: DEFAULT_POSITIONAL_LEVEL,
            sample_rate: slam_audio::DEFAULT_SAMPLE_RATE,
            key_overlay: None,
            hud_animations: true,
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
    /// MD5 of the beatmap file, lowercase hex: its replays name it.
    pub beatmap_hash: String,
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
    let default_dir = crate::default_skin::dir()?;
    let default = Skin::load_default(&default_dir)
        .map_err(|e| format!("default skin {}: {e}", default_dir.display()))?;
    // A skin that does not load is not worth cancelling the play: play on the default one.
    let user = settings.skin.as_ref().and_then(|dir| {
        Skin::load(dir)
            .inspect_err(|e| {
                tracing::warn!("skin {}: {e}, playing on the default skin", dir.display());
            })
            .ok()
    });
    // `BeatmapSkins` puts the beatmap's textures and configuration in front of the skins; its
    // combo colours and samples keep their own settings and read `beatmap_skin`.
    let skins = SkinChain::new(user, default)
        .with_beatmap(settings.beatmap_skins.then(|| beatmap_skin.clone()));
    Ok(Loaded {
        beatmap,
        mods,
        audio,
        beatmap_skin,
        skins,
        background,
        beatmap_hash: crate::replays::md5_hex(&bytes),
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
    /// MD5 of the beatmap file, lowercase hex.
    pub beatmap_hash: String,
}

/// The HUD's texts in the language of `localizer`.
pub fn hud_labels(localizer: &Localizer) -> HudLabels {
    let text = |key: &str| {
        let mut args = FluentArgs::new();
        args.set("offset", OFFSET_PLACEHOLDER);
        args.set("value", OFFSET_PLACEHOLDER);
        localizer.format(key, Some(&args)).into_owned()
    };
    HudLabels {
        skip: text("play-skip-button"),
        offset_zero: text("play-offset"),
        offset_earlier: text("play-offset-earlier"),
        offset_later: text("play-offset-later"),
        menu: MenuLabels {
            paused: text("play-menu-paused"),
            failed: text("play-menu-failed"),
            continue_button: text("play-menu-continue"),
            retry_button: text("play-menu-retry"),
            quit_button: text("play-menu-quit"),
            save_replay: text("play-menu-save-replay"),
            replay_saved: text("play-menu-replay-saved"),
            replay_unavailable: text("play-menu-replay-unavailable"),
            retry_count: text("play-menu-retry-count"),
            song_progress: text("play-menu-song-progress"),
            accuracy: text("play-menu-accuracy"),
        },
        fps: FpsLabels {
            frame_time: text("fps-counter-frame-time"),
            fps: text("fps-counter-fps"),
        },
    }
}

/// Reads and builds a play of `map`: beatmap, skins, hitsounds, visuals, slider meshes, and
/// a texture loader of its own that starts loading the play's images. Call it off the hot
/// paths; `limits` are the draw device's.
pub fn prepare(
    map: &Path,
    settings: &PlaySettings,
    limits: Limits,
    registry: Arc<FontRegistry>,
    labels: &HudLabels,
) -> Result<Prepared, String> {
    let config = TextureLoaderConfig {
        request_capacity: TEXTURE_REQUEST_CAPACITY,
        ..TextureLoaderConfig::new(limits)
    };
    let mut loaded = load(map, settings)?;
    // Images that do not decode are missing to every lookup below (ADR-0110).
    // `beatmap_skin` itself only gives combo colours and samples.
    loaded
        .skins
        .set_image_check(slam_play::session::skin_image_check(config.image_limits()));
    let mods = &loaded.mods;
    let beatmap = &loaded.beatmap;

    let mut cache = SampleCache::new(settings.sample_rate, CHANNELS);
    let sample_skins = SampleSkins {
        // Like lazer's `AllowSampleLookup`, disabled beatmap hitsounds drop the whole
        // beatmap skin from the sample lookup.
        beatmap: settings.beatmap_hitsounds.then_some(&loaded.beatmap_skin),
        chain: &loaded.skins,
    };
    let hitsounds = Hitsounds::new(beatmap, sample_skins, &mut cache);
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (load)
    let fail_sound = slam_play::hitsounds::effect("Gameplay/failsound", sample_skins, &mut cache);
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/ComboEffects.cs (load)
    let combo_break = slam_play::hitsounds::effect("Gameplay/combobreak", sample_skins, &mut cache);
    let sounds = GameplaySounds::new(
        beatmap,
        hitsounds,
        SoundSettings {
            positional_level: settings.positional_hitsounds_level,
            always_play_first_combo_break: settings.always_play_first_combo_break,
            ..SoundSettings::for_mods(mods)
        },
    )
    .with_fail_sound(fail_sound)
    .with_combo_break_sound(combo_break);
    let circle_mods = CircleMods::new(mods);
    let windows = slam_osu::hit_windows::HitWindows::new(beatmap.difficulty.overall_difficulty);
    let cursor_scale = CursorScale::new(
        settings.cursor_size,
        settings.auto_cursor_size,
        beatmap.difficulty.circle_size,
    );
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
            snaking: settings.snaking,
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
            settings.hit_lighting,
        ),
        cursor: CursorView::new(settings.cursor_trail, cursor_scale),
        resume: slam_play::resume::ResumeView::new(settings.cursor_trail, cursor_scale),
    };

    let (mut requests, uploads): (TextureRequests, TextureUploads) =
        texture_loader(&config).map_err(|e| e.to_string())?;
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
    // In place of a missing or broken one, as lazer's `BeatmapBackground` falls back to
    // `Backgrounds/bg1` (ADR-0076).
    let default_background = requests
        .load_pixels(
            slam_play::background::default_image(),
            TextureKind::Standalone,
        )
        .inspect_err(|e| tracing::warn!("default background: {}", e.reason))
        .ok();
    let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits))
        .map_err(|e| e.to_string())?;
    let menu = MenuView::new(Arc::clone(&registry), &labels.menu);
    // Each element grows about its anchor (ADR-0118).
    let hud_layout: Vec<LayoutElement> = settings
        .hud_layout
        .iter()
        .map(|element| element.scaled_by(settings.game_scale))
        .collect();
    let hud = HudView::new(
        registry,
        labels,
        HudSetup {
            layout: &hud_layout,
            windows: &windows,
            meter_hiding: &slam_play::hit_error::SpinnerHiding::new(beatmap),
            song_progress: slam_play::song_progress::SongProgress::new(beatmap),
            key_overlay: settings.key_overlay,
            animations: settings.hud_animations,
        },
    );
    // The layout decides how much text the HUD draws.
    let text = SpriteBatch::with_capacity(MAX_GLYPH_SPRITES.max(hud.max_glyphs()));
    let scene = PlayScene {
        requests,
        uploads,
        textures,
        glyphs,
        view,
        hud,
        background,
        default_background,
        background_view: BackgroundView::new(BackgroundSettings {
            dim_level: settings.dim,
            ..BackgroundSettings::default()
        }),
        batch: SpriteBatch::with_capacity(MAX_SPRITES),
        bodies: SliderBodies::new(),
        meshes: SliderVisuals::meshes(beatmap),
        objects: beatmap.hit_objects.len(),
        text,
        cursor: SpriteBatch::with_capacity(MAX_CURSOR_SPRITES + MAX_TARGET_SPRITES),
        menu,
        menu_shapes: SpriteBatch::with_capacity(MAX_MENU_SPRITES),
        menu_text: SpriteBatch::with_capacity(MAX_GLYPH_SPRITES + MAX_FPS_GLYPHS),
        stats: PlayfieldStats::default(),
    };
    Ok(Prepared {
        audio: loaded.audio,
        sounds,
        scene,
        background_name,
        beatmap: loaded.beatmap,
        mods: loaded.mods,
        beatmap_hash: loaded.beatmap_hash,
    })
}

/// The GPU resources of a play, made by [`PlayScene::start`] and destroyed by
/// [`SceneGpu::destroy`]. The draw thread keeps them beside the scene, so the scene
/// (made by the loader and passed through the update thread) does not name the render
/// backend.
pub struct SceneGpu<D: Device> {
    textures: Textures<D::Texture>,
    renderer: SpriteRenderer<D>,
    /// Draws MSDF glyphs.
    text_renderer: SpriteRenderer<D>,
    sliders: SliderRenderer<D>,
    /// Slider body meshes on the GPU, by object index.
    slider_buffers: Vec<Option<SliderBuffer<D>>>,
}

/// The draw side of a play: its textures, glyphs, views and sprite batches.
pub struct PlayScene {
    /// Kept while the loader may still load the play's images.
    requests: TextureRequests,
    uploads: TextureUploads,
    textures: PlayTextures,
    glyphs: GlyphCache,
    view: PlayView,
    hud: HudView,
    /// The beatmap's background, if it has one.
    background: Option<TextureHandle>,
    /// SLAM's own background, drawn when the beatmap's is missing or failed (ADR-0076).
    default_background: Option<TextureHandle>,
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
    /// The pause and fail menus, on top of everything (ADR-0052).
    menu: MenuView,
    menu_shapes: SpriteBatch,
    menu_text: SpriteBatch,
    /// The playfield batch's figures of the last frame, for the console report.
    stats: PlayfieldStats,
}

/// What the last frame drew of the playfield batch (background, objects, HUD shapes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlayfieldStats {
    pub sprites: SpriteStats,
    pub slider_bodies: u32,
}

impl PlayScene {
    /// The background dim the scene was prepared with.
    #[cfg(test)]
    pub(crate) fn dim(&self) -> f64 {
        self.background_view.settings().dim_level
    }

    /// The views the scene was prepared with.
    #[cfg(test)]
    pub(crate) fn view(&self) -> &PlayView {
        &self.view
    }

    /// Makes the play's GPU resources on the draw thread and waits up to `glyph_wait` for
    /// the HUD's glyphs and those of `fps`, the counter the play draws (a glyph takes
    /// several frames to appear; late ones show when ready). Navigation: allocates and
    /// uploads.
    pub fn start<D: Device>(
        &mut self,
        device: &mut D,
        glyph_wait: Duration,
        fps: Option<&FpsOverlay>,
    ) -> SceneGpu<D> {
        // The device made these with the same context before; failing now is a driver
        // fault the application does not recover from.
        // Three draws a frame (playfield and HUD, cursor, menu), each with its spare slots;
        // two of text (HUD, menu).
        let renderer = SpriteRenderer::new(device, 3 * DEFAULT_SLOTS).expect("renderer");
        let text_renderer =
            SpriteRenderer::with_shader(device, 2 * DEFAULT_SLOTS, &shaders::MSDF).expect("text");
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
        while self.hud.request_glyphs(&mut self.glyphs)
            + self.menu.request_glyphs(&mut self.glyphs)
            + fps.map_or(0, |overlay| overlay.request_glyphs(&mut self.glyphs))
            > 0
            && Instant::now() < deadline
        {
            self.glyphs.process(device, &mut textures);
            std::thread::sleep(Duration::from_millis(1));
        }
        SceneGpu {
            textures,
            renderer,
            text_renderer,
            sliders,
            slider_buffers,
        }
    }

    /// Draws the play with `r`, the resources its [`PlayScene::start`] made, into the
    /// frame `device` began at `size`, with the FPS counter `fps` on top (ADR-0060).
    #[allow(clippy::too_many_arguments)]
    pub fn draw<D: Device>(
        &mut self,
        r: &mut SceneGpu<D>,
        device: &mut D,
        size: Extent,
        now_ns: u64,
        snapshot: &PlaySnapshot,
        fps: Option<&FpsView>,
        over_layer: bool,
    ) {
        self.uploads.process(device, &mut r.textures);
        self.glyphs.process(device, &mut r.textures);
        let textures = self.textures.resolve(&self.uploads);
        let (width, height) = (size.width as f32, size.height as f32);
        let layout = PlayfieldLayout::new(width, height);
        self.hud.update(
            width,
            height,
            &snapshot.hud,
            snapshot.offset_panel.offset_ms,
        );
        self.hud.animate(snapshot, now_ns);
        self.menu.update(width, height, &snapshot.menu);
        self.batch.clear();
        self.text.clear();
        self.cursor.clear();
        self.menu_shapes.clear();
        self.menu_text.clear();
        let background = slam_play::background::shown_region(
            self.background.map(|h| self.uploads.get(h)),
            self.default_background.map(|h| self.uploads.get(h)),
        );
        let time = snapshot.time_at(now_ns);
        let fail = FailFrame::new(snapshot.fail, time, now_ns, width, height);
        self.background_view.push(
            &mut self.batch,
            [width, height],
            snapshot,
            now_ns,
            background,
            over_layer,
        );
        // The game layer: everything up to the menus (lazer's `FailAnimationContainer`
        // content), with a black box behind it once the play fails.
        let layer_first = self.batch.len();
        if let Some(fail) = &fail {
            let grey = fail.background_grey;
            self.batch
                .transform_from(0, &Affine::IDENTITY, [grey, grey, grey, 1.0]);
            if let Some(white) = textures.white {
                let mut black = Sprite::new(white.texture, white.uv, [0.0, 0.0], [width, height]);
                black.colour = Color::BLACK;
                self.batch.push(&black);
            }
        }
        self.view.push(
            &mut self.batch,
            &mut self.bodies,
            &layout,
            snapshot,
            now_ns,
            &textures,
        );
        self.hud.push_shapes(
            &mut self.batch,
            snapshot,
            now_ns,
            textures.white,
            textures.meter.as_ref(),
            &textures.hud,
        );
        self.hud.push_text(
            &mut self.text,
            snapshot,
            now_ns,
            &mut self.glyphs,
            &textures.hud,
        );
        self.view
            .push_cursor(&mut self.cursor, &layout, snapshot, now_ns, &textures);
        let layer = fail.map_or(Affine::IDENTITY, |fail| fail.layer);
        let layer_tint = fail.map_or([1.0; 4], |fail| fail.layer_tint());
        if fail.is_some() {
            self.batch.transform_from(layer_first, &layer, layer_tint);
            self.text.transform_from(0, &layer, layer_tint);
            self.cursor.transform_from(0, &layer, layer_tint);
        }
        if let Some(white) = textures.white {
            self.menu
                .push_shapes(&mut self.menu_shapes, &snapshot.menu, now_ns, white);
        }
        self.menu.push_text(
            &mut self.menu_text,
            &snapshot.menu,
            now_ns,
            &mut self.glyphs,
        );
        // Last: the counter is on top of the menus.
        if let Some(view) = fps {
            if let Some(white) = textures.white {
                view.push_shapes(&mut self.menu_shapes, white);
            }
            view.push_text(&mut self.menu_text, &mut self.glyphs);
        }
        r.renderer.begin_frame();
        r.text_renderer.begin_frame();
        let gpu_textures = &r.textures;
        let projection = Projection::pixels(size);
        // A failed draw loses one frame; the next one tries again.
        let texture = |id| gpu_textures.get(id);
        let _ = r.sliders.begin_frame(device, size);
        self.stats = PlayfieldStats::default();
        // The playfield's sprites in parts, with the slider bodies between them.
        if let Ok(uploaded) = r.renderer.upload(device, &self.batch, &projection, texture) {
            let mut from = 0;
            for body in self.bodies.as_slice() {
                if let Ok(stats) = r.renderer.draw_range(
                    device,
                    &uploaded,
                    &self.batch,
                    from..body.before_sprite,
                    texture,
                ) {
                    self.stats.sprites += stats;
                }
                from = body.before_sprite;
                let Some(Some(buffer)) = r.slider_buffers.get(body.index as usize) else {
                    continue;
                };
                let origin = layout.to_window(body.position);
                let path = projection
                    .transformed(&layer.then_after(&body.transform))
                    .scaled(layout.scale(), [origin.x, origin.y]);
                let tint = tint_srgb(body.tint, layer_tint);
                self.stats.slider_bodies += 1;
                let _ = r.sliders.draw(
                    device,
                    buffer,
                    &SliderDraw {
                        projection: &path,
                        radius: body.radius,
                        range: body.range,
                        tint,
                        gradient: self.view.sliders.visuals.gradient(body.gradient),
                    },
                    None,
                );
            }
            if let Ok(stats) =
                r.renderer
                    .draw_range(device, &uploaded, &self.batch, from..u32::MAX, texture)
            {
                self.stats.sprites += stats;
            }
        }
        let _ = r
            .text_renderer
            .draw(device, &self.text, &projection, texture);
        let _ = r.renderer.draw(device, &self.cursor, &projection, texture);
        if !self.menu_shapes.is_empty() {
            let _ = r
                .renderer
                .draw(device, &self.menu_shapes, &projection, texture);
            let _ = r
                .text_renderer
                .draw(device, &self.menu_text, &projection, texture);
        }
    }

    /// Whether every image of the play is uploaded (or failed) and every glyph drawn so far
    /// is ready: the next frame draws everything the scene has. For screenshots; walks the
    /// handles and glyphs.
    pub fn loaded(&self) -> bool {
        self.requests.all_finished(&self.uploads) && self.glyphs.all_ready()
    }

    /// What the last [`PlayScene::draw`] drew of the playfield batch.
    pub fn stats(&self) -> PlayfieldStats {
        self.stats
    }
}

impl<D: Device> SceneGpu<D> {
    /// Destroys the play's GPU resources. Navigation: frees.
    pub fn destroy(self, device: &mut D) {
        let Self {
            mut textures,
            renderer,
            text_renderer,
            mut sliders,
            slider_buffers,
        } = self;
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
