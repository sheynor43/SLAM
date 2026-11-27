//! The song select's steady state does not touch the allocator: input and snapshots on the
//! update side, `SongSelectView::push` on the draw side once the rows in view are cached.

use std::sync::Arc;

use std::ops::Range;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::{EDGE_MARGIN, ShapeBatch};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::textures::Textures;
use slam_render::{
    BufferDesc, Color, DescError, Device, DeviceError, DrawError, IndexFormat, PipelineDesc,
    RenderTargetDesc, TextureDesc,
};
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::screen::Pose;
use slam_ui::song_select::{
    DifficultyRow, MAX_VISIBLE_ROWS, ModColumns, QUERY_CAPACITY, RowThumb, SearchSource, SetRow,
    SettingRow, SettingRowKind, SongFilter, SongList, SongSelect, SongSelectInput,
    SongSelectLabels, SongSelectLayout, SongSelectStatus, SongSelectView, Thumbs,
};
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

const MOD_COLUMNS: ModColumns = [
    &["EZ", "NF", "HT", "DC"],
    &["HR", "SD", "PF"],
    &["AT", "RX"],
    &["DA", "CL"],
    &[],
];

fn list_of(count: u64) -> Arc<SongList> {
    list_rated(count, 1.0)
}

/// `list_of` with every rating and performance times `factor`: the same rows, as a mod
/// that changes the difficulty would republish them.
fn list_rated(count: u64, factor: f32) -> Arc<SongList> {
    Arc::new(SongList {
        sets: (0..count)
            .map(|k| SetRow {
                key: k,
                artist: format!("artist {k}"),
                title: format!("title {k}"),
                creator: "mapper".into(),
                difficulties: (0..3)
                    .map(|d| {
                        let mut row = DifficultyRow::new(SearchSource {
                            difficulty: &format!("diff {d}"),
                            creator: "mapper",
                            artist: &format!("artist {k}"),
                            title: &format!("title {k}"),
                            ..SearchSource::default()
                        });
                        // Two rated, one not yet: pills, dots and the accent fallback.
                        // Rows are known by their keys, as the rolling numbers are.
                        row.key = 1 + k * 3 + d;
                        row.stars = (d < 2).then_some((1.5 + d as f32 * 2.7) * factor);
                        row.pp = row.stars.map(|s| s * s * 31.0);
                        row
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// Gives `screen` `list` with filter buffers made for it, as the application does.
fn put(screen: &mut SongSelect, list: Arc<SongList>) {
    let filter = SongFilter::for_list(&list);
    screen.set_list(list, filter);
}

#[test]
fn input_does_not_allocate() {
    let list = list_of(40);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SongSelect::new();
    put(&mut screen, list);
    assert_no_alloc(|| {
        for input in [
            SongSelectInput::Down,
            SongSelectInput::Up,
            SongSelectInput::Right,
            SongSelectInput::PageDown,
            SongSelectInput::End,
            SongSelectInput::Home,
            SongSelectInput::Scroll { lines: 2.0 },
            SongSelectInput::PointerMove { x: 900.0, y: 300.0 },
            SongSelectInput::PointerPress { x: 900.0, y: 300.0 },
            SongSelectInput::Activate,
            SongSelectInput::Back,
        ] {
            std::hint::black_box(screen.input(input, &layout));
        }
        std::hint::black_box(screen.snapshot());
    });
}

const LIMITS: Limits = Limits {
    max_texture_size: 4096,
    max_uniform_block_size: 16384,
    max_uniform_buffer_slots: 12,
    max_texture_slots: 16,
};

fn white() -> AtlasRegion {
    AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: Extent::new(1, 1),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
    }
}

fn labels() -> SongSelectLabels {
    SongSelectLabels {
        no_folder: "no folder".into(),
        failed: "failed".into(),
        loading: "loading".into(),
        empty: "empty".into(),
        choose_folder: "choose folder".into(),
        no_audio: "no audio".into(),
        footer_back: "Back".into(),
        footer_pin: "+".into(),
        footer_reset: "Reset mods".into(),
        pp_suffix: "pp".into(),
        search_placeholder: "search...".into(),
        search_no_matches: "no matches".into(),
        mods_title: "Mods".into(),
        mods_hint: "mods hint".into(),
        mods_multiplier: "multiplier".into(),
        mods_customise: "Customise".into(),
        setting_texts: vec![
            ("mod-a", "Mod A".into()),
            ("setting-number", "Number".into()),
            ("setting-toggle", "Toggle".into()),
            ("setting-choice", "Choice".into()),
            ("value-a", "Value A".into()),
        ],
        mod_column_titles: [
            "Reduction".into(),
            "Increase".into(),
            "Automation".into(),
            "Conversion".into(),
            "Fun".into(),
        ],
        mod_texts: MOD_COLUMNS.map(|column| {
            column
                .iter()
                .map(|a| (format!("Mod {a}"), format!("What {a} does to the play")))
                .collect()
        }),
        mod_columns: MOD_COLUMNS,
        // The longest labels: Russian.
        details: slam_ui::song_select::DetailLabels {
            length: "Длина".into(),
            bpm: "BPM".into(),
            circles: "Круги".into(),
            sliders: "Слайдеры".into(),
            spinners: "Спиннеры".into(),
            circle_size: "CS".into(),
            approach_rate: "AR".into(),
            overall_difficulty: "OD".into(),
            drain_rate: "HP".into(),
            scores: "Локальные рекорды".into(),
            no_scores: "Рекордов нет".into(),
            accuracy: "ACCURACY".into(),
            combo: "MAX COMBO".into(),
        },
    }
}

struct Rig {
    view: SongSelectView,
    glyphs: slam_text::GlyphCache,
    shapes: ShapeBatch,
    text: SpriteBatch,
    thumbs: bool,
    full: bool,
    rows: [Option<RowThumb>; MAX_VISIBLE_ROWS],
    now_ns: u64,
}

impl Rig {
    fn new() -> Rig {
        Rig::with_theme(Theme {
            song_difficulty_info: slam_ui::song_select::DifficultyInfo::StarsAndPp,
            ..Theme::default()
        })
    }

    fn with_theme(theme: Theme) -> Rig {
        let registry = Arc::new(FontRegistry::with_builtin());
        let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
        Rig {
            // Stars and pp: every text a difficulty row can show (ADR-0151).
            view: SongSelectView::new(registry, theme, &labels()),
            glyphs,
            shapes: ShapeBatch::with_capacity(256),
            text: SpriteBatch::with_capacity(8192),
            thumbs: false,
            full: false,
            rows: [None; MAX_VISIBLE_ROWS],
            now_ns: 0,
        }
    }

    fn frame(&mut self, snapshot: &slam_ui::song_select::SongSelectSnapshot) {
        self.shapes.clear();
        self.text.clear();
        let pose = Pose {
            opacity: 0.5,
            offset: 0.05,
            scale: 1.0,
        };
        self.view.push(
            &mut self.shapes,
            &mut self.text,
            &mut self.glyphs,
            1280.0,
            720.0,
            snapshot,
            pose,
            white(),
            Thumbs {
                enabled: self.thumbs,
                full: self.full,
                rows: &self.rows,
                now_ns: self.now_ns,
            },
        );
    }
}

#[test]
fn view_pushes_without_allocating_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(40));
    screen.set_status(SongSelectStatus::Scanning { sets: 40 });
    let snapshot = screen.snapshot();
    rig.frame(&snapshot);
    rig.frame(&snapshot);
    assert_no_alloc(|| {
        for _ in 0..10 {
            rig.frame(&snapshot);
        }
    });
    assert!(rig.shapes.len() > 4);
    // A full list stays well within the shell's frame budget for two layers (ADR-0136).
    assert!(
        rig.shapes.len() <= SONG_SELECT_SHAPE_BUDGET,
        "{}",
        rig.shapes.len()
    );
}

#[test]
fn rolling_ratings_and_pp_do_not_allocate() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(40));
    screen.set_status(SongSelectStatus::Ready);
    let before = screen.snapshot();
    let ms = 1_000_000;
    put(&mut screen, list_rated(40, 2.37));
    let after = screen.snapshot();
    // The widest numbers are shown once, so the buffers have their room.
    rig.now_ns = 1_000 * ms;
    rig.frame(&after);
    rig.frame(&after);
    rig.now_ns = 5_000 * ms;
    rig.frame(&after);
    rig.frame(&before);
    // The rows come again with other ratings: their numbers roll for 750 ms.
    assert_no_alloc(|| {
        for step in 0..60u64 {
            rig.now_ns = (5_100 + step * 16) * ms;
            rig.frame(&before);
        }
    });
}

/// Shapes a song select frame may take: under half the shell's `MAX_SHAPES` (1024), so a
/// transition's two layers and the overlays fit.
const SONG_SELECT_SHAPE_BUDGET: usize = 400;

#[test]
fn changing_lists_and_scanning_texts_do_not_allocate() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    screen.set_status(SongSelectStatus::Scanning { sets: 1 });
    // Same lengths each time, so the warm buffers suffice. The test keeps every Arc alive,
    // so dropping a snapshot only decrements a count.
    let keep: Vec<_> = (0..6u32)
        .map(|i| {
            screen.set_scanning_text(Arc::from(format!("Scanning songs: {i:04} sets")));
            put(&mut screen, list_of(40));
            screen.snapshot()
        })
        .collect();
    for snapshot in &keep {
        rig.frame(snapshot);
    }
    assert_no_alloc(|| {
        for snapshot in keep.iter().cycle().take(30) {
            rig.frame(snapshot);
        }
    });
}

#[test]
fn scrolling_a_long_list_does_not_allocate_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(2000));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let pass = |rig: &mut Rig, screen: &mut SongSelect| {
        screen.input(SongSelectInput::Home, &layout);
        for _ in 0..2000 {
            screen.input(SongSelectInput::Scroll { lines: 0.5 }, &layout);
            rig.frame(&screen.snapshot());
        }
    };
    pass(&mut rig, &mut screen);
    slam_testkit::assert_no_alloc(|| pass(&mut rig, &mut screen));
}

