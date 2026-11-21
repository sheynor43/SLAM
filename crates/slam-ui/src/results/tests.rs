use super::*;

fn text(f: impl FnOnce(&mut String) -> fmt::Result) -> String {
    let mut s = String::new();
    f(&mut s).unwrap();
    s
}

#[test]
fn accuracy_is_floored_to_four_digits_and_shown_as_a_percentage() {
    let cases: &[(f64, &str)] = &[
        (0.0, "0.00%"),
        (1.0, "100.00%"),
        (0.95, "95.00%"),
        (0.899999, "89.99%"),
        (0.9, "90.00%"),
        (0.999999, "99.99%"),
        (0.9999, "99.99%"),
        (0.98765, "98.76%"),
        (0.7, "70.00%"),
        (0.8, "80.00%"),
        // 0.57 * 1e4 is 5699.999999999999 in f64: lazer floors it to 56.99% too.
        (0.57, "56.99%"),
        (1.0 / 3.0, "33.33%"),
        (0.00004, "0.00%"),
        (0.0001, "0.01%"),
        (f64::NAN, "0.00%"),
        (-0.5, "0.00%"),
        (1.5, "100.00%"),
    ];
    for &(accuracy, expected) in cases {
        assert_eq!(
            text(|s| write_accuracy(s, accuracy)),
            expected,
            "{accuracy}"
        );
    }
}

#[test]
fn numbers_group_by_thousands() {
    let cases: &[(i64, &str)] = &[
        (0, "0"),
        (7, "7"),
        (999, "999"),
        (1_000, "1,000"),
        (12_345, "12,345"),
        (123_456, "123,456"),
        (1_234_567, "1,234,567"),
        (-1_234, "-1,234"),
        (i64::MAX, "9,223,372,036,854,775,807"),
        (i64::MIN, "-9,223,372,036,854,775,808"),
    ];
    for &(value, expected) in cases {
        assert_eq!(text(|s| write_grouped(s, value)), expected);
    }
}

#[test]
fn fixed_two_decimals_round_group_and_keep_the_sign() {
    let cases: &[(f64, &str)] = &[
        (0.0, "0.00"),
        (-0.0, "0.00"),
        (1.0, "1.00"),
        (1.234, "1.23"),
        (1.236, "1.24"),
        (0.125, "0.13"),
        (0.375, "0.38"),
        (2.625, "2.63"),
        (0.5, "0.50"),
        // A hair under 1.005 in f64: the exact value rounds down, as .NET's does.
        (1.005, "1.00"),
        (999.999, "1,000.00"),
        (1_000.0, "1,000.00"),
        (1_234.567, "1,234.57"),
        (12_345_678.9, "12,345,678.90"),
        (-1.234, "-1.23"),
        (-1_234.5, "-1,234.50"),
        (-0.001, "-0.00"),
        (1e15, "1000000000000000.00"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "∞"),
        (f64::NEG_INFINITY, "-∞"),
    ];
    for &(value, expected) in cases {
        assert_eq!(text(|s| write_fixed2(s, value)), expected, "{value}");
    }
}

#[test]
fn hit_errors_are_early_by_their_raw_sign() {
    assert!(is_early(-1.234));
    assert!(is_early(-0.001));
    assert!(!is_early(0.0));
    assert!(!is_early(-0.0));
    assert!(!is_early(0.001));
    assert!(!is_early(25.0));
}

#[test]
fn ranks_show_lazers_letters() {
    let cases = [
        (Rank::F, "F", false),
        (Rank::D, "D", false),
        (Rank::C, "C", false),
        (Rank::B, "B", false),
        (Rank::A, "A", false),
        (Rank::S, "S", false),
        (Rank::SH, "S", true),
        (Rank::X, "SS", false),
        (Rank::XH, "SS", true),
    ];
    for (rank, letters, silver) in cases {
        assert_eq!(
            (rank.letters(), rank.silver()),
            (letters, silver),
            "{rank:?}"
        );
    }
}

fn stat(value: u32, maximum: u32) -> ResultsStat {
    ResultsStat { value, maximum }
}

