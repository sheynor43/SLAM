//! `slam-app ui-preview`: draws the interface screens with sample data on the headless
//! OpenGL of the screenshots and writes them as PNGs, to look at interface changes without
//! opening a window. Developer command: its console output is not localised.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use slam_play::{
    AudioFigures, FpsCorner, FpsCounter, FpsMode, FpsView, InputFigures, ThreadFigures,
};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::{Device, Extent};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::cursor::{CursorMotion, push_slam_cursor};
use slam_ui::i18n::Localizer;
use slam_ui::menu::{LogoAudio, MainMenu, MenuInput, MenuLayout, MenuSnapshot, MenuStyle};
use slam_ui::results::{Rank, ResultsData, ResultsMods, ResultsSnapshot, ResultsStat, StatKind};
use slam_ui::screen::Pose;
use slam_ui::settings_screen::SettingsScreen;
use slam_ui::song_select::beat::{BeatPoint, BeatTiming};
use slam_ui::song_select::{
    BeatmapDetails, DetailStats, DifficultyRow, LocalScores, MAX_LOCAL_SCORES, ScoreMods, ScoreRow,
    SearchSource, SetRow, SongFilter, SongList, SongSelect, SongSelectInput, SongSelectLayout,
    SongSelectStatus, Thumbs,
};
use slam_ui::theme::Theme;

use slam_render::upload::{TextureLoaderConfig, TextureRequests, TextureState, texture_loader};
use slam_skin::{Skin, SkinChain};
use std::time::{Duration, Instant};

use super::intro::{Gl, Gpu, draw_layers, settle, with_headless_gl};
use super::{Frame, REFERENCE_RENDERER};
use crate::ranking_skin::{MAX_RANKING_TEXTURES, RankingSkin, RankingView, load_chain};
use crate::settings::Settings;
use crate::shell::language::{InterfaceViews, interface_views};
use crate::shell::{Rows, SECTIONS};

pub const USAGE: &str =
    "usage: slam-app ui-preview --out DIR [--size WxH] [--any-renderer] [--skin DIR]";

/// Size of the previews unless given.
pub const DEFAULT_SIZE: (u32, u32) = (1280, 720);
/// The shapes' drift: the shell sets it from the frame's time, any fixed value keeps the
/// frames equal.
const DRIFT: f32 = 0.37;
/// The time the song select's screen is ticked to.
const NOW_NS: u64 = 5_000_000_000;

/// The `ui-preview` command's options.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub out: PathBuf,
    pub size: (u32, u32),
    pub any_renderer: bool,
    /// A skin folder: the `results-stable` scenes are drawn with its ranking images.
    pub skin: Option<PathBuf>,
}

impl Options {
    /// Parses the arguments after `ui-preview`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
        let mut args = args.into_iter();
        let (mut out, mut size, mut any_renderer, mut skin) = (None, DEFAULT_SIZE, false, None);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--out" => out = Some(PathBuf::from(value()?)),
                "--size" => size = super::parse_size(&value()?)?,
                "--any-renderer" => any_renderer = true,
                "--skin" => skin = Some(PathBuf::from(value()?)),
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(Options {
            out: out.ok_or("--out DIR is required")?,
            size,
            any_renderer,
            skin,
        })
    }
}

/// Draws every preview and writes them to `options.out`.
pub fn run(options: &Options) -> Result<(), String> {
    std::fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    let frames = render_with(options.size, options.any_renderer, options.skin.as_deref())?;
    for (name, frame) in frames {
        let path = options.out.join(format!("{name}.png"));
        frame.write_png(&path)?;
        println!("{}", path.display());
    }
    Ok(())
}

