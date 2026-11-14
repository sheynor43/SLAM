use std::sync::Arc;

use slam_text::FontRegistry;

use super::super::{
    DifficultyRow, MAX_SCORE_MODS, ScoreMods, SearchSource, SetRow, SongFilter, SongList,
    SongSelect,
};
use super::*;

fn labels() -> DetailLabels {
    DetailLabels {
        length: "Length".into(),
        bpm: "BPM".into(),
        circles: "Circles".into(),
        sliders: "Sliders".into(),
        spinners: "Spinners".into(),
        circle_size: "CS".into(),
        approach_rate: "AR".into(),
        overall_difficulty: "OD".into(),
        drain_rate: "HP".into(),
        scores: "Local scores".into(),
        no_scores: "No local scores".into(),
    }
}

fn stats() -> DetailStats {
    DetailStats {
        length_ms: 205_400,
        bpm_minimum: 180,
        bpm_maximum: 180,
        bpm_most_common: 180,
        circles: 512,
        sliders: 300,
        spinners: 2,
    }
}

fn details(scores: Option<LocalScores>) -> BeatmapDetails {
    BeatmapDetails {
        circle_size: 4.0,
        approach_rate: 9.3,
        overall_difficulty: 8.0,
        drain_rate: 5.0,
        stats: Some(stats()),
        scores,
    }
}

fn row(score: u64) -> ScoreRow {
    ScoreRow {
        total_score: score,
        accuracy: 0.98457,
        max_combo: 1234,
        rank: *b"SH",
        mods: ScoreMods::from_acronyms(["HD", "DT"]),
        date_ms: 1_794_681_000_000,
    }
}

struct Rig {
    panel: InfoPanel,
    type_setter: Typesetter,
    theme: Theme,
    screen: SongSelect,
}

impl Rig {
    fn new() -> Rig {
        let theme = Theme::default();
        let mut screen = SongSelect::new();
        let list = Arc::new(SongList {
            sets: vec![SetRow {
                key: 1,
                artist: "Artist".into(),
                title: "Title".into(),
                creator: "mapper".into(),
                difficulties: vec![DifficultyRow::new(SearchSource {
                    difficulty: "Insane",
                    ..SearchSource::default()
                })],
            }],
        });
        let filter = SongFilter::for_list(&list);
        screen.set_list(list, filter);
        let mut type_setter = Typesetter::new(Arc::new(FontRegistry::with_builtin()), 700);
        let mut panel = InfoPanel::new(&labels());
        panel.lay_out(&mut type_setter, &theme, 1.0);
        Rig {
            panel,
            type_setter,
            theme,
            screen,
        }
    }

    fn show(&mut self, details: BeatmapDetails) {
        self.screen.set_details(Some(details));
        let snapshot = self.screen.snapshot();
        let layout = SongSelectLayout::new(1280.0, 768.0, &self.theme);
        self.panel.refresh(
            &mut self.type_setter,
            &self.theme,
            &layout,
            [10.0; 2],
            &snapshot,
        );
    }

    /// How many boxes and non-empty texts the panel draws for `details`.
    fn drawn(&self, details: BeatmapDetails) -> (usize, usize) {
        let mut screen = self.screen.clone();
        screen.set_details(Some(details));
        let snapshot = screen.snapshot();
        let layout = SongSelectLayout::new(1280.0, 768.0, &self.theme);
        let (mut rects, mut texts) = (0, 0);
        self.panel.push(
            &layout,
            &self.theme,
            &snapshot,
            &mut |_, _, _| rects += 1,
            &mut |line, _, _, _| texts += usize::from(!line.text.is_empty()),
        );
        (rects, texts)
    }
}

#[test]
fn the_panel_writes_the_name_statistics_and_values() {
    let mut rig = Rig::new();
    rig.show(details(None));
    assert_eq!(rig.panel.name.text, "Insane");
    assert_eq!(rig.panel.artist.text, "Artist - Title");
    assert_eq!(rig.panel.stats.text, "Length 03:25  \u{b7}  BPM 180");
    assert_eq!(
        rig.panel.counts.text,
        "Circles 512  \u{b7}  Sliders 300  \u{b7}  Spinners 2"
    );
    let values: Vec<_> = rig.panel.values.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(values, ["4", "9.3", "8", "5"]);
}