fn data(stats: [ResultsStat; StatKind::COUNT]) -> ResultsData {
    ResultsData {
        id: 1,
        title: "artist - title [diff]".to_owned(),
        score: 1_000_000,
        accuracy: 0.95,
        rank: Rank::S,
        stats,
        combo: 100,
        max_combo: 120,
        mods: ResultsMods::default(),
        average_hit_error: None,
        average_hit_error_text: None,
        unstable_rate: None,
        extra: crate::results::ResultsExtra::default(),
    }
}

#[test]
fn rows_follow_lazers_order_and_hide_absent_results() {
    use StatKind as K;
    let none = ResultsStat::default();
    // (stats by kind, expected rows)
    type Case = (Vec<(K, ResultsStat)>, Vec<K>);
    let cases: Vec<Case> = vec![
        // Circles only: the four basic rows, zeros included.
        (vec![], vec![K::Great, K::Ok, K::Meh, K::Miss]),
        // Optional rows need a maximum, whatever the value.
        (
            vec![(K::SliderTick, stat(3, 0)), (K::SpinnerSpin, stat(1, 0))],
            vec![K::Great, K::Ok, K::Meh, K::Miss],
        ),
        (
            vec![
                (K::SpinnerSpin, stat(0, 5)),
                (K::SliderTick, stat(2, 4)),
                (K::SliderTail, stat(1, 1)),
                (K::SpinnerBonus, stat(100, 200)),
                (K::SliderEnd, stat(0, 2)),
            ],
            vec![
                K::Great,
                K::Ok,
                K::Meh,
                K::Miss,
                K::SliderTick,
                K::SliderEnd,
                K::SliderTail,
                K::SpinnerBonus,
                K::SpinnerSpin,
            ],
        ),
    ];
    for (set, expected) in cases {
        let mut stats = [none; StatKind::COUNT];
        for (kind, s) in set {
            stats[kind.index()] = s;
        }
        let kinds: Vec<K> = data(stats).rows().map(|(kind, _)| kind).collect();
        assert_eq!(kinds, expected);
    }
}

#[test]
fn stat_values_show_a_fraction_for_the_optional_kinds() {
    use StatKind as K;
    let cases = [
        (K::Great, stat(120, 0), "120"),
        (K::Miss, stat(0, 0), "0"),
        (K::SliderTick, stat(7, 9), "7/9"),
        (K::SpinnerSpin, stat(0, 3), "0/3"),
    ];
    for (kind, s, expected) in cases {
        assert_eq!(text(|out| write_stat(out, kind, s)), expected);
    }
}

#[test]
fn kinds_are_indexed_in_display_order() {
    for (i, kind) in StatKind::ALL.into_iter().enumerate() {
        assert_eq!(kind.index(), i);
    }
}

#[test]
fn mods_keep_order_and_drop_the_surplus() {
    let mods = ResultsMods::new(["HD", "DT", "CLASSIC"]);
    let shown: Vec<&str> = mods.iter().map(ModAcronym::as_str).collect();
    assert_eq!(shown, ["HD", "DT", "CLA"]);
    assert!(ResultsMods::default().is_empty());
    let many = ResultsMods::new(std::iter::repeat_n("HD", 20));
    assert_eq!(many.iter().count(), MAX_MODS);
}

fn layout() -> ResultsLayout {
    ResultsLayout::new(1280.0, 720.0, &Theme::default())
}

#[test]
fn keys_and_buttons_ask_for_their_action() {
    let mut screen = Results::new(data([ResultsStat::default(); StatKind::COUNT]));
    let layout = layout();
    assert_eq!(
        screen.input(ResultsInput::Retry, &layout, 0),
        Some(ResultsAction::Retry)
    );
    assert_eq!(
        screen.input(ResultsInput::Back, &layout, 0),
        Some(ResultsAction::Back)
    );
    for (button, action) in [
        (ResultsButton::Retry, ResultsAction::Retry),
        (ResultsButton::Back, ResultsAction::Back),
    ] {
        let [x, y] = layout.buttons[ResultsButton::ALL
            .iter()
            .position(|b| *b == button)
            .unwrap()]
        .centre();
        assert_eq!(
            screen.input(ResultsInput::PointerMove { x, y }, &layout, 0),
            None
        );
        assert_eq!(screen.snapshot().hovered, Some(button));
        assert_eq!(
            screen.input(ResultsInput::PointerPress { x, y }, &layout, 0),
            Some(action)
        );
    }
    // Off the buttons: no hover, no action.
    let miss = ResultsInput::PointerPress { x: 1.0, y: 1.0 };
    assert_eq!(screen.input(miss, &layout, 0), None);
    assert_eq!(screen.snapshot().hovered, None);
}