/// What one preview shows.
#[derive(Clone, Copy)]
enum Scene {
    /// The main menu (ADR-0186): the logo alone, or open in a style with the pointer over
    /// one of its buttons, on a loud beat.
    Menu {
        style: MenuStyle,
        open: bool,
    },
    /// The mod select panel open over the song select, a few mods on (ADR-0152).
    Mods,
    /// The replay toast over the song select, hovered (ADR-0154).
    Toast,
    SongSelect {
        effects: bool,
    },
    Settings,
    Cursor,
    /// The FPS counter in each of its modes, one per corner (ADR-0138).
    Fps,
    /// The volume overlay at an edge, a meter selected and sparks flying (ADR-0145).
    Volume(slam_ui::volume::Edge, slam_ui::volume::MeterStyle),
    /// SLAM's results screen; the flag picks the full combo play (ADR-0158).
    Results {
        full_combo: bool,
    },
    /// The results in the skin's own look; needs `--skin`.
    ResultsStable {
        full_combo: bool,
        /// Scrolled to the details page (ADR-0161).
        scrolled: bool,
    },
}

impl Scene {
    fn needs_skin(self) -> bool {
        matches!(self, Scene::ResultsStable { .. })
    }
}

/// The names of the PNGs and what they show, in the order they are drawn.
const SCENES: [(&str, Scene); 20] = [
    (
        "menu-logo",
        Scene::Menu {
            style: MenuStyle::Lazer,
            open: false,
        },
    ),
    (
        "menu-lazer",
        Scene::Menu {
            style: MenuStyle::Lazer,
            open: true,
        },
    ),
    (
        "menu-stable",
        Scene::Menu {
            style: MenuStyle::Stable,
            open: true,
        },
    ),
    ("song-select", Scene::SongSelect { effects: true }),
    ("mods", Scene::Mods),
    ("toast", Scene::Toast),
    (
        "song-select-noeffects",
        Scene::SongSelect { effects: false },
    ),
    ("settings", Scene::Settings),
    ("cursor", Scene::Cursor),
    ("fps", Scene::Fps),
    ("results", Scene::Results { full_combo: false }),
    (
        "results-stable",
        Scene::ResultsStable {
            full_combo: false,
            scrolled: false,
        },
    ),
    (
        "results-stable-perfect",
        Scene::ResultsStable {
            full_combo: true,
            scrolled: false,
        },
    ),
    (
        "results-stable-scrolled",
        Scene::ResultsStable {
            full_combo: false,
            scrolled: true,
        },
    ),
    (
        "volume-arc-left",
        Scene::Volume(
            slam_ui::volume::Edge::Left,
            slam_ui::volume::MeterStyle::Arc,
        ),
    ),
    (
        "volume-arc-bottom",
        Scene::Volume(
            slam_ui::volume::Edge::Bottom,
            slam_ui::volume::MeterStyle::Arc,
        ),
    ),
    (
        "volume-column-left",
        Scene::Volume(
            slam_ui::volume::Edge::Left,
            slam_ui::volume::MeterStyle::Column,
        ),
    ),
    (
        "volume-column-top",
        Scene::Volume(
            slam_ui::volume::Edge::Top,
            slam_ui::volume::MeterStyle::Column,
        ),
    ),
    (
        "volume-bar-right",
        Scene::Volume(
            slam_ui::volume::Edge::Right,
            slam_ui::volume::MeterStyle::Bar,
        ),
    ),
    (
        "volume-bar-bottom",
        Scene::Volume(
            slam_ui::volume::Edge::Bottom,
            slam_ui::volume::MeterStyle::Bar,
        ),
    ),
];

/// Draws the previews, named. Opens SDL video: one call at a time in a process, on its
/// main thread.
pub fn render(size: (u32, u32), any_renderer: bool) -> Result<Vec<(&'static str, Frame)>, String> {
    render_with(size, any_renderer, None)
}

/// [`render`] with the skin folder the `results-stable` scenes are drawn with; without one
/// they are skipped.
pub fn render_with(
    size: (u32, u32),
    any_renderer: bool,
    skin: Option<&Path>,
) -> Result<Vec<(&'static str, Frame)>, String> {
    let skin = skin.map(Path::to_path_buf);
    let extent = Extent::new(size.0, size.1);
    with_headless_gl(size, move |device| {
        let driver = device.driver_info();
        if !any_renderer && !driver.contains(REFERENCE_RENDERER) {
            return Err(format!(
                "the references are drawn on {REFERENCE_RENDERER}, not {driver}"
            ));
        }
        let mut gpu = Gpu::new(device)?;
        let frames = draw_scenes(device, &mut gpu, extent, skin.as_deref());
        gpu.destroy(device);
        frames
    })
}