#[test]
fn mod_panel_input_does_not_allocate() {
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(10));
    *screen.mods_mut() = slam_ui::song_select::ModSelect::new(MOD_COLUMNS);
    let [x, y] = layout.mods.tile_rect(1, 1).centre();
    assert_no_alloc(|| {
        for input in [
            SongSelectInput::ToggleMods,
            SongSelectInput::ModHotkey {
                column: 0,
                index: 1,
            },
            SongSelectInput::ModHotkey {
                column: 2,
                index: 0,
            },
            SongSelectInput::PointerMove { x, y },
            SongSelectInput::PointerPress { x, y },
            SongSelectInput::DeselectMods,
            SongSelectInput::Down,
            SongSelectInput::Back,
            SongSelectInput::ToggleMods,
            SongSelectInput::Activate,
            SongSelectInput::DeselectMods,
        ] {
            std::hint::black_box(screen.input(input, &layout));
            screen.mods_mut().set_active(0, 1, true);
            std::hint::black_box(screen.snapshot());
        }
    });
}

#[test]
fn mod_panel_and_selected_line_push_without_allocating_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(10));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    *screen.mods_mut() = slam_ui::song_select::ModSelect::new(MOD_COLUMNS);
    let [x, y] = layout.mods.tile_rect(0, 0).centre();
    // Every shape of the view once, so the buffers have grown.
    let states: Vec<_> = (0..4)
        .map(|i| {
            screen.mods_mut().set_active(0, i % 3, i % 2 == 0);
            screen.mods_mut().set_active(1, 1, true);
            screen.input(SongSelectInput::PointerMove { x, y }, &layout);
            if i % 2 == 0 {
                screen.mods_mut().open();
            } else {
                screen.mods_mut().close();
            }
            screen.snapshot()
        })
        .collect();
    for _ in 0..2 {
        for snapshot in &states {
            rig.frame(snapshot);
        }
    }
    assert_no_alloc(|| {
        for snapshot in states.iter().cycle().take(40) {
            rig.frame(snapshot);
        }
    });
}