#[test]
fn layout_keeps_everything_inside_the_window() {
    for (w, h) in [(1280.0, 720.0), (800.0, 600.0), (1920.0, 1080.0)] {
        let layout = ResultsLayout::new(w, h, &Theme::default());
        let rows = (0..StatKind::COUNT).map(|n| layout.stat_row(n));
        for rect in layout
            .buttons
            .iter()
            .chain(&layout.info)
            .chain([&layout.title, &layout.rank])
            .copied()
            .chain(rows)
        {
            assert!(rect.x >= 0.0 && rect.y >= 0.0, "{w}x{h}: {rect:?}");
            assert!(rect.x + rect.width <= w + 0.01, "{w}x{h}: {rect:?}");
            assert!(rect.y + rect.height <= h + 0.01, "{w}x{h}: {rect:?}");
        }
        // The columns do not overlap, and the rows end above the buttons.
        assert!(layout.rank.x + layout.rank.width <= layout.stat_row(0).x);
        let last = layout.stat_row(StatKind::COUNT - 1);
        assert!(last.y + last.height <= layout.buttons[0].y, "{w}x{h}");
        let info = layout.info[5];
        assert!(info.y + info.height <= layout.buttons[0].y, "{w}x{h}");
    }
}

#[test]
fn the_last_info_row_stays_clear_of_the_hint_and_the_buttons() {
    let sizes = [(16.0, 9.0), (4.0, 3.0)];
    for (aw, ah) in sizes {
        for height in [480.0_f32, 600.0, 720.0, 900.0, 1080.0, 1440.0] {
            let width = (height * aw / ah).round();
            let layout = ResultsLayout::new(width, height, &Theme::default());
            let last = layout.info[5];
            let bottom = last.y + last.height;
            assert!(
                bottom <= layout.hint.y,
                "{width}x{height}: {last:?} {:?}",
                layout.hint
            );
            assert!(bottom <= layout.buttons[0].y, "{width}x{height}");
            // The rows are stacked in order without overlapping.
            for pair in layout.info.windows(2) {
                assert!(pair[0].y + pair[0].height <= pair[1].y, "{width}x{height}");
            }
        }
    }
}

fn art() -> stable::StableArt {
    let size = |w, h| {
        Some(stable::ArtSize {
            size: [w, h],
            density: 1.0,
        })
    };
    stable::StableArt {
        title: size(300.0, 75.0),
        back: size(225.0, 87.0),
        retry: size(200.0, 60.0),
    }
}

#[test]
fn stable_look_uses_the_skin_buttons_and_scrolls() {
    let theme = Theme::default();
    for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (1024.0, 768.0)] {
        let slam = ResultsLayout::new_for(w, h, &theme, None);
        assert_eq!(slam, ResultsLayout::new(w, h, &theme));
        assert_eq!(slam.scroll_area, 0.0);
        let stable = ResultsLayout::new_for(w, h, &theme, Some(&art()));
        let [retry, back] = stable.buttons;
        // Back at the bottom left, Retry at the right edge, centred at 576/768.
        assert_eq!((back.x, back.y + back.height), (0.0, h));
        assert!((retry.x + retry.width - w).abs() < 1e-3);
        assert!((retry.y + retry.height / 2.0 - 576.0 / 768.0 * h).abs() < 1e-2);
        assert!(stable.scroll_area > 0.0 && stable.scroll_area < h);
        // Hit testing follows the drawn rects; Retry moves with the scroll.
        let [x, y] = back.centre();
        assert_eq!(stable.button_at(x, y), Some(ResultsButton::Back));
        assert_eq!(
            stable.button_at_scrolled(x, y, 50.0),
            Some(ResultsButton::Back)
        );
        let [x, y] = retry.centre();
        assert_eq!(stable.button_at(x, y), Some(ResultsButton::Retry));
        assert_eq!(stable.button_at_scrolled(x, y, 10.0 * h), None);
        assert_eq!(
            stable.button_at_scrolled(x, y - 30.0, 30.0),
            Some(ResultsButton::Retry)
        );
        let [x, y] = slam.buttons[0].centre();
        assert_eq!(slam.button_at(x, y), Some(ResultsButton::Retry));
    }
}