#[test]
fn no_scores_leave_the_rows_empty_and_scores_fill_them() {
    let mut rig = Rig::new();
    rig.show(details(Some(LocalScores::default())));
    assert!(rig.panel.scores.iter().all(|l| l.score.text.is_empty()));
    let mut scores = LocalScores::default();
    scores.rows[0] = Some(row(1_234_567));
    scores.rows[1] = Some(ScoreRow {
        mods: ScoreMods::default(),
        ..row(900)
    });
    rig.show(details(Some(scores)));
    let first = &rig.panel.scores[0];
    assert_eq!(first.rank.text, "SH");
    assert_eq!(first.score.text, "1,234,567");
    assert_eq!(first.accuracy.text, "98.45%");
    assert_eq!(
        first.extra.text,
        "1234x  \u{b7}  HD DT  \u{b7}  2026-11-14 18:30"
    );
    assert_eq!(
        rig.panel.scores[1].extra.text,
        "1234x  \u{b7}  2026-11-14 18:30"
    );
    assert!(rig.panel.scores[2].score.text.is_empty());
}

#[test]
fn the_empty_state_draws_its_line_and_rows_draw_boxes() {
    let mut rig = Rig::new();
    let empty = details(Some(LocalScores::default()));
    rig.show(empty);
    let (rects, texts) = rig.drawn(empty);
    // The bars: a track and a fill each.
    assert_eq!(rects, 8);
    // Name, artist, 4 labels, 4 values, length/BPM, counts, title and the empty line.
    assert_eq!(texts, 2 + 4 + 4 + 2 + 1 + 1);
    let mut scores = LocalScores::default();
    scores.rows[0] = Some(row(5));
    scores.rows[1] = Some(row(4));
    let listed = details(Some(scores));
    rig.show(listed);
    let (rects, _) = rig.drawn(listed);
    assert_eq!(rects, 8 + 2);
}

#[test]
fn unread_scores_draw_neither_rows_nor_the_empty_line() {
    let mut rig = Rig::new();
    rig.show(details(Some(LocalScores::default())));
    let (_, with) = rig.drawn(details(Some(LocalScores::default())));
    let (_, without) = rig.drawn(details(None));
    assert_eq!(with, without + 1);
}

#[test]
fn the_panel_fits_and_keeps_clear_of_the_list_at_common_sizes() {
    for &(w, h) in &[
        (1280.0_f32, 720.0_f32),
        (1920.0, 1080.0),
        (1024.0, 768.0),
        (800.0, 600.0),
        (2560.0, 1440.0),
    ] {
        // A menu scale of `m` makes the reference height `1 / m` as tall.
        for menu_scale in [0.75, 1.0, 1.5] {
            let theme = Theme::default().with_ui_scale(menu_scale);
            let layout = SongSelectLayout::new(w, h, &theme);
            let info = layout.info;
            let inset = theme.song_margin * layout.scale / 2.0;
            let tag = format!("{w}x{h} x{menu_scale}");
            assert!(info.x >= inset, "{tag}");
            // The info backing ends before the list backing starts.
            assert!(
                info.x + info.width + inset <= layout.header.x - inset,
                "{tag}"
            );
            // The info content stays above the hint row, which shows the mods.
            assert!(info.y + info.height <= layout.hint.y, "{tag}");
            assert!(
                layout.search.y >= layout.header.y + layout.header.height,
                "{tag}"
            );
            assert!(
                layout.list.y > layout.search.y + layout.search.height,
                "{tag}"
            );
            assert!(layout.list.y + layout.list.height <= layout.hint.y, "{tag}");
            let parts = InfoLayout::new(&info, &theme, layout.scale);
            let bottom = info.y + info.height;
            // At the largest scale the panel is shorter than its parts; every part shown
            // still lies inside, and the statistics fit at the others.
            if menu_scale <= 1.0 {
                assert!(
                    parts.scores_title.y + parts.scores_title.height <= bottom + 0.5,
                    "{tag}"
                );
                assert!(parts.row_count >= 3, "{tag}: {}", parts.row_count);
            }
            for bar in &parts.bars {
                assert!(bar.track.width > 0.0, "{tag}");
                assert!(bar.track.x + bar.track.width <= bar.value.x, "{tag}");
            }
            for area in &parts.rows[..parts.row_count] {
                assert!(area.y + area.height <= bottom, "{tag}");
            }
        }
    }
}