fn draw_scenes(
    device: &mut Gl,
    gpu: &mut Gpu,
    size: Extent,
    skin: Option<&Path>,
) -> Result<Vec<(&'static str, Frame)>, String> {
    let mut state = State::new(device, size)?;
    if let Some(dir) = skin {
        state.stable = Some(load_ranking_skin(device, gpu, dir)?);
    }
    let background = state.views.theme.background;
    let has_skin = state.stable.is_some();
    SCENES
        .iter()
        .filter(|(name, scene)| {
            let skipped = scene.needs_skin() && !has_skin;
            if skipped {
                tracing::info!("{name}: skipped, pass --skin DIR to draw it");
            }
            !skipped
        })
        .map(|&(name, scene)| {
            let frame = settle(
                device,
                gpu,
                &mut state,
                |state| &mut state.glyphs,
                background,
                size,
                |device, gpu, state| state.draw(device, gpu, scene, size),
            )
            .map_err(|e| format!("{name}: {e}"))?;
            Ok((name, frame))
        })
        .collect()
}

struct State {
    glyphs: GlyphCache,
    settings: Settings,
    rows: Rows,
    views: InterfaceViews,
    /// The same views with the theme's effects and shadows off.
    plain_views: InterfaceViews,
    song_select: SongSelect,
    /// The FPS counter's views, one per mode, and the counter they show.
    fps: Vec<(FpsMode, FpsCorner, FpsView)>,
    fps_counter: FpsCounter,
    shapes: ShapeBatch,
    text: SpriteBatch,
    /// The skin's ranking images, loaded for `--skin`.
    stable: Option<(RankingView, RankingSkin, TextureRequests)>,
    results_en: Option<String>,
    results_extra: slam_ui::results::ResultsExtra,
}

impl State {
    fn new(device: &mut Gl, size: Extent) -> Result<State, String> {
        // Every text a difficulty row can show (ADR-0151).
        let theme = Theme {
            song_difficulty_info: slam_ui::song_select::DifficultyInfo::StarsAndPp,
            ..Theme::default()
        };
        let registry = Arc::new(FontRegistry::with_builtin());
        let localizer = Localizer::builtin().map_err(|e| e.to_string())?;
        let settings = Settings::default();
        let rows = Rows::new(&settings);
        let views = interface_views(&registry, theme, &localizer, &rows, &settings);
        let plain = theme.with_effects(false).with_shadow(0.0);
        let plain_views = interface_views(&registry, plain, &localizer, &rows, &settings);
        let labels = crate::play::scene::hud_labels(&localizer).fps;
        let fps = [
            (FpsMode::Fps, FpsCorner::TopLeft),
            (FpsMode::Draw, FpsCorner::TopRight),
            (FpsMode::Update, FpsCorner::BottomLeft),
            (FpsMode::Full, FpsCorner::BottomRight),
        ]
        .map(|(mode, corner)| {
            let mut view = FpsView::new(Arc::clone(&registry), &labels);
            view.set_mode(mode);
            view.set_corner(corner);
            (mode, corner, view)
        })
        .into();
        let glyphs = glyph_atlas(registry, &GlyphAtlasConfig::new(device.limits()))
            .map_err(|e| e.to_string())?;
        Ok(State {
            fps,
            fps_counter: sample_fps_counter(),
            glyphs,
            song_select: sample_song_select(&theme, size),
            settings,
            rows,
            views,
            plain_views,
            shapes: ShapeBatch::with_capacity(64),
            text: SpriteBatch::with_capacity(64),
            stable: None,
            results_en: Some(early_text(&localizer)),
            results_extra: sample_extra(&localizer),
        })
    }

