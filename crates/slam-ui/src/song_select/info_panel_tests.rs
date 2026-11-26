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
        accuracy: "ACCURACY".into(),
        combo: "MAX COMBO".into(),
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
        multiplier: 1.1801,
        date_ms: 1_794_681_000_000,
        ..Default::default()
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
    fn drawn(&mut self, details: BeatmapDetails) -> (usize, usize) {
        let mut screen = self.screen.clone();
        screen.set_details(Some(details));
        let snapshot = screen.snapshot();
        let layout = SongSelectLayout::new(1280.0, 768.0, &self.theme);
        let (mut rects, texts) = (0, std::cell::Cell::new(0));
        self.panel.push(
            &layout,
            &self.theme,
            &snapshot,
            1.0,
            0,
            &mut |_, _, _| rects += 1,
            &mut |line, _, _, _| texts.set(texts.get() + usize::from(!line.text.is_empty())),
            &mut |line, _, _, _, _, _| texts.set(texts.get() + usize::from(!line.text.is_empty())),
        );
        (rects, texts.get())
    }

    /// The outlines of the wedges drawn at `opacity`.
    fn outlines(&mut self, details: BeatmapDetails, opacity: f32) -> Vec<Color> {
        let mut screen = self.screen.clone();
        screen.set_details(Some(details));
        let snapshot = screen.snapshot();
        let layout = SongSelectLayout::new(1280.0, 768.0, &self.theme);
        let mut found = Vec::new();
        self.panel.push(
            &layout,
            &self.theme,
            &snapshot,
            opacity,
            0,
            &mut |_, _, look| {
                if let Look::Wedge { outline, .. } = look {
                    found.push(outline);
                }
            },
            &mut |_, _, _, _| {},
            &mut |_, _, _, _, _, _| {},
        );
        found
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
    assert_eq!(first.combo.text, "1234x");
    assert_eq!(first.date.text, "2026-11-14 18:30");
    assert_eq!(first.extra.text, "HD DT  \u{b7}  1.19x");
    assert_eq!(rig.panel.scores[1].extra.text, "");
    assert_eq!(rig.panel.scores[1].date.text, "2026-11-14 18:30");
    assert!(rig.panel.scores[2].score.text.is_empty());
}