fn setting_rows() -> [SettingRow; 5] {
    let row = |label, kind| SettingRow {
        label,
        suffix: "x",
        kind,
    };
    [
        row("mod-a", SettingRowKind::Header),
        row(
            "setting-number",
            SettingRowKind::Number {
                value: 1.25,
                min: 0.5,
                max: 2.0,
                step: 0.05,
                decimals: 2,
                unset: false,
            },
        ),
        row("setting-toggle", SettingRowKind::Toggle { value: true }),
        row(
            "setting-choice",
            SettingRowKind::Choice {
                index: Some(0),
                count: 2,
                values: ["value-a", "value-a", ""],
            },
        ),
        row(
            "setting-number",
            SettingRowKind::Number {
                value: 5.0,
                min: 0.0,
                max: 10.0,
                step: 0.1,
                decimals: 1,
                unset: true,
            },
        ),
    ]
}

#[test]
fn settings_panel_input_does_not_allocate() {
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(10));
    *screen.mods_mut() = slam_ui::song_select::ModSelect::new(MOD_COLUMNS);
    let rows = setting_rows();
    // The panel opens where the pointer is (ADR-0177).
    let [hx, hy] = layout.mods.customise.header.centre();
    let at = layout.mods.customise.at((hx, hy), rows.len());
    let track = at.slider(&at.row_rect(1)).hit;
    assert_no_alloc(|| {
        screen.mods_mut().set_setting_rows(&rows);
        for (now, input) in [
            SongSelectInput::ToggleMods,
            SongSelectInput::PointerMove { x: hx, y: hy },
            SongSelectInput::ToggleCustomisation,
            SongSelectInput::Down,
            SongSelectInput::Right,
            SongSelectInput::Left,
            SongSelectInput::Down,
            SongSelectInput::Space,
            SongSelectInput::Activate,
            SongSelectInput::ResetSetting,
            SongSelectInput::Scroll { lines: 1.0 },
            SongSelectInput::PointerPress {
                x: track.x + 10.0,
                y: track.y + 2.0,
            },
            SongSelectInput::PointerMove { x: 5.0, y: 600.0 },
            SongSelectInput::PointerRelease,
            // Drag the panel by its header, then close it with a right click.
            SongSelectInput::PointerPress {
                x: at.header.x + 8.0,
                y: at.header.y + 4.0,
            },
            SongSelectInput::PointerMove { x: 300.0, y: 300.0 },
            SongSelectInput::PointerRelease,
            SongSelectInput::SecondaryPress { x: 300.0, y: 300.0 },
            SongSelectInput::ToggleCustomisation,
            SongSelectInput::ToggleCustomisation,
        ]
        .into_iter()
        .enumerate()
        {
            std::hint::black_box(screen.input(input, &layout));
            screen.tick(now as u64 * 100_000_000, &layout);
            screen.mods_mut().set_setting_rows(&rows);
            std::hint::black_box(screen.snapshot());
        }
    });
}