#[test]
fn back_button_is_clamped_to_one_and_a_half_base_heights() {
    let tall = stable::StableArt {
        back: Some(stable::ArtSize {
            size: [225.0, 870.0],
            density: 1.0,
        }),
        ..art()
    };
    let normal = stable::geometry(1024.0, 768.0, &art()).back;
    let clamped = stable::geometry(1024.0, 768.0, &tall).back;
    assert!((clamped.height - normal.height * 1.5).abs() < 1e-2);
    assert!(clamped.width < normal.width);
}

#[test]
fn hover_animates_over_a_tenth_of_a_second() {
    let layout = ResultsLayout::new_for(1024.0, 768.0, &Theme::default(), Some(&art()));
    let mut screen = Results::new(data([ResultsStat::default(); StatKind::COUNT]));
    let [x, y] = layout.buttons[1].centre();
    screen.input(ResultsInput::PointerMove { x, y }, &layout, 1_000);
    let snap = screen.snapshot();
    assert_eq!(snap.hover_amount(ResultsButton::Back, 1_000), 0.0);
    let half = snap.hover_amount(ResultsButton::Back, 1_000 + stable::HOVER_NS / 2);
    assert!(
        half > 0.5 && half < 1.0,
        "quad out is ahead of linear: {half}"
    );
    assert_eq!(
        snap.hover_amount(ResultsButton::Back, 1_000 + stable::HOVER_NS),
        1.0
    );
    assert_eq!(
        snap.hover_amount(ResultsButton::Retry, 1_000 + stable::HOVER_NS),
        0.0
    );
    // Leaving brings it back.
    let later = 1_000 + 10 * stable::HOVER_NS;
    screen.input(ResultsInput::PointerMove { x: 1.0, y: 1.0 }, &layout, later);
    let snap = screen.snapshot();
    assert_eq!(
        snap.hover_amount(ResultsButton::Back, later + stable::HOVER_NS),
        0.0
    );
}

#[test]
fn scrolling_clamps_eases_and_the_hint_scrolls_to_the_bottom() {
    let layout = ResultsLayout::new_for(1024.0, 768.0, &Theme::default(), Some(&art()));
    let mut screen = Results::new(data([ResultsStat::default(); StatKind::COUNT]));
    let scroll = |screen: &mut Results, input| screen.input(input, &layout, 0);
    scroll(&mut screen, ResultsInput::Scroll { delta: -3.0 });
    assert_eq!(screen.snapshot().scroll_target(), 0.0);
    // Past the top it springs back (see the test below); start over at rest.
    let mut screen = Results::new(data([ResultsStat::default(); StatKind::COUNT]));
    scroll(&mut screen, ResultsInput::Scroll { delta: 2.0 });
    let target = screen.snapshot().scroll_target();
    assert!(target > 0.0 && target < 1.0);
    // Eased: not there at once, there after the duration.
    let snap = screen.snapshot();
    assert_eq!(snap.scroll_at(0), 0.0);
    assert!((snap.scroll_at(stable::SCROLL_NS) - target).abs() < 1e-6);
    scroll(&mut screen, ResultsInput::Page { delta: 5.0 });
    assert_eq!(screen.snapshot().scroll_target(), 1.0);
    scroll(&mut screen, ResultsInput::Edge { bottom: false });
    assert_eq!(screen.snapshot().scroll_target(), 0.0);
    // The hint is a button while not at the bottom.
    let [x, y] = layout.hint.centre();
    assert_eq!(
        screen.input(ResultsInput::PointerPress { x, y }, &layout, 0),
        None
    );
    assert_eq!(screen.snapshot().scroll_target(), 1.0);
    scroll(&mut screen, ResultsInput::Edge { bottom: true });
    // At the bottom the hint is gone: a press there is no scroll.
    let snap = screen.snapshot();
    assert_eq!(snap.scroll_at(10 * stable::SCROLL_NS), 1.0);
    // The SLAM look does not scroll.
    let slam = ResultsLayout::new(1024.0, 768.0, &Theme::default());
    let mut flat = Results::new(data([ResultsStat::default(); StatKind::COUNT]));
    flat.input(ResultsInput::Edge { bottom: true }, &slam, 0);
    assert_eq!(flat.snapshot().scroll_target(), 0.0);
}