#[test]
fn numbers_dates_and_accuracy_are_formatted() {
    let grouped = |n| {
        let mut s = String::new();
        push_grouped(&mut s, n);
        s
    };
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1000), "1,000");
    assert_eq!(grouped(1_234_567), "1,234,567");
    assert_eq!(grouped(u64::MAX), "18,446,744,073,709,551,615");
    let accuracy = |a| {
        let mut s = String::new();
        push_accuracy(&mut s, a);
        s
    };
    assert_eq!(accuracy(1.0), "100.00%");
    assert_eq!(accuracy(0.99996), "99.99%");
    assert_eq!(accuracy(0.9845), "98.45%");
    assert_eq!(accuracy(0.0), "0.00%");
    let date = |ms| {
        let mut s = String::new();
        push_date(&mut s, ms);
        s
    };
    assert_eq!(date(0), "1970-01-01 00:00");
    assert_eq!(
        date(951_782_400_000 + 3_600_000 * 23 + 60_000 * 59),
        "2000-02-29 23:59"
    );
    assert_eq!(date(-1000), "1969-12-31 23:59");
}

#[test]
fn mods_keep_their_order_and_are_cut_to_the_capacity() {
    let mods = ScoreMods::from_acronyms(["HD", "DT", "ABC"]);
    assert_eq!(mods.iter().collect::<Vec<_>>(), ["HD", "DT", "ABC"]);
    let many = ScoreMods::from_acronyms(["A"; 20]);
    assert_eq!(many.iter().count(), MAX_SCORE_MODS);
}

#[test]
fn stats_show_one_bpm_or_the_range_and_the_most_common() {
    let mut s = String::new();
    write_length_bpm(&mut s, &labels(), &stats());
    assert_eq!(s, "Length 03:25  \u{b7}  BPM 180");
    let ranged = DetailStats {
        bpm_minimum: 150,
        bpm_maximum: 200,
        ..stats()
    };
    s.clear();
    write_length_bpm(&mut s, &labels(), &ranged);
    assert!(s.contains("BPM 150-200 (180)"), "{s}");
}

#[test]
fn durations_as_lazer_formats_them() {
    let cases = [
        (0, "00:00"),
        (59_999, "00:59"),
        (205_000, "03:25"),
        (3_600_000, "01:00:00"),
        (90_061_000, "01:01:01:01"),
    ];
    for (ms, text) in cases {
        let mut s = String::new();
        push_duration(&mut s, ms);
        assert_eq!(s, text, "{ms}");
    }
}

#[test]
fn difficulty_values_keep_two_decimals_at_most() {
    let cases = [
        (4.0, "4"),
        (9.3, "9.3"),
        (7.666_667, "7.67"),
        (8.005, "8.01"),
        (10.0, "10"),
        (0.05, "0.05"),
        (-0.5, "-0.5"),
        // The float rounds to 2.675000 at seven digits before the decimals: EZ on CS 5.35.
        (5.35f32 * 0.5, "2.68"),
        // HR on CS 4.25: 5.5249996 to 5.525000.
        (4.25f32 * 1.3, "5.53"),
        (12345.678, "12345.68"),
    ];
    for (value, text) in cases {
        let mut s = String::new();
        push_difficulty(&mut s, value);
        assert_eq!(s, text, "{value}");
    }
}