#[test]
fn settings_panel_pushes_without_allocating_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(10));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    *screen.mods_mut() = slam_ui::song_select::ModSelect::new(MOD_COLUMNS);
    let mut rows = setting_rows();
    screen.mods_mut().set_setting_rows(&rows);
    screen.input(SongSelectInput::ToggleMods, &layout);
    screen.input(SongSelectInput::ToggleCustomisation, &layout);
    // The values the number rows go through, so that every text has been laid out.
    let states: Vec<_> = (0..4)
        .map(|i| {
            for row in &mut rows {
                if let SettingRowKind::Number { value, unset, .. } = &mut row.kind {
                    *value += 0.25 * f64::from(i);
                    *unset = i % 2 == 0;
                }
            }
            screen.mods_mut().set_setting_rows(&rows);
            screen.input(SongSelectInput::Down, &layout);
            screen.snapshot()
        })
        .collect();
    for _ in 0..2 {
        for snapshot in &states {
            rig.frame(snapshot);
        }
    }
    assert!(rig.shapes.len() > 15);
    assert_no_alloc(|| {
        for snapshot in states.iter().cycle().take(40) {
            rig.frame(snapshot);
        }
    });
}

#[test]
fn searching_does_not_allocate() {
    let list = list_of(2000);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SongSelect::new();
    put(&mut screen, Arc::clone(&list));
    // A list change with a query keeps it; its buffers are made before, as the library
    // thread does.
    let next = SongFilter::for_list(&list);
    let mut old = SongFilter::default();
    assert_no_alloc(|| {
        for input in [
            SongSelectInput::Type('t'),
            SongSelectInput::Type('i'),
            SongSelectInput::Type(' '),
            SongSelectInput::Type('\u{44b}'),
            SongSelectInput::Erase,
            SongSelectInput::Erase,
            SongSelectInput::Type('1'),
            SongSelectInput::Down,
            SongSelectInput::PageDown,
            SongSelectInput::DeselectMods,
            SongSelectInput::Type('9'),
            SongSelectInput::Type('9'),
            SongSelectInput::Type('9'),
            SongSelectInput::Type('9'),
            SongSelectInput::PointerPress { x: 900.0, y: 300.0 },
            SongSelectInput::Back,
            SongSelectInput::Type('d'),
            SongSelectInput::Back,
            SongSelectInput::Back,
        ] {
            std::hint::black_box(screen.input(input, &layout));
            std::hint::black_box(screen.snapshot());
        }
        // The query language (ADR-0099): every kind of filter, typed one character at a
        // time, each re-filtering the list.
        for c in "ar>=5 cs<9 od!=3 hp:5 star>1 bpm=180 length<3:30 status=r,l \
                  lastplayed<1y2M played=0 ranked>=2012.03 divisor:4 \
                  creator!=\"x y\"! artist=artist title=\"title 1\" source=s tag=t tag!=u \
                  diff=d \"artist 1\" \"title\"! 42 [diff 1]"
            .chars()
        {
            std::hint::black_box(screen.input(SongSelectInput::Type(c), &layout));
        }
        for _ in 0..QUERY_CAPACITY {
            screen.input(SongSelectInput::Erase, &layout);
        }
        assert!(screen.query().is_empty());
        screen.input(SongSelectInput::Type('t'), &layout);
        old = screen.set_list(Arc::clone(&list), next);
        std::hint::black_box(screen.snapshot());
    });
    assert_eq!(screen.query().as_str(), "t");
    drop(old);
}

