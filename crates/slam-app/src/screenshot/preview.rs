//! `slam-app ui-preview`: draws the interface screens with sample data on the headless
//! OpenGL of the screenshots and writes them as PNGs, to look at interface changes without
//! opening a window. Developer command: its console output is not localised.

use std::path::PathBuf;
use std::sync::Arc;

use slam_render::shape::ShapeBatch;
use slam_render::sprite::SpriteBatch;
use slam_render::{Device, Extent};
use slam_text::{FontRegistry, GlyphAtlasConfig, GlyphCache, glyph_atlas};
use slam_ui::cursor::{CursorMotion, push_slam_cursor};
use slam_ui::i18n::Localizer;
use slam_ui::menu::MenuSnapshot;
use slam_ui::screen::Pose;
use slam_ui::settings_screen::SettingsScreen;
use slam_ui::song_select::{
    BeatmapDetails, DetailStats, DifficultyRow, LocalScores, MAX_LOCAL_SCORES, ScoreMods, ScoreRow,
    SearchSource, SetRow, SongFilter, SongList, SongSelect, SongSelectInput, SongSelectLayout,
    SongSelectStatus, Thumbs,
};
use slam_ui::theme::Theme;

use super::intro::{Gl, Gpu, draw_layer, settle, with_headless_gl};
use super::{Frame, REFERENCE_RENDERER};
use crate::settings::Settings;
use crate::shell::language::{InterfaceViews, interface_views};
use crate::shell::{Rows, SECTIONS};

pub const USAGE: &str = "usage: slam-app ui-preview --out DIR [--size WxH] [--any-renderer]";

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
}

impl Options {
    /// Parses the arguments after `ui-preview`.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
        let mut args = args.into_iter();
        let (mut out, mut size, mut any_renderer) = (None, DEFAULT_SIZE, false);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--out" => out = Some(PathBuf::from(value()?)),
                "--size" => size = super::parse_size(&value()?)?,
                "--any-renderer" => any_renderer = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(Options {
            out: out.ok_or("--out DIR is required")?,
            size,
            any_renderer,
        })
    }
}

/// Draws every preview and writes them to `options.out`.
pub fn run(options: &Options) -> Result<(), String> {
    std::fs::create_dir_all(&options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    let frames = render(options.size, options.any_renderer)?;
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
    Menu(usize),
    SongSelect { effects: bool },
    Settings,
    Cursor,
}

/// The names of the PNGs and what they show, in the order they are drawn.
const SCENES: [(&str, Scene); 6] = [
    ("menu-play", Scene::Menu(0)),
    ("menu-settings", Scene::Menu(1)),
    ("song-select", Scene::SongSelect { effects: true }),
    (
        "song-select-noeffects",
        Scene::SongSelect { effects: false },
    ),
    ("settings", Scene::Settings),
    ("cursor", Scene::Cursor),
];

/// Draws the previews, named. Opens SDL video: one call at a time in a process, on its
/// main thread.
pub fn render(size: (u32, u32), any_renderer: bool) -> Result<Vec<(&'static str, Frame)>, String> {
    let extent = Extent::new(size.0, size.1);
    with_headless_gl(size, move |device| {
        let driver = device.driver_info();
        if !any_renderer && !driver.contains(REFERENCE_RENDERER) {
            return Err(format!(
                "the references are drawn on {REFERENCE_RENDERER}, not {driver}"
            ));
        }
        let mut gpu = Gpu::new(device)?;
        let frames = draw_scenes(device, &mut gpu, extent);
        gpu.destroy(device);
        frames
    })
}

fn draw_scenes(
    device: &mut Gl,
    gpu: &mut Gpu,
    size: Extent,
) -> Result<Vec<(&'static str, Frame)>, String> {
    let mut state = State::new(device, size)?;
    let background = state.views.theme.background;
    SCENES
        .iter()
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
    shapes: ShapeBatch,
    text: SpriteBatch,
}

impl State {
    fn new(device: &mut Gl, size: Extent) -> Result<State, String> {
        let theme = Theme::default();
        let registry = Arc::new(FontRegistry::with_builtin());
        let localizer = Localizer::builtin().map_err(|e| e.to_string())?;
        let settings = Settings::default();
        let rows = Rows::new(&settings);
        let views = interface_views(&registry, theme, &localizer, &rows, &settings);
        let plain = theme.with_effects(false).with_shadow(0.0);
        let plain_views = interface_views(&registry, plain, &localizer, &rows, &settings);
        let glyphs = glyph_atlas(registry, &GlyphAtlasConfig::new(device.limits()))
            .map_err(|e| e.to_string())?;
        Ok(State {
            glyphs,
            song_select: sample_song_select(&theme, size),
            settings,
            rows,
            views,
            plain_views,
            shapes: ShapeBatch::with_capacity(64),
            text: SpriteBatch::with_capacity(64),
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
        let white = gpu.white;
        match scene {
            Scene::Menu(selected) => self.views.menu.push(
                &mut self.shapes,
                &mut self.text,
                &mut self.glyphs,
                width,
                height,
                &MenuSnapshot { selected },
                Pose::SHOWN,
                white,
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
                    Thumbs::OFF,
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
                );
            }
            Scene::Cursor => {
                // At rest, pressed, and turned.
                let motions = [
                    CursorMotion::REST,
                    CursorMotion {
                        scale: 0.9,
                        glow: 1.0,
                        rotation: 0.0,
                    },
                    CursorMotion {
                        scale: 1.0,
                        glow: 0.0,
                        rotation: 40.0,
                    },
                ];
                for (i, motion) in motions.iter().enumerate() {
                    let tip = [width * (i as f32 + 1.0) / 4.0, height * 0.35];
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
        draw_layer(device, gpu, &self.shapes, &self.text, size)
    }
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
    let layout = SongSelectLayout::new(size.width as f32, size.height as f32, theme);
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
    let y = layout.row_y(hovered, 0.0) + layout.row_height / 2.0;
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
        assert!(parse(&[]).is_err());
        assert!(parse(&["--out", "d", "--bogus"]).is_err());
    }

    /// Draws every preview on whatever OpenGL there is.
    #[test]
    #[ignore = "opens a headless OpenGL window"]
    fn draws_every_preview() {
        let frames = render((640, 360), true).unwrap();
        assert_eq!(frames.len(), SCENES.len());
        for (name, frame) in &frames {
            let first = &frame.pixels[..4];
            assert!(
                frame.pixels.chunks(4).any(|p| p != first),
                "{name} is one flat colour"
            );
        }
    }
}