#[test]
fn header_lines_follow_the_info_label() {
    let unit = 2.0;
    let bar = stable::header_min_height(unit);
    let (lines, count) = stable::header_lines(unit, bar, true);
    assert_eq!(count, 3);
    assert_eq!(lines[0].kind, stable::HeaderKind::Title);
    assert_eq!((lines[0].x, lines[0].y), (10.0, 10.0));
    // 0.741 of the minimum height ratio is below 1: the global scale is 1.
    assert_eq!(lines[0].scale, 1.0);
    assert!((lines[1].scale - 0.78).abs() < 1e-6);
    assert!(lines[1].y > lines[0].y && lines[2].y > lines[1].y);
    // A tall bar scales the lines up.
    let (tall, _) = stable::header_lines(unit, bar * 3.0, true);
    assert!(tall[0].scale > 2.0);
    // Without the mapper line the played line takes its place.
    let (two, count) = stable::header_lines(unit, bar, false);
    assert_eq!(count, 2);
    assert_eq!(two[1].kind, stable::HeaderKind::Played);
    assert_eq!(two[1].y, lines[1].y);
}

#[test]
fn health_line_points_and_colours() {
    let graph = Rect {
        x: 100.0,
        y: 200.0,
        width: 100.0,
        height: 50.0,
    };
    let mut segments = Vec::new();
    stable::health_segments(graph, &[1.0, 0.8, 0.2, 0.0, 1.0], |a, b, low| {
        segments.push((a, b, low));
    });
    assert_eq!(segments.len(), 4);
    // Inset 8% each side: x from 108 to 192, y from 204 to 246.
    assert_eq!(segments[0].0, [108.0, 204.0]);
    assert!((segments[3].1[0] - 192.0).abs() < 1e-3 && (segments[3].1[1] - 204.0).abs() < 1e-3);
    assert!((segments[3].0[1] - 246.0).abs() < 1e-3);
    let lows: Vec<bool> = segments.iter().map(|s| s.2).collect();
    assert_eq!(lows, [false, false, true, false]);
    // Nothing without a curve; long curves are sampled.
    stable::health_segments(graph, &[], |_, _, _| panic!());
    stable::health_segments(graph, &[0.5], |_, _, _| panic!());
    let long = vec![0.5; 10_000];
    let mut n = 0;
    stable::health_segments(graph, &long, |_, _, _| n += 1);
    assert_eq!(n, stable::MAX_HEALTH_SEGMENTS);
}

#[test]
fn hint_bounce_stays_in_range() {
    for i in 0..200 {
        let b = stable::hint_bounce(i as f32 * 0.037);
        assert!((-0.25..=0.0).contains(&b), "{b}");
    }
    assert_eq!(stable::hint_bounce(0.0), -0.25);
}

#[test]
fn the_wheel_pulls_the_content_past_the_top_and_it_springs_back() {
    let layout = ResultsLayout::new_for(1920.0, 1080.0, &Theme::default(), Some(&art()));
    let mut screen = Results::new(data([ResultsStat::default(); StatKind::COUNT]));
    screen.input(ResultsInput::Scroll { delta: -1.0 }, &layout, 0);
    let snap = screen.snapshot();
    // The target stays at the top; the content goes past it, furthest at 100 ms.
    assert_eq!(snap.scroll_target(), 0.0);
    assert_eq!(snap.scroll_at(0), 0.0);
    let furthest = snap.scroll_at(100_000_000);
    assert!((furthest + 0.15).abs() < 1e-6, "{furthest}");
    // Halfway back it is still pulled, then at rest at the top.
    let back = snap.scroll_at(200_000_000);
    assert!(back < 0.0 && back > furthest, "{back}");
    assert_eq!(snap.scroll_at(600_000_000), 0.0);

    // Quick lines add up, to a limit.
    for i in 0..20 {
        screen.input(
            ResultsInput::Scroll { delta: -1.0 },
            &layout,
            1_000_000_000 + i,
        );
    }
    let snap = screen.snapshot();
    assert!((snap.scroll_at(1_100_000_019) + 0.75).abs() < 1e-6);

    // Down from a pull scrolls as before.
    screen.input(ResultsInput::Scroll { delta: 1.0 }, &layout, 3_000_000_000);
    assert!(screen.snapshot().scroll_target() > 0.0);
}