#[test]
fn pushing_with_a_query_does_not_allocate_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(200));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    // Typed text, an empty query, and a query that matches nothing, each once to warm up.
    let mut states = Vec::new();
    for text in ["title 1", "title 12", "no such title"] {
        for c in text.chars() {
            screen.input(SongSelectInput::Type(c), &layout);
        }
        states.push(screen.snapshot());
        screen.input(SongSelectInput::Back, &layout);
        states.push(screen.snapshot());
    }
    for _ in 0..2 {
        for snapshot in &states {
            rig.frame(snapshot);
        }
    }
    assert!(states.iter().any(|s| s.row_count == 0));
    assert_no_alloc(|| {
        for snapshot in states.iter().cycle().take(60) {
            rig.frame(snapshot);
        }
    });
}

#[test]
fn rows_scrolled_part_way_stay_inside_the_list() {
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let (top, bottom) = (layout.list.y, layout.list.y + layout.list.height);
    // Vertices above and below the list, at a scroll of `lines` rows.
    let outside = |lines: f32| {
        let mut rig = Rig::new();
        let mut screen = SongSelect::new();
        put(&mut screen, list_of(40));
        screen.set_status(SongSelectStatus::Ready);
        screen.input(SongSelectInput::Scroll { lines }, &layout);
        let snapshot = screen.snapshot();
        // A notch scrolls by a set row (ADR-0149).
        assert_eq!(snapshot.scroll, lines * layout.set_pitch);
        rig.frame(&snapshot);
        let mut counts = (0, 0);
        // Shapes reach their edge margin past their rectangles (ADR-0116).
        let shapes = rig
            .shapes
            .vertices()
            .iter()
            .map(|v| (v.position[1], EDGE_MARGIN));
        let text = rig.text.vertices().iter().map(|v| (v.position[1], 0.0));
        for (y, margin) in shapes.chain(text) {
            counts.0 += usize::from(y < top - margin - 0.01);
            counts.1 += usize::from(y > bottom + margin + 0.01);
        }
        counts
    };
    // Only the header, search line, hints and backing are drawn outside, whatever the
    // scroll: rows cut part-way add nothing there.
    assert_eq!(outside(3.5), outside(3.0));
    assert_eq!(outside(3.25), outside(3.0));
}