    fn draw(
        &mut self,
        device: &mut Gl,
        gpu: &mut Gpu,
        scene: Scene,
        size: Extent,
    ) -> Result<(), String> {
        let (width, height) = (size.width as f32, size.height as f32);
        self.shapes.clear();
        self.text.clear();
        self.shapes.set_drift(DRIFT);
        let mut split = None;
        let white = gpu.white;
        match scene {
            Scene::Menu { style, open } => self.views.menu.push(
                &mut self.shapes,
                &mut self.text,
                &mut self.glyphs,
                width,
                height,
                &sample_menu(style, open, width, height, &self.views.theme),
                Pose::SHOWN,
                white,
                gpu.logo.as_ref(),
            ),
            Scene::SongSelect { effects } => {
                let views = if effects {
                    &mut self.views
                } else {
                    &mut self.plain_views
                };
                views.song_select.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    &self.song_select.snapshot(),
                    Pose::SHOWN,
                    white,
                    Thumbs {
                        logo: gpu.logo.as_ref(),
                        ..Thumbs::OFF
                    },
                );
            }
            Scene::Mods => {
                let mut screen = self.song_select.clone();
                *screen.mods_mut() =
                    slam_ui::song_select::ModSelect::new(crate::shell::mod_columns());
                screen.mods_mut().open();
                screen.mods_mut().set_active(1, 0, true);
                screen.mods_mut().set_active(1, 5, true);
                screen.mods_mut().set_active(3, 1, true);
                // The first frame starts the slide, the second is drawn when it is over.
                for now_ns in [0, 5_000_000_000] {
                    self.shapes.clear();
                    self.text.clear();
                    split = self
                        .views
                        .song_select
                        .push(
                            &mut self.shapes,
                            &mut self.text,
                            &mut self.glyphs,
                            width,
                            height,
                            &screen.snapshot(),
                            Pose::SHOWN,
                            white,
                            Thumbs {
                                now_ns,
                                ..Thumbs::OFF
                            },
                        )
                        .panel;
                }
            }
            Scene::Toast => {
                self.views.song_select.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    &self.song_select.snapshot(),
                    Pose::SHOWN,
                    white,
                    Thumbs {
                        logo: gpu.logo.as_ref(),
                        ..Thumbs::OFF
                    },
                );
                let frame = slam_ui::toast::ToastFrame {
                    visible: true,
                    since_ns: 1,
                    hover: true,
                    variant: slam_ui::toast::REPLAY_SAVED,
                };
                self.views.toast.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    &frame,
                    width,
                    height,
                    5_000_000_000,
                    white,
                );
            }
            Scene::Settings => {
                let mut screen = SettingsScreen::new(&self.rows.specs(&self.settings), SECTIONS);
                self.rows.refresh(&mut screen, &self.settings);
                self.views.settings.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    width,
                    height,
                    &screen.snapshot(),
                    Pose::SHOWN,
                    white,
                    0,
                );
            }
            Scene::Fps => {
                for (_, _, view) in &mut self.fps {
                    view.update(width, height, &self.fps_counter);
                    view.push_shapes(&mut self.shapes, &self.views.theme, white);
                    view.push_text(&mut self.text, &mut self.glyphs);
                }
            }
            Scene::Volume(edge, style) => {
                let frame = sample_volume(edge, style);
                self.views.volume.push(
                    &mut self.shapes,
                    &mut self.text,
                    &mut self.glyphs,
                    &frame,
                    width,
                    height,
                    VOLUME_NOW,
                    white,
                );
            }
            Scene::Results { full_combo }
            | Scene::ResultsStable {
                full_combo,
                scrolled: _,
            } => {
                let mut data = sample_results(full_combo, self.results_en.clone());
                data.extra = self.results_extra.clone();
                let scrolled = matches!(scene, Scene::ResultsStable { scrolled: true, .. });
                let snapshot =
                    ResultsSnapshot::new(Arc::new(data)).scrolled(if scrolled { 1.0 } else { 0.0 });
                let stable = match (scene, &self.stable) {
                    (Scene::ResultsStable { .. }, Some((view, skin, _))) => view.resolve(skin),
                    _ => None,
                };
                match stable {
                    Some(skin) => self.views.results.push_stable(
                        &mut self.shapes,
                        &mut self.text,
                        &mut self.glyphs,
                        width,
                        height,
                        &snapshot,
                        Pose::SHOWN,
                        white,
                        &skin,
                    ),
                    None if scene.needs_skin() => {
                        return Err("the skin's ranking images are not usable".to_owned());
                    }
                    None => self.views.results.push(
                        &mut self.shapes,
                        &mut self.text,
                        &mut self.glyphs,
                        width,
                        height,
                        &snapshot,
                        Pose::SHOWN,
                        white,
                    ),
                }
            }
            Scene::Cursor => {
                // At rest, pressed with a ring, turned, spun part-way, and with a trail.
                let mut trailing = CursorMotion::REST;
                for (i, slot) in trailing.trail.iter_mut().enumerate() {
                    *slot = (
                        [-(i as f32 + 1.0) * 18.0, -(i as f32 + 1.0) * 6.0],
                        0.9 - 0.15 * i as f32,
                    );
                }
                let mut pressed = CursorMotion {
                    scale: 0.9,
                    glow: 1.0,
                    ..CursorMotion::REST
                };
                pressed.ripples[0] = ([0.0; 2], 0.35);
                let motions = [
                    CursorMotion::REST,
                    pressed,
                    CursorMotion {
                        rotation: 40.0,
                        ..CursorMotion::REST
                    },
                    CursorMotion {
                        spin: 1.1,
                        ..CursorMotion::REST
                    },
                    trailing,
                ];
                for (i, motion) in motions.iter().enumerate() {
                    let tip = [width * (i as f32 + 1.0) / 6.0, height * 0.35];
                    push_slam_cursor(
                        &mut self.shapes,
                        &self.views.theme,
                        white,
                        tip,
                        90.0,
                        motion,
                    );
                }
            }
        }
        draw_layers(device, gpu, &self.shapes, &self.text, size, split)
    }
}