#[test]
fn the_empty_state_draws_its_line_and_rows_draw_boxes() {
    let mut rig = Rig::new();
    let empty = details(Some(LocalScores::default()));
    rig.show(empty);
    let (rects, texts) = rig.drawn(empty);
    // The two wedges (ADR-0136), the bars: a track and a fill each.
    assert_eq!(rects, 2 + 8);
    // Name, artist, 4 labels, 4 values, length/BPM, counts, title and the empty line.
    assert_eq!(texts, 2 + 4 + 4 + 2 + 1 + 1);
    let mut scores = LocalScores::default();
    scores.rows[0] = Some(row(5));
    scores.rows[1] = Some(row(4));
    let listed = details(Some(scores));
    rig.show(listed);
    let (rects, _) = rig.drawn(listed);
    assert_eq!(rects, 2 + 8 + 2);
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
            // The two wedges keep apart (ADR-0136).
            assert!(
                parts.counts.y + parts.counts.height + inset < parts.scores_title.y - inset,
                "{tag}"
            );
            // Leaning from the bottom, the wedges' top right corner stays clear of the list.
            let lean = (info.height + 2.0 * inset) * theme.button_skew().tan();
            assert!(
                info.x + info.width + inset + lean <= layout.header.x - inset + 0.5,
                "{tag}"
            );
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

#[test]
fn the_wedge_outline_fades_with_the_screen() {
    let mut rig = Rig::new();
    let shown = details(Some(LocalScores::default()));
    let full = rig.outlines(shown, 1.0);
    let leaving = rig.outlines(shown, 0.25);
    assert_eq!(full.len(), leaving.len());
    assert!(
        full[0].a > 0.0,
        "the selected difficulty's wedge is outlined"
    );
    for (a, b) in full.iter().zip(&leaving) {
        assert!((b.a - a.a * 0.25).abs() < 1e-6, "{a:?} {b:?}");
    }
}

#[test]
fn the_wedges_run_off_the_left_edge_from_top_to_bottom() {
    let mut rig = Rig::new();
    let mut screen = rig.screen.clone();
    screen.set_details(Some(details(Some(LocalScores::default()))));
    let snapshot = screen.snapshot();
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    let tan = rig.theme.button_skew().tan();
    let mut lefts = Vec::new();
    rig.panel.push(
        &layout,
        &rig.theme,
        &snapshot,
        1.0,
        0,
        &mut |r, _, look| {
            if let Look::Wedge { lean, .. } = look {
                assert_eq!(lean, tan);
                // The sheared left edge at the top and at the bottom.
                let centre = r.x + r.width / 2.0;
                let top = centre - r.width / 2.0 + r.height / 2.0 * lean;
                let bottom = centre - r.width / 2.0 - r.height / 2.0 * lean;
                lefts.push((top, bottom));
            }
        },
        &mut |_, _, _, _| {},
        &mut |_, _, _, _, _, _| {},
    );
    assert_eq!(lefts.len(), 2);
    for (top, bottom) in lefts {
        assert!(top <= 0.0 && bottom <= 0.0, "{top} {bottom}");
    }
}

#[test]
fn the_difficulty_lines_keep_a_text_pad_from_the_leaning_edge() {
    let mut rig = Rig::new();
    rig.show(details(Some(LocalScores::default())));
    let mut screen = rig.screen.clone();
    screen.set_details(Some(details(Some(LocalScores::default()))));
    let snapshot = screen.snapshot();
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    let pad = rig.theme.song_text_pad * layout.scale;
    let inset = rig.theme.song_margin * layout.scale / 2.0;
    let mut wedges = Vec::new();
    let lines = std::cell::RefCell::new(Vec::new());
    rig.panel.push(
        &layout,
        &rig.theme,
        &snapshot,
        1.0,
        0,
        &mut |r, _, look| {
            if let Look::Wedge { lean, .. } = look {
                wedges.push((*r, lean));
            }
        },
        &mut |line, x, centre_y, _| {
            if !line.text.is_empty() {
                lines
                    .borrow_mut()
                    .push((x, x + line.layout.width(), centre_y));
            }
        },
        &mut |line, marquee, avail, x, centre_y, _| {
            if !line.text.is_empty() {
                // The visible part: the room when the text rides.
                let width = line.layout.width().min(avail);
                assert!(marquee.overflow() >= 0.0);
                lines.borrow_mut().push((x, x + width, centre_y));
            }
        },
    );
    // The difficulty's wedge is the first; its right edge leans from its centre.
    let (wedge, lean) = wedges[0];
    let edge = |y: f32| wedge.x + wedge.width + (wedge.y + wedge.height / 2.0 - y) * lean;
    let inside: Vec<_> = lines
        .into_inner()
        .into_iter()
        .filter(|&(_, _, y)| y > wedge.y && y < wedge.y + wedge.height)
        .collect();
    // The name, the artist, four bars of two lines and the two statistics lines.
    assert!(inside.len() >= 12, "{}", inside.len());
    for (left, right, y) in inside {
        assert!(right <= edge(y) - pad, "{right} vs edge {} at {y}", edge(y));
        assert!(left >= inset, "{left} at {y}");
    }
}

/// Another difficulty's values: the bars move to them over the theme's time (ADR-0156); the
/// first one shows at once.
#[test]
fn the_bars_move_to_new_values_and_the_first_shows_at_once() {
    let mut rig = Rig::new();
    let ms = 1_000_000;
    let push = |rig: &mut Rig, details: BeatmapDetails, now: u64| {
        let mut screen = rig.screen.clone();
        screen.set_details(Some(details));
        let snapshot = screen.snapshot();
        let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
        rig.panel.push(
            &layout,
            &rig.theme,
            &snapshot,
            1.0,
            now,
            &mut |_, _, _| {},
            &mut |_, _, _, _| {},
            &mut |_, _, _, _, _, _| {},
        );
    };
    let duration = rig.theme.song_info_transition_ms as u64 * ms;
    let at = |rig: &Rig, now: u64| {
        rig.panel.motion.bars[0].at(now, duration, crate::anim::ease_out_quint)
    };
    push(&mut rig, details(None), 10 * ms);
    assert_eq!(rig.panel.motion.texts_since, 0);
    assert_eq!(at(&rig, 10 * ms), 0.4);
    let harder = BeatmapDetails {
        circle_size: 8.0,
        ..details(None)
    };
    push(&mut rig, harder, 20 * ms);
    let early = at(&rig, 21 * ms);
    assert!(early > 0.4 && early < 0.8, "{early}");
    assert_eq!(at(&rig, 20 * ms + duration), 0.8);
    // The same values again change nothing.
    push(&mut rig, harder, 30 * ms);
    assert_eq!(at(&rig, 20 * ms + duration), 0.8);
}

/// The centres of the score rows as the panel draws them (the rounded fills).
fn drawn_score_centres(
    rig: &mut Rig,
    screen: &SongSelect,
    layout: &SongSelectLayout,
) -> Vec<[f32; 2]> {
    let snapshot = screen.snapshot();
    let mut centres = Vec::new();
    rig.panel.push(
        layout,
        &rig.theme,
        &snapshot,
        1.0,
        0,
        &mut |r, _, look| {
            if matches!(look, Look::Round) {
                centres.push(r.centre());
            }
        },
        &mut |_, _, _, _| {},
        &mut |_, _, _, _, _, _| {},
    );
    centres
}

#[test]
fn a_click_on_a_listed_score_opens_it_and_hover_follows_the_pointer() {
    use super::super::{SongSelectAction, SongSelectInput as I};
    let mut rig = Rig::new();
    let mut screen = rig.screen.clone();
    let mut scores = LocalScores::default();
    scores.rows[0] = Some(row(1_234_567));
    scores.rows[1] = Some(row(900));
    screen.set_details(Some(details(Some(scores))));
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    rig.show(details(Some(scores)));
    let centres = drawn_score_centres(&mut rig, &screen, &layout);
    assert_eq!(centres.len(), 2);

    for (i, [x, y]) in centres.iter().copied().enumerate() {
        assert_eq!(screen.input(I::PointerMove { x, y }, &layout), None);
        assert_eq!(screen.snapshot().score_hovered, Some(i));
        assert_eq!(screen.input(I::PointerPress { x, y }, &layout), None);
        assert_eq!(
            screen.input(I::PointerRelease, &layout),
            Some(SongSelectAction::OpenScore {
                row: scores.rows[i].unwrap()
            })
        );
    }
    // Past the last listed row there is nothing to open, and nothing is hovered.
    let [x, y] = centres[1];
    let pitch = rig.theme.song_score_row_height * layout.scale;
    screen.input(I::PointerMove { x, y: y + pitch }, &layout);
    assert_eq!(screen.snapshot().score_hovered, None);
    screen.input(I::PointerPress { x, y: y + pitch }, &layout);
    assert_eq!(screen.input(I::PointerRelease, &layout), None);
}

#[test]
fn the_hovered_score_row_takes_the_hover_fill() {
    use super::super::SongSelectInput as I;
    let mut rig = Rig::new();
    let mut screen = rig.screen.clone();
    let mut scores = LocalScores::default();
    scores.rows[0] = Some(row(1));
    scores.rows[1] = Some(row(2));
    screen.set_details(Some(details(Some(scores))));
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    rig.show(details(Some(scores)));
    let [x, y] = drawn_score_centres(&mut rig, &screen, &layout)[1];
    screen.input(I::PointerMove { x, y }, &layout);
    let snapshot = screen.snapshot();
    let mut fills = Vec::new();
    // Long after the rows came in.
    rig.panel.push(
        &layout,
        &rig.theme,
        &snapshot,
        1.0,
        5_000_000_000,
        &mut |_, colour, look| {
            if matches!(look, Look::Round) {
                fills.push(colour);
            }
        },
        &mut |_, _, _, _| {},
        &mut |_, _, _, _, _, _| {},
    );
    assert_eq!(
        fills,
        [rig.theme.song_search_fill, rig.theme.button_hover_fill]
    );
}

#[test]
fn short_texts_narrow_the_panel_and_a_new_width_moves_there() {
    let mut rig = Rig::new();
    rig.theme.song_info_auto_width = true;
    rig.show(details(None));
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    let snapshot = {
        let mut screen = rig.screen.clone();
        screen.set_details(Some(details(None)));
        screen.snapshot()
    };
    let right_edge = |panel: &mut InfoPanel, now_ns: u64| {
        let mut edge = 0.0_f32;
        panel.push(
            &layout,
            &rig.theme,
            &snapshot,
            1.0,
            now_ns,
            &mut |r, _, look| {
                if matches!(look, Look::Wedge { .. }) {
                    edge = edge.max(r.x + r.width);
                }
            },
            &mut |_, _, _, _| {},
            &mut |_, _, _, _, _, _| {},
        );
        edge
    };
    let first = right_edge(&mut rig.panel, 0);
    let full = layout.info.x + layout.info.width + rig.theme.song_margin;
    assert!(first < full, "short texts: {first} < {full}");
    // A longer name widens the panel over the transition, not at once.
    rig.panel.name.text.push_str(" with a much longer name");
    rig.type_setter
        .lay_out(&mut rig.panel.name, rig.theme.song_info_title_size);
    let start = right_edge(&mut rig.panel, 1);
    let later = right_edge(&mut rig.panel, 200_000_000);
    let end = right_edge(&mut rig.panel, 2_000_000_000);
    assert!(start - first < 1.0, "no jump: {first} -> {start}");
    assert!(start < later && later < end, "{start} < {later} < {end}");
}

/// The right edge of the widest wedge the panel draws.
fn wedge_edge(rig: &mut Rig) -> f32 {
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    let mut screen = rig.screen.clone();
    screen.set_details(Some(details(None)));
    let snapshot = screen.snapshot();
    let mut edge = 0.0_f32;
    rig.panel.push(
        &layout,
        &rig.theme,
        &snapshot,
        1.0,
        0,
        &mut |r, _, look| {
            if matches!(look, Look::Wedge { .. }) {
                edge = edge.max(r.x + r.width);
            }
        },
        &mut |_, _, _, _| {},
        &mut |_, _, _, _, _, _| {},
    );
    edge
}

#[test]
fn the_panel_does_not_follow_its_texts_unless_asked() {
    let mut rig = Rig::new();
    assert!(!rig.theme.song_info_auto_width);
    rig.show(details(None));
    let fixed = wedge_edge(&mut rig);
    let mut auto = Rig::new();
    auto.theme.song_info_auto_width = true;
    auto.show(details(None));
    assert!(
        wedge_edge(&mut auto) < fixed - 1.0,
        "short texts narrow it only when asked"
    );
}

#[test]
fn the_default_width_is_lazers_at_its_ui_scale_and_the_multipliers_scale_it() {
    let theme = Theme::default();
    // Lazer at 0.85: 700 * 0.85 on a 1080p window.
    let at = |theme: &Theme| SongSelectLayout::new(1920.0, 1080.0, theme).info.width;
    assert!((at(&theme) - 595.0).abs() < 8.0, "{}", at(&theme));
    let narrow = Theme {
        song_info_width_mult: 0.5,
        ..Theme::default()
    };
    assert!((at(&narrow) - at(&theme) / 2.0).abs() < 1.0);
    // Wider windows get lazer's bonus, up to half the window.
    let wide = SongSelectLayout::new(2560.0, 1080.0, &theme).info.width;
    assert!(wide > at(&theme) * 1.05, "{wide}");
}

#[test]
fn the_height_multiplier_stretches_the_rows() {
    let theme = Theme::default();
    let tall = Theme {
        song_info_height_mult: 1.5,
        ..Theme::default()
    };
    let layout = SongSelectLayout::new(1280.0, 768.0, &theme);
    let taller = SongSelectLayout::new(1280.0, 768.0, &tall);
    assert!(taller.info_parts.bars[3].track.y > layout.info_parts.bars[3].track.y);
    assert!(taller.info_parts.rows[0].height > layout.info_parts.rows[0].height);
}

#[test]
fn texts_that_do_not_fit_ride_instead_of_being_cut() {
    let mut rig = Rig::new();
    rig.show(details(None));
    rig.panel.name.text = "A very long difficulty name that cannot fit the panel at all".into();
    rig.type_setter.lay_out_face(
        &mut rig.panel.name,
        rig.theme.heading_face(rig.theme.song_info_title_size),
    );
    assert!(!rig.panel.name.text.contains('\u{2026}'));
    let mut screen = rig.screen.clone();
    screen.set_details(Some(details(None)));
    let snapshot = screen.snapshot();
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    let params = crate::marquee::MarqueeParams::new(&rig.theme, layout.scale);
    // The draw side hands each marquee the room; here it only reads the overflow.
    let room = std::cell::Cell::new(0.0_f32);
    rig.panel.push(
        &layout,
        &rig.theme,
        &snapshot,
        1.0,
        0,
        &mut |_, _, _| {},
        &mut |_, _, _, _| {},
        &mut |line, marquee, avail, _, _, _| {
            if line.text.starts_with("A very long") {
                room.set(avail);
                marquee.set_overflow(line.layout.width() - avail, 0);
            }
        },
    );
    let mut marquee = crate::marquee::Marquee::default();
    marquee.set_overflow(rig.panel.name.layout.width() - room.get(), 0);
    assert!(marquee.overflow() > 0.0, "the name overflows its room");
    // Held for the pause, then moving.
    assert_eq!(marquee.offset(params.pause_ns / 2, &params), 0.0);
    assert!(marquee.offset(params.pause_ns + 500_000_000, &params) > 0.0);
}

/// Per row, when its entrance started, after the frame at `now`.
fn row_starts(rig: &mut Rig, screen: &SongSelect, now: u64) -> [u64; MAX_LOCAL_SCORES] {
    let snapshot = screen.snapshot();
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    rig.panel.push(
        &layout,
        &rig.theme,
        &snapshot,
        1.0,
        now,
        &mut |_, _, _| {},
        &mut |_, _, _, _| {},
        &mut |_, _, _, _, _, _| {},
    );
    rig.panel.motion.row_since
}

#[test]
fn score_rows_come_in_staggered_and_only_new_rows_animate_again() {
    let mut rig = Rig::new();
    let ms = 1_000_000u64;
    let mut scores = LocalScores::default();
    for (i, slot) in scores.rows.iter_mut().enumerate().take(3) {
        *slot = Some(row(100 - i as u64));
    }
    let mut screen = rig.screen.clone();
    screen.set_details(Some(details(Some(scores))));
    let t = &rig.theme;
    let (delay, stagger) = (
        t.song_score_enter_delay_ms as u64 * ms,
        t.song_score_enter_stagger_ms as u64 * ms,
    );
    let now = 10 * ms;
    let first = row_starts(&mut rig, &screen, now);
    for (i, start) in first.iter().enumerate().take(3) {
        assert_eq!(*start, now + delay + stagger * i as u64, "row {i}");
    }
    // The same difficulty with a refreshed list of the same length: nothing starts over.
    let mut refreshed = scores;
    refreshed.rows[0] = Some(row(5000));
    screen.set_details(Some(details(Some(refreshed))));
    assert_eq!(row_starts(&mut rig, &screen, 900 * ms), first);
    // A new row comes in alone, after the delay.
    refreshed.rows[3] = Some(row(1));
    screen.set_details(Some(details(Some(refreshed))));
    let more = row_starts(&mut rig, &screen, 2000 * ms);
    assert_eq!(more[..3], first[..3]);
    assert_eq!(more[3], 2000 * ms + delay);
    // Another difficulty starts all over.
    let mut other = screen.clone();
    let list = Arc::new(SongList {
        sets: vec![SetRow {
            key: 2,
            artist: "Artist".into(),
            title: "Title".into(),
            creator: "mapper".into(),
            difficulties: vec![DifficultyRow {
                key: 77,
                ..DifficultyRow::new(SearchSource::default())
            }],
        }],
    });
    let filter = SongFilter::for_list(&list);
    other.set_list(list, filter);
    other.set_details(Some(details(Some(scores))));
    let again = row_starts(&mut rig, &other, 3000 * ms);
    assert_eq!(again[0], 3000 * ms + delay);
}

#[test]
fn the_panel_does_not_move_sideways_when_scores_come_or_go() {
    let mut rig = Rig::new();
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    let wedge_x = |rig: &mut Rig, details: BeatmapDetails| {
        rig.show(details);
        let mut screen = rig.screen.clone();
        screen.set_details(Some(details));
        let snapshot = screen.snapshot();
        let mut first = None;
        rig.panel.push(
            &layout,
            &rig.theme,
            &snapshot,
            1.0,
            0,
            &mut |r, _, look| {
                if first.is_none() && matches!(look, Look::Wedge { .. }) {
                    first = Some(r.x + r.width);
                }
            },
            &mut |_, _, _, _| {},
            &mut |_, _, _, _, _, _| {},
        );
        first.unwrap()
    };
    let none = wedge_x(&mut rig, details(None));
    let empty = wedge_x(&mut rig, details(Some(LocalScores::default())));
    let mut some = LocalScores::default();
    some.rows[0] = Some(row(1_000_000));
    let one = wedge_x(&mut rig, details(Some(some)));
    assert!(
        (none - empty).abs() < 1e-3 && (empty - one).abs() < 1e-3,
        "{none} {empty} {one}"
    );
}

#[test]
fn a_new_star_rating_eases_the_pills_colour() {
    let mut rig = Rig::new();
    let layout = SongSelectLayout::new(1280.0, 768.0, &rig.theme);
    let with_stars = |stars: f32| {
        let mut difficulty = DifficultyRow::new(SearchSource {
            difficulty: "Insane",
            ..SearchSource::default()
        });
        difficulty.stars = Some(stars);
        Arc::new(SongList {
            sets: vec![SetRow {
                key: 1,
                artist: "Artist".into(),
                title: "Title".into(),
                creator: "mapper".into(),
                difficulties: vec![difficulty],
            }],
        })
    };
    let pill = |rig: &mut Rig, now_ns: u64| {
        let snapshot = rig.screen.snapshot();
        let mut colour = None;
        rig.panel.push(
            &layout,
            &rig.theme,
            &snapshot,
            1.0,
            now_ns,
            &mut |_, c, look| {
                if colour.is_none() && matches!(look, Look::Pill) {
                    colour = Some(c);
                }
            },
            &mut |_, _, _, _| {},
            &mut |_, _, _, _, _, _| {},
        );
        colour.expect("a star pill")
    };
    let list = with_stars(2.0);
    let filter = SongFilter::for_list(&list);
    rig.screen.set_list(list, filter);
    let low = pill(&mut rig, 1_000_000_000);
    let list = with_stars(7.0);
    let filter = SongFilter::for_list(&list);
    rig.screen.refresh_list(list, filter);
    let start = pill(&mut rig, 1_001_000_000);
    let end = pill(&mut rig, 3_000_000_000);
    let far = |a: Color, b: Color| (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs();
    assert!(far(start, low) < 0.05, "no jump at once");
    assert!(far(end, low) > 0.2, "it gets there");
    assert_eq!(end, crate::star_colour::for_stars(7.0));
}