#[test]
fn changing_details_push_without_allocating_once_warm() {
    use slam_ui::song_select::{
        BeatmapDetails, DetailStats, LocalScores, MAX_LOCAL_SCORES, ScoreMods, ScoreRow,
    };

    let scores = |i: u32| LocalScores {
        rows: std::array::from_fn(|n| {
            (n < (i as usize % 4) * 4).then(|| ScoreRow {
                total_score: 1_000_000 + u64::from(i) * 7919 + n as u64,
                accuracy: 0.9 + n as f64 / 1000.0,
                max_combo: 100 + n as u32,
                rank: *b"SH",
                mods: ScoreMods::from_acronyms(["HD", "DT"]),
                date_ms: 1_700_000_000_000 + i64::from(i) * 86_400_000,
                ..Default::default()
            })
        }),
    };
    assert_eq!(scores(0).rows.len(), MAX_LOCAL_SCORES);

    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(40));
    let details = |i: u32| BeatmapDetails {
        circle_size: 4.0 + i as f32 / 10.0,
        approach_rate: 9.25,
        overall_difficulty: 8.0,
        drain_rate: 6.0,
        stats: i.is_multiple_of(2).then_some(DetailStats {
            length_ms: 205_000 + u64::from(i) * 1000,
            bpm_minimum: 150,
            bpm_maximum: 200 + i as i32,
            bpm_most_common: 180,
            circles: 512 + i,
            sliders: 300,
            spinners: 2,
        }),
        scores: Some(scores(i)),
    };
    let snapshots: Vec<_> = (0..6)
        .map(|i| {
            screen.set_details(Some(details(i)));
            screen.snapshot()
        })
        .collect();
    for snapshot in &snapshots {
        rig.frame(snapshot);
    }
    assert_no_alloc(|| {
        for snapshot in snapshots.iter().rev() {
            rig.frame(snapshot);
        }
        for snapshot in &snapshots {
            rig.frame(snapshot);
        }
    });
}

#[test]
fn dragging_and_flinging_the_list_do_not_allocate() {
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(200));
    screen.set_status(SongSelectStatus::Ready);
    screen.tick(0, &layout);
    let x = layout.list.x + layout.list.width / 2.0;
    let y = layout.list.y + layout.list.height / 2.0;
    assert_no_alloc(|| {
        let mut now = 0;
        screen.input(SongSelectInput::PointerPress { x, y }, &layout);
        for i in 1..=10 {
            let y = y - i as f32 * 30.0;
            screen.input(SongSelectInput::PointerMove { x, y }, &layout);
            now += 16_000_000;
            screen.tick(now, &layout);
        }
        screen.input(SongSelectInput::PointerRelease, &layout);
        for _ in 0..100 {
            now += 16_000_000;
            screen.tick(now, &layout);
            std::hint::black_box(screen.snapshot());
        }
        // A click: press and release in place.
        screen.input(SongSelectInput::PointerPress { x, y }, &layout);
        std::hint::black_box(screen.input(SongSelectInput::PointerRelease, &layout));
    });
    assert!(screen.snapshot().scroll > 0.0);
}

#[test]
fn switching_to_the_no_audio_message_does_not_allocate() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(40));
    screen.set_status(SongSelectStatus::Ready);
    let hints = screen.snapshot();
    screen.set_no_audio(true);
    let refused = screen.snapshot();
    assert!(refused.no_audio && !hints.no_audio);
    rig.frame(&hints);
    rig.frame(&refused);
    assert_no_alloc(|| {
        for _ in 0..10 {
            rig.frame(&hints);
            rig.frame(&refused);
        }
    });
}

/// Handles of a texture loader: distinct, whatever the bytes.
fn thumbnail_rows() -> [Option<RowThumb>; MAX_VISIBLE_ROWS] {
    use slam_render::upload::{TextureKind, TextureLoaderConfig, texture_loader};
    let (mut requests, _uploads) = texture_loader(&TextureLoaderConfig::new(LIMITS)).unwrap();
    std::array::from_fn(|i| {
        // Every other row has not arrived yet.
        (i % 2 == 0).then(|| RowThumb {
            handle: requests.load(Vec::new(), TextureKind::Standalone).unwrap(),
            region: AtlasRegion {
                texture: TextureId(3),
                origin: Origin::new(0, 0),
                size: Extent::new(8, 8),
                uv: UvRect::FULL,
            },
        })
    })
}

#[test]
fn thumbnails_do_not_allocate_once_warm_and_add_shapes() {
    let mut rig = Rig::new();
    rig.thumbs = true;
    rig.rows = thumbnail_rows();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(200));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let pass = |rig: &mut Rig, screen: &mut SongSelect| {
        screen.input(SongSelectInput::Home, &layout);
        for i in 0..400 {
            screen.input(SongSelectInput::Scroll { lines: 0.5 }, &layout);
            rig.now_ns = i * 8_000_000;
            rig.frame(&screen.snapshot());
        }
    };
    pass(&mut rig, &mut screen);
    slam_testkit::assert_no_alloc(|| pass(&mut rig, &mut screen));
    // Placeholders and images: more shapes than without thumbnails.
    let snapshot = screen.snapshot();
    rig.frame(&snapshot);
    let with = rig.shapes.len();
    rig.thumbs = false;
    rig.frame(&snapshot);
    assert!(with > rig.shapes.len(), "{with} vs {}", rig.shapes.len());
}