/// The skin's ranking images of the folder `dir` over the default skin, uploaded: blocks
/// until every image is in.
fn load_ranking_skin(
    device: &mut Gl,
    gpu: &mut Gpu,
    dir: &Path,
) -> Result<(RankingView, RankingSkin, TextureRequests), String> {
    let default = Skin::load_default(&crate::default_skin::dir()?).map_err(|e| e.to_string())?;
    let user = Skin::load(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let chain = SkinChain::new(Some(user), default);
    let config = TextureLoaderConfig {
        max_textures: MAX_RANKING_TEXTURES,
        request_capacity: 128,
        job_capacity: 2,
        ..TextureLoaderConfig::new(device.limits())
    };
    let (mut requests, mut uploads) = texture_loader(&config).map_err(|e| e.to_string())?;
    let skin = load_chain(&mut requests, &chain);
    if !skin.present {
        return Err(format!("{}: the skin has no ranking-panel", dir.display()));
    }
    let deadline = Instant::now() + super::LOAD_TIMEOUT;
    loop {
        uploads.process(device, &mut gpu.textures);
        let pending = skin
            .images
            .iter()
            .flatten()
            .any(|image| matches!(uploads.get(image.handle), TextureState::Pending));
        if !pending {
            break;
        }
        if Instant::now() > deadline {
            return Err("the skin's images did not load".to_owned());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    // The requests are kept: the loader ends with them.
    Ok((RankingView::new(Some(uploads), None), skin, requests))
}

/// The average hit error's text of the sample results.
fn early_text(localizer: &Localizer) -> String {
    let mut args = slam_ui::i18n::FluentArgs::new();
    args.set("ms", "1.50".to_owned());
    localizer
        .format("results-hit-error-early", Some(&args))
        .into_owned()
}

/// What stable's results add to the sample play: the mapper and player lines, a health
/// curve that dips below half once, and the detail lines (ADR-0161).
fn sample_extra(localizer: &Localizer) -> slam_ui::results::ResultsExtra {
    let text = |key: &str, args: &[(&str, &str)]| {
        let mut fluent = slam_ui::i18n::FluentArgs::new();
        for (name, value) in args {
            fluent.set(*name, (*value).to_owned());
        }
        localizer.format(key, Some(&fluent)).into_owned()
    };
    let health = (0..256)
        .map(|i| {
            let t = i as f32 / 255.0;
            let dip = (-((t - 0.55) / 0.08).powi(2)).exp() * 0.75;
            (0.95 - 0.1 * (t * 9.0).sin().abs() - dip).clamp(0.0, 1.0)
        })
        .collect();
    slam_ui::results::ResultsExtra {
        beatmap_by: text("results-beatmap-by", &[("mapper", "Jounzan")]),
        played_by: text(
            "results-played-by",
            &[("player", "Guest"), ("date", "2026-11-20 21:55:12")],
        ),
        health,
        details: vec![
            text("results-details-stars", &[("stars", "5.23")]),
            text("results-details-speed", &[("speed", "1.5")]),
            text(
                "results-details-difficulty",
                &[("cs", "4"), ("ar", "9.33"), ("od", "8.5"), ("hp", "6")],
            ),
            text(
                "results-details-error",
                &[("early", "-15.00"), ("late", "+4.00")],
            ),
        ],
    }
}

/// The sample play of the results scenes: rank A with 456 of 789 combo, or a full combo
/// with rank SH.
fn sample_results(full_combo: bool, average_text: Option<String>) -> ResultsData {
    let mut stats = [ResultsStat::default(); StatKind::COUNT];
    let mut set = |kind: StatKind, value| {
        stats[kind.index()] = ResultsStat { value, maximum: 0 };
    };
    set(StatKind::Great, 500);
    set(StatKind::Ok, 20);
    set(StatKind::Meh, 3);
    set(StatKind::Miss, if full_combo { 0 } else { 2 });
    ResultsData {
        id: 1,
        title: "Sample Artist - Sample Song [Sample Difficulty]".to_owned(),
        score: 1_234_567,
        accuracy: 0.9567,
        rank: if full_combo { Rank::SH } else { Rank::A },
        stats,
        combo: if full_combo { 789 } else { 456 },
        max_combo: 789,
        mods: ResultsMods::new(["HD", "DT"]),
        average_hit_error: Some(-1.5),
        average_hit_error_text: average_text,
        unstable_rate: Some(98.5),
        extra: slam_ui::results::ResultsExtra::default(),
    }
}

/// When the volume previews are drawn: well after the overlay showed.
const VOLUME_NOW: u64 = 10_000_000_000;

/// The volume overlay shown with the hitsounds empty, the music full, the master just
/// raised with a quick run of notches (sparks still flying) and the effects selected.
fn sample_volume(
    edge: slam_ui::volume::Edge,
    style: slam_ui::volume::MeterStyle,
) -> slam_ui::volume::VolumeFrame {
    const MS: u64 = 1_000_000;
    let mut mixer = slam_ui::volume::VolumeMixer::new();
    let mut levels = [0.0, 0.62, 0.7, 1.0];
    mixer.show(MS);
    for k in 0..6 {
        mixer.adjust(&mut levels, 1.0, false, VOLUME_NOW - 260 * MS + k * 40 * MS);
    }
    mixer.step_meter(false, VOLUME_NOW - 30 * MS);
    let mut frame = mixer.frame(edge, style, &levels);
    frame.since_ns = 0;
    frame
}

/// A counter that has watched a 240 Hz game with a few slow frames and ticks, and the
/// other threads running.
fn sample_fps_counter() -> FpsCounter {
    const MS: u64 = 1_000_000;
    let mut counter = FpsCounter::new();
    let mut now = 0;
    for frame in 0..1200_u64 {
        // 4.2 ms frames, with a slow one now and then.
        now += match frame % 97 {
            0 => 11 * MS,
            50 => 7 * MS,
            _ => 4_200_000,
        };
        let update = match frame % 131 {
            0 => 3 * MS,
            _ => 250_000 + (frame % 7) * 20_000,
        };
        counter.frame(now, update, 240.0, 4000.0);
    }
    counter.set_figures(ThreadFigures {
        update_hz: 3980.0,
        audio: Some(AudioFigures {
            callbacks_hz: 375.0,
            buffer_ms: 2.7,
            xruns: 0,
        }),
        input: Some(InputFigures {
            polls_hz: 1010.0,
            events_hz: 125.0,
        }),
    });
    counter
}

/// A song select with a sample list, a selected difficulty in an expanded set, one hovered
/// row and the details of the selected difficulty.
fn sample_song_select(theme: &Theme, size: Extent) -> SongSelect {
    /// Artist, title, creator and the difficulties with their stars.
    type Sample = (
        &'static str,
        &'static str,
        &'static str,
        &'static [(&'static str, f32)],
    );
    const SETS: [Sample; 6] = [
        (
            "Camellia",
            "Exit This Earth's Atomosphere",
            "Realazy",
            &[
                ("Easy", 1.6),
                ("Normal", 2.7),
                ("Hard", 4.1),
                ("Another", 6.3),
                ("Extra", 7.9),
            ],
        ),
        (
            "Kano",
            "Stella-rium",
            "Nathan",
            &[
                ("Beginner", 1.5),
                ("Advanced", 3.4),
                ("Insane", 5.5),
                ("Expert", 6.8),
            ],
        ),
        (
            "LeaF",
            "Doppelganger",
            "Hollow Wings",
            &[
                ("Normal", 2.9),
                ("Hard", 4.8),
                ("Insane", 6.1),
                ("Lunatic", 8.5),
            ],
        ),
        (
            "Mili",
            "Rain or Shine",
            "Kyouren",
            &[("Easy", 1.8), ("Normal", 3.0), ("Hard", 4.6)],
        ),
        (
            "Team Grimoire",
            "Rigid Mind",
            "Zerobyte",
            &[
                ("Hard", 4.4),
                ("Insane", 5.9),
                ("Another", 7.2),
                ("Extra", 8.2),
            ],
        ),
        (
            "xi",
            "Freedom Dive",
            "Nakagawa-Kanon",
            &[("Easy", 2.0), ("Hard", 4.9), ("FOUR DIMENSIONS", 7.6)],
        ),
    ];
    let sets = SETS
        .iter()
        .enumerate()
        .map(|(i, &(artist, title, creator, difficulties))| SetRow {
            key: i as u64 + 1,
            artist: artist.into(),
            title: title.into(),
            creator: creator.into(),
            difficulties: difficulties
                .iter()
                .enumerate()
                .map(|(d, &(version, stars))| {
                    let mut row = DifficultyRow::new(SearchSource {
                        difficulty: version,
                        creator,
                        artist,
                        title,
                        ..SearchSource::default()
                    });
                    row.key = (i * 10 + d) as u64 + 1;
                    row.stars = Some(stars);
                    row.pp = Some(stars * stars * 31.0);
                    row.approach_rate = 7.0 + d as f32 * 0.6;
                    row.overall_difficulty = 6.0 + d as f32 * 0.5;
                    row
                })
                .collect(),
        })
        .collect();
    let list = Arc::new(SongList { sets });
    let filter = SongFilter::for_list(&list);
    let mut screen = SongSelect::new();
    screen.set_list(list, filter);
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(size.width as f32, size.height as f32, theme)
        .with_mod_columns(&crate::shell::mod_columns());
    // The second set, its third difficulty.
    for input in [
        SongSelectInput::Right,
        SongSelectInput::Down,
        SongSelectInput::Down,
    ] {
        screen.input(input, &layout);
    }
    screen.set_details(Some(sample_details()));
    // The pointer over the row two below the selected one: the rows above the selected set
    // are one each, the selected set has its row and its difficulties.
    let (selected_set, selected_difficulty) = screen.selected().unwrap_or((0, 0));
    let hovered = selected_set + 1 + selected_difficulty + 2;
    // The selection centres the list: let it get there first.
    screen.tick(NOW_NS, &layout);
    let snap = screen.snapshot();
    let y = layout.row_y(&snap.geometry, hovered, snap.shown_scroll)
        + layout.plate_height(&snap.geometry, hovered) / 2.0;
    let x = layout.list.x + layout.list.width / 2.0 + layout.lean_x(y);
    screen.input(SongSelectInput::PointerMove { x, y }, &layout);
    screen.tick(NOW_NS, &layout);
    screen
}

fn sample_details() -> BeatmapDetails {
    let score = |total_score, accuracy, rank: &[u8; 2], mods: &[&str]| {
        Some(ScoreRow {
            total_score,
            accuracy,
            max_combo: 1234,
            rank: *rank,
            mods: ScoreMods::from_acronyms(mods.iter().copied()),
            date_ms: 1_794_681_000_000,
            ..Default::default()
        })
    };
    let mut rows = [None; MAX_LOCAL_SCORES];
    rows[0] = score(9_876_543, 0.9912, b"SH", &["HD", "DT"]);
    rows[1] = score(8_123_456, 0.9731, b"S\0", &["HD"]);
    rows[2] = score(5_432_100, 0.9205, b"A\0", &[]);
    BeatmapDetails {
        circle_size: 4.0,
        approach_rate: 9.3,
        overall_difficulty: 8.0,
        drain_rate: 5.0,
        stats: Some(DetailStats {
            length_ms: 205_400,
            bpm_minimum: 180,
            bpm_maximum: 180,
            bpm_most_common: 180,
            circles: 512,
            sliders: 300,
            spinners: 2,
        }),
        scores: Some(LocalScores { rows }),
    }
}

/// The main menu a moment after a loud beat (ADR-0186): its buttons out in `style` with the
/// pointer on the second, when `open`.
fn sample_menu(
    style: MenuStyle,
    open: bool,
    width: f32,
    height: f32,
    theme: &Theme,
) -> MenuSnapshot {
    let layout = MenuLayout::new(width, height, theme);
    let mut menu = MainMenu::new();
    menu.set_style(style);
    let mut timing = BeatTiming::default();
    timing.push_point(BeatPoint {
        time: 0.0,
        beat_length: 500.0,
        meter: 4,
    });
    timing.push_kiai(0.0, 10_000.0);
    // A falling spectrum, as music has.
    let spectrum: [f32; slam_audio::SPECTRUM_BINS] =
        std::array::from_fn(|i| 0.45 / (1.0 + i as f32 / 8.0));
    const MS: u64 = 1_000_000;
    if open {
        menu.input(MenuInput::Up, &layout, 0);
        menu.input(MenuInput::Down, &layout, 0);
    }
    let end = 1960;
    for ms in 0..end {
        let audio = LogoAudio {
            track_ms: Some(ms as f64),
            timing: Some(&timing),
            spectrum: Some(&spectrum),
        };
        menu.tick(ms * MS, &audio, theme.effects);
        if open && ms == 1500 {
            let snapshot = menu.snapshot(&layout, ms * MS);
            let [x, y] = snapshot.buttons[1].rect.centre();
            menu.input(MenuInput::PointerMove { x, y }, &layout, ms * MS);
        }
    }
    menu.snapshot(&layout, end * MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn parses_out_size_and_renderer() {
        let options = parse(&["--out", "d", "--size", "640x360", "--any-renderer"]).unwrap();
        assert_eq!(options.out, PathBuf::from("d"));
        assert_eq!(options.size, (640, 360));
        assert!(options.any_renderer);
        assert_eq!(parse(&["--out", "d"]).unwrap().size, DEFAULT_SIZE);
        assert_eq!(options.skin, None);
        let with_skin = parse(&["--out", "d", "--skin", "s"]).unwrap();
        assert_eq!(with_skin.skin, Some(PathBuf::from("s")));
        assert!(parse(&["--out", "d", "--skin"]).is_err());
        assert!(parse(&[]).is_err());
        assert!(parse(&["--out", "d", "--bogus"]).is_err());
    }

    /// Draws every preview on whatever OpenGL there is.
    #[test]
    #[ignore = "opens a headless OpenGL window"]
    fn draws_every_preview() {
        let frames = render((640, 360), true).unwrap();
        // The stable scenes need a skin.
        assert_eq!(frames.len(), SCENES.len() - 2);
        for (name, frame) in &frames {
            let first = &frame.pixels[..4];
            assert!(
                frame.pixels.chunks(4).any(|p| p != first),
                "{name} is one flat colour"
            );
        }
    }
}
