//! The song select's steady state does not touch the allocator: input and snapshots on the
//! update side, `SongSelectView::push` on the draw side once the rows in view are cached.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::sprite::SpriteBatch;
use slam_render::textures::TextureId;
use slam_render::{Extent, Limits, Origin};
use slam_testkit::assert_no_alloc;
use slam_text::{FontRegistry, GlyphAtlasConfig, glyph_atlas};
use slam_ui::screen::Pose;
use slam_ui::song_select::{
    DifficultyRow, ModColumns, SetRow, SettingRow, SettingRowKind, SongList, SongSelect,
    SongSelectInput, SongSelectLabels, SongSelectLayout, SongSelectStatus, SongSelectView,
};
use slam_ui::theme::Theme;

slam_testkit::install_counting_allocator!();

const MOD_COLUMNS: ModColumns = [
    &["EZ", "NF", "HT", "DC"],
    &["HR", "SD", "PF"],
    &["DA", "CL"],
];

fn list_of(count: u64) -> Arc<SongList> {
    Arc::new(SongList {
        sets: (0..count)
            .map(|k| SetRow {
                key: k,
                artist: format!("artist {k}"),
                title: format!("title {k}"),
                creator: "mapper".into(),
                difficulties: (0..3)
                    .map(|d| DifficultyRow {
                        version: format!("diff {d}"),
                    })
                    .collect(),
            })
            .collect(),
    })
}

#[test]
fn input_does_not_allocate() {
    let list = list_of(40);
    let layout = SongSelectLayout::new(1280.0, 720.0, &Theme::default());
    let mut screen = SongSelect::new();
    screen.set_list(list);
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
        hint: "hint".into(),
        mods_title: "Mods".into(),
        mods_hint: "mods hint".into(),
        mods_customise: "Customise".into(),
        setting_texts: vec![
            ("mod-a", "Mod A".into()),
            ("setting-number", "Number".into()),
            ("setting-toggle", "Toggle".into()),
            ("setting-choice", "Choice".into()),
            ("value-a", "Value A".into()),
        ],
        mod_column_titles: ["Reduction".into(), "Increase".into(), "Conversion".into()],
        mod_columns: MOD_COLUMNS,
    }
}

struct Rig {
    view: SongSelectView,
    glyphs: slam_text::GlyphCache,
    shapes: SpriteBatch,
    text: SpriteBatch,
}

impl Rig {
    fn new() -> Rig {
        let registry = Arc::new(FontRegistry::with_builtin());
        let glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(LIMITS)).unwrap();
        Rig {
            view: SongSelectView::new(registry, Theme::default(), &labels()),
            glyphs,
            shapes: SpriteBatch::with_capacity(256),
            text: SpriteBatch::with_capacity(8192),
        }
    }

    fn frame(&mut self, snapshot: &slam_ui::song_select::SongSelectSnapshot) {
        self.shapes.clear();
        self.text.clear();
        let pose = Pose {
            opacity: 0.5,
            offset: 0.05,
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
        );
    }
}

#[test]
fn view_pushes_without_allocating_once_warm() {
    let mut rig = Rig::new();
    let mut screen = SongSelect::new();
    screen.set_list(list_of(40));
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
}

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
            screen.set_list(list_of(40));
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
    screen.set_list(list_of(2000));
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
    screen.set_list(list_of(10));
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
    screen.set_list(list_of(10));
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
    screen.set_list(list_of(10));
    *screen.mods_mut() = slam_ui::song_select::ModSelect::new(MOD_COLUMNS);
    let rows = setting_rows();
    let [hx, hy] = layout.mods.customise.header.centre();
    let track = layout
        .mods
        .customise
        .slider(&layout.mods.customise.row_rect(1))
        .hit;
    assert_no_alloc(|| {
        screen.mods_mut().set_setting_rows(&rows);
        for (now, input) in [
            SongSelectInput::ToggleMods,
            SongSelectInput::PointerMove { x: hx, y: hy },
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
            SongSelectInput::PointerMove { x: 5.0, y: 700.0 },
            SongSelectInput::PointerRelease,
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
    screen.set_list(list_of(10));
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