#[test]
fn the_scrollbar_does_not_allocate_in_input_or_view() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(200));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let (x, y) = (
        layout.scrollbar.x + layout.scrollbar.width / 2.0,
        layout.scrollbar.y + layout.scrollbar.height / 2.0,
    );
    let pass = |rig: &mut Rig, screen: &mut SongSelect| {
        for i in 0..60u64 {
            rig.now_ns = i * 8_000_000;
            for input in [
                SongSelectInput::PointerMove { x, y },
                SongSelectInput::PointerPress { x, y },
                SongSelectInput::PointerMove { x, y: y + 30.0 },
                SongSelectInput::PointerRelease,
            ] {
                std::hint::black_box(screen.input(input, &layout));
            }
            rig.frame(&screen.snapshot());
        }
    };
    pass(&mut rig, &mut screen);
    slam_testkit::assert_no_alloc(|| pass(&mut rig, &mut screen));
}

#[test]
fn full_plate_thumbnails_do_not_allocate_do_not_shift_texts_and_cover_the_plate() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(200));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    screen.input(SongSelectInput::Home, &layout);
    let snapshot = screen.snapshot();
    let (mut device, mut textures) = (UploadDevice::default(), Textures::with_capacity(16));
    let mut warm = |rig: &mut Rig| {
        // Glyphs are made on another thread: frame until the texts are all there.
        for i in 0..2000 {
            rig.now_ns = 10_000_000_000 + i * 600_000_000;
            rig.frame(&snapshot);
            if rig.glyphs.all_ready() && !rig.text.vertices().is_empty() {
                break;
            }
            rig.glyphs.process(&mut device, &mut textures);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // And once more, past every fade.
        rig.now_ns += 5_000_000_000;
        rig.frame(&snapshot);
    };
    // The texts without thumbnails, in a warm frame.
    warm(&mut rig);
    let plain_text: Vec<[f32; 2]> = rig.text.vertices().iter().map(|v| v.position).collect();
    let plain_shapes = rig.shapes.len();
    assert!(!plain_text.is_empty());
    // Small thumbnails push the set rows' texts to the right.
    rig.thumbs = true;
    rig.rows = thumbnail_rows();
    warm(&mut rig);
    let small: Vec<[f32; 2]> = rig.text.vertices().iter().map(|v| v.position).collect();
    assert_ne!(small, plain_text);
    // Full plates leave them where they were, and add one image per loaded set row.
    rig.full = true;
    warm(&mut rig);
    let full: Vec<[f32; 2]> = rig.text.vertices().iter().map(|v| v.position).collect();
    assert_eq!(full, plain_text);
    assert!(rig.shapes.len() > plain_shapes);
    slam_testkit::assert_no_alloc(|| warm(&mut rig));
}

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

/// One frame at `now_ns` with the pose shown; returns the alphas of the panel layer's shape
/// and text vertices.
fn panel_alphas(
    rig: &mut Rig,
    snapshot: &slam_ui::song_select::SongSelectSnapshot,
    now_ns: u64,
) -> (Vec<f32>, Vec<f32>) {
    rig.shapes.clear();
    rig.text.clear();
    let split = rig.view.push(
        &mut rig.shapes,
        &mut rig.text,
        &mut rig.glyphs,
        1280.0,
        720.0,
        snapshot,
        Pose::SHOWN,
        white(),
        Thumbs {
            enabled: false,
            full: false,
            rows: &rig.rows,
            now_ns,
        },
    );
    let (shapes_at, text_at) = split
        .panel
        .unwrap_or((rig.shapes.len() as u32, rig.text.len() as u32));
    let shapes = rig.shapes.vertices()[shapes_at as usize * 4..]
        .iter()
        .map(|v| v.colour[3])
        .collect();
    let text = rig.text.vertices()[text_at as usize * 4..]
        .iter()
        .map(|v| v.colour[3])
        .collect();
    (shapes, text)
}

