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
        screen.input(ResultsInput::Retry, &layout),
        Some(ResultsAction::Retry)
    );
    assert_eq!(
        screen.input(ResultsInput::Back, &layout),
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
            screen.input(ResultsInput::PointerMove { x, y }, &layout),
            None
        );
        assert_eq!(screen.snapshot().hovered, Some(button));
        assert_eq!(
            screen.input(ResultsInput::PointerPress { x, y }, &layout),
            Some(action)
        );
    }
    // Off the buttons: no hover, no action.
    let miss = ResultsInput::PointerPress { x: 1.0, y: 1.0 };
    assert_eq!(screen.input(miss, &layout), None);
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
        let info = layout.info[3];
        assert!(info.y + info.height <= layout.buttons[0].y, "{w}x{h}");
    }
}