#[test]
fn the_mod_panel_fades_its_plates_and_texts_as_one_layer() {
    // Without the stagger every part of the panel slides together, so one factor scales all.
    let theme = Theme {
        mod_column_stagger_ms: 0.0,
        ..Theme::default()
    };
    let in_ms = theme.mod_panel_in_ms as u64;
    let mut rig = Rig::with_theme(theme);
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(10));
    screen.set_status(SongSelectStatus::Ready);
    *screen.mods_mut() = slam_ui::song_select::ModSelect::new(MOD_COLUMNS);
    let closed = screen.snapshot();
    screen.mods_mut().open();
    let open = screen.snapshot();
    let ms = 1_000_000u64;
    panel_alphas(&mut rig, &closed, 1000 * ms);
    // The slide starts when the view first sees the panel open.
    panel_alphas(&mut rig, &open, 1000 * ms);
    // Warm the glyph cache at the end of the slide, then look part way through it.
    let (mut device, mut textures) = (UploadDevice::default(), Textures::with_capacity(16));
    for _ in 0..2000 {
        panel_alphas(&mut rig, &open, 1000 * ms + (in_ms + 100) * ms);
        if rig.glyphs.all_ready() && !rig.text.vertices().is_empty() {
            break;
        }
        rig.glyphs.process(&mut device, &mut textures);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let (mid_shapes, mid_text) = panel_alphas(&mut rig, &open, 1000 * ms + 60 * ms);
    let (full_shapes, full_text) = panel_alphas(&mut rig, &open, 1000 * ms + (in_ms + 100) * ms);
    assert_eq!(mid_shapes.len(), full_shapes.len());
    assert_eq!(mid_text.len(), full_text.len());
    assert!(
        mid_text.len() > 40,
        "{} of {} sprites",
        mid_text.len(),
        rig.text.len()
    );
    let ratios: Vec<f32> = mid_shapes
        .iter()
        .zip(&full_shapes)
        .chain(mid_text.iter().zip(&full_text))
        .filter(|(_, full)| **full > 0.0)
        .map(|(mid, full)| mid / full)
        .collect();
    let first = ratios[0];
    assert!(first > 0.0 && first < 1.0, "{first}");
    for r in &ratios {
        assert!((r - first).abs() < 1e-3, "{r} vs {first}");
    }
}

#[test]
fn the_footer_works_and_draws_without_allocating() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(10));
    screen.set_status(SongSelectStatus::Ready);
    *screen.mods_mut() = slam_ui::song_select::ModSelect::new(MOD_COLUMNS);
    screen.mods_mut().set_active(0, 0, true);
    let presets: Arc<[Arc<str>]> = Arc::from([Arc::from("HD DT"), Arc::from("EZ")]);
    screen.set_mod_presets(Arc::clone(&presets));
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let buttons = screen.footer_buttons(&layout);
    assert!(buttons.pin.is_some() && buttons.reset.is_some());
    let hovered = screen.snapshot();
    // Warm: laid out presets, mods label and the slide.
    rig.now_ns = 1_000_000;
    rig.frame(&hovered);
    rig.frame(&hovered);
    let at = |r: slam_ui::menu::Rect| {
        let [x, y] = r.centre();
        (x, y)
    };
    let points = [
        at(buttons.back),
        at(buttons.mods),
        at(buttons.presets[0]),
        at(buttons.pin.unwrap()),
        at(buttons.reset.unwrap()),
    ];
    assert_no_alloc(|| {
        for (x, y) in points {
            for input in [
                SongSelectInput::PointerMove { x, y },
                SongSelectInput::PointerPress { x, y },
                SongSelectInput::PointerRelease,
                SongSelectInput::SecondaryPress { x, y },
            ] {
                std::hint::black_box(screen.input(input, &layout));
                std::hint::black_box(screen.footer_state());
            }
            // The Mods button toggled the panel; draw a frame of the state.
            rig.now_ns += 16_000_000;
            rig.frame(&screen.snapshot());
        }
    });
    assert!(rig.shapes.len() > 8);
}

/// Rows just past the view's ends have their texts laid out already, so they do not pop in
/// as they scroll into view (ADR-0172).
#[test]
fn rows_just_outside_the_view_are_laid_out() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    put(&mut screen, list_of(200));
    screen.set_status(SongSelectStatus::Ready);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    screen.input(SongSelectInput::Scroll { lines: 20.0 }, &layout);
    let snapshot = screen.snapshot();
    rig.frame(&snapshot);
    let visible = layout.visible_rows(&snapshot.geometry, snapshot.shown_scroll);
    assert!(
        rig.view.laid_out_rows() >= visible.len() + 3,
        "{} laid out, {} in view",
        rig.view.laid_out_rows(),
        visible.len()
    );
}
