//! The query parser against lazer's `FilterQueryParserTest`.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/NonVisual/Filtering/FilterQueryParserTest.cs

use super::query::*;

fn terms(c: &Criteria) -> Vec<(&str, MatchMode)> {
    c.terms().iter().map(|t| (c.term(t), t.mode)).collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn bare_words() {
    let c = Criteria::parse("looking for a beatmap");
    assert_eq!(c.terms().len(), 4);
    assert_eq!(c.search_number, None);
}

#[test]
fn bare_words_with_exact_match() {
    let c = Criteria::parse("looking for \"a beatmap\"! like \"this\"");
    use MatchMode::*;
    assert_eq!(
        terms(&c),
        [
            ("a beatmap", FullPhrase),
            ("this", IsolatedPhrase),
            ("looking", Substring),
            ("for", Substring),
            ("like", Substring),
        ]
    );
}

#[test]
fn full_phrase_with_exclamation_point_in_term() {
    let c = Criteria::parse("looking for \"circles!\"!");
    assert_eq!(c.terms().len(), 3);
    assert_eq!(terms(&c)[0], ("circles!", MatchMode::FullPhrase));
}

#[test]
fn broken_full_phrase() {
    let c = Criteria::parse("\"!");
    assert_eq!(terms(&c), [("!", MatchMode::IsolatedPhrase)]);
}

#[test]
fn star_queries() {
    for key in ["star", "stars"] {
        let c = Criteria::parse(&format!("{key}<4 easy"));
        assert_eq!(c.terms().len(), 1);
        assert_eq!(c.star_difficulty.max, Some(4.0));
        assert_eq!(c.star_difficulty.min, None);
        assert!(!c.star_difficulty.upper_inclusive);
    }
    let c = Criteria::parse("stars>=6");
    assert_eq!(c.star_difficulty.min, Some(6.0));
    assert!(c.star_difficulty.lower_inclusive);
    assert_eq!(c.star_difficulty.max, None);
}

#[test]
fn approach_rate_queries() {
    let c = Criteria::parse("ar>=9 difficult");
    let min = c.approach_rate.min.unwrap();
    assert!(min > 8.9 && min < 9.0);
    assert_eq!(c.approach_rate.max, None);
    assert_eq!(c.terms().len(), 1);
}

#[test]
fn drain_rate_queries() {
    for query in ["dr>2 quite specific dr<:6", "hp>2 quite specific hp<=6"] {
        let c = Criteria::parse(query);
        assert_eq!(c.terms().len(), 2, "{query}");
        let r = c.drain_rate;
        assert!(r.min.unwrap() > 2.0 && r.min.unwrap() < 2.1, "{query}");
        assert!(r.max.unwrap() > 6.0 && r.max.unwrap() < 6.1, "{query}");
    }
}

#[test]
fn overall_difficulty_queries() {
    let c = Criteria::parse("od>4 easy od<8");
    let r = c.overall_difficulty;
    assert!(r.min.unwrap() > 4.0 && r.min.unwrap() < 4.1);
    assert!(r.max.unwrap() > 7.9 && r.max.unwrap() < 8.0);
}

#[test]
fn bpm_queries() {
    let c = Criteria::parse("bpm=200");
    assert_eq!((c.bpm.min, c.bpm.max), (Some(199.5), Some(200.5)));
    let c = Criteria::parse("bpm>:200 gotta go fast");
    assert_eq!(c.terms().len(), 3);
    assert_eq!((c.bpm.min, c.bpm.max), (Some(199.5), None));
}

#[test]
fn length_queries() {
    // (value, milliseconds, scale)
    let cases = [
        ("23s", 23_000.0, 1000.0),
        ("9m", 540_000.0, 60_000.0),
        ("0.25h", 900_000.0, 3_600_000.0),
        ("70", 70_000.0, 1000.0),
        ("7m27s", 447_000.0, 1000.0),
        ("7:27", 447_000.0, 1000.0),
        ("1h2m3s", 3_723_000.0, 1000.0),
        ("1h2m3.5s", 3_723_500.0, 1000.0),
        ("1:2:3", 3_723_000.0, 1000.0),
        ("1:02:03", 3_723_000.0, 1000.0),
        ("6", 6000.0, 1000.0),
        ("6.5", 6500.0, 1000.0),
        ("6.5s", 6500.0, 1000.0),
        ("6.5m", 390_000.0, 60_000.0),
        ("6h5m", 21_900_000.0, 60_000.0),
        ("65m", 3_900_000.0, 60_000.0),
        ("90s", 90_000.0, 1000.0),
        ("80m20s", 4_820_000.0, 1000.0),
        ("1h20s", 3_620_000.0, 1000.0),
    ];
    for (value, ms, scale) in cases {
        let c = Criteria::parse(&format!("length={value} time"));
        assert_eq!(c.terms().len(), 1, "{value}");
        let r = c.length;
        assert!(close(r.min.unwrap(), ms - scale / 2.0), "{value}: {r:?}");
        assert!(close(r.max.unwrap(), ms + scale / 2.0), "{value}: {r:?}");
    }
}

#[test]
fn invalid_length_queries() {
    for value in [
        "7.5m27s", "7m27", "7m7m7m", "7m70s", "5s6m", "0:", ":0", "0:3:", "3:15.5",
    ] {
        let c = Criteria::parse(&format!("length={value} time"));
        assert!(!c.length.has_filter(), "{value}");
        assert_eq!(c.terms().len(), 2, "{value}");
    }
}

#[test]
fn divisor_queries() {
    let c = Criteria::parse("that's a time signature alright! divisor:12");
    assert_eq!(c.terms().len(), 5);
    let r = c.beat_divisor;
    assert_eq!((r.min, r.max), (Some(12), Some(12)));
    assert!(r.lower_inclusive && r.upper_inclusive);
}

#[test]
fn status_queries() {
    assert!(Criteria::parse("status=r").online_status.contains("Ranked"));
    let c = Criteria::parse("I want the pp status=ranked");
    assert_eq!(c.terms().len(), 4);
    assert!(c.online_status.contains("Ranked"));
    assert!(
        Criteria::parse("status=ranked status=loved")
            .online_status
            .is_empty()
    );
    assert!(
        !Criteria::parse("status!=r")
            .online_status
            .contains("Ranked")
    );
    let both = Criteria::parse("status=ranked,loved").online_status;
    assert!(both.contains("Ranked") && both.contains("Loved"));
    let range = Criteria::parse("status>=r").online_status;
    assert_eq!(range.len(), 4);
    for s in ["Ranked", "Approved", "Qualified", "Loved"] {
        assert!(range.contains(s), "{s}");
    }
    assert_eq!(Criteria::parse("status>=r,l").online_status.len(), 9);
    let two = Criteria::parse("status>r status<l").online_status;
    assert_eq!(two.len(), 2);
    assert!(two.contains("Approved") && two.contains("Qualified"));
    let mixed = Criteria::parse("status>r status=loved").online_status;
    assert!(!mixed.is_empty() && mixed.contains("Loved"));
    // FilterMatchingTest (TestNotEqualSearchForEnumFilter): `l` is Loved, not
    // LocallyModified, as .NET lists the names by unsigned value.
    let not_loved = Criteria::parse("status!=l").online_status;
    assert!(!not_loved.contains("Loved") && not_loved.contains("LocallyModified"));
    let neither = Criteria::parse("status!=r,l").online_status;
    assert!(!neither.contains("Ranked") && !neither.contains("Loved"));
    assert_eq!(neither.len(), 7);
}

#[test]
fn creator_queries() {
    for key in ["creator", "author", "mapper"] {
        let c = Criteria::parse(&format!("beatmap specifically by {key}=my_fav"));
        assert_eq!(c.terms().len(), 3, "{key}");
        assert_eq!(c.term(&c.creator), "my_fav");
    }
}

#[test]
fn title_queries() {
    let c = Criteria::parse("find me songs with title=\"a certain title\" please");
    assert_eq!(c.terms().len(), 5);
    assert_eq!(c.term(&c.title), "a certain title");
    assert_eq!(c.title.mode, MatchMode::IsolatedPhrase);
}

#[test]
fn artist_queries() {
    let c = Criteria::parse("find me songs by artist=singer please");
    assert_eq!(c.terms().len(), 5);
    assert_eq!(c.term(&c.artist), "singer");
    assert_eq!(c.artist.mode, MatchMode::Substring);

    let c = Criteria::parse("really like artist=\"name with space\" yes");
    assert_eq!(c.terms().len(), 3);
    assert_eq!(c.term(&c.artist), "name with space");
    assert_eq!(c.artist.mode, MatchMode::IsolatedPhrase);

    let c = Criteria::parse("artist=\"The Only One\"!");
    assert_eq!(c.terms().len(), 0);
    assert_eq!(c.term(&c.artist), "the only one");
    assert_eq!(c.artist.mode, MatchMode::FullPhrase);

    let c = Criteria::parse("weird artist=double\"quote");
    assert_eq!(c.terms().len(), 1);
    assert_eq!(c.term(&c.artist), "double\"quote");

    let c = Criteria::parse("artist=><something");
    assert_eq!(c.term(&c.artist), "><something");
}

#[test]
fn unrecognised_or_invalid_keywords_stay_in_the_text() {
    let c = Criteria::parse("unrecognised=keyword");
    assert_eq!(terms(&c), [("unrecognised=keyword", MatchMode::Substring)]);
    for query in ["cs=nope", "bpm>=bad", "divisor<nah", "status=noidea"] {
        let c = Criteria::parse(query);
        assert_eq!(terms(&c), [(query, MatchMode::Substring)]);
    }
    // Text filters take only `=` and `!=`.
    let c = Criteria::parse("artist>x");
    assert!(!c.artist.has_filter());
    assert_eq!(c.terms().len(), 1);
}

#[test]
fn source_queries() {
    let c = Criteria::parse("find me songs with source=\"unit tests\" please");
    assert_eq!(c.terms().len(), 5);
    assert_eq!(c.term(&c.source), "unit tests");
    assert_eq!(c.source.mode, MatchMode::IsolatedPhrase);
}

#[test]
fn valid_date_ago_queries() {
    for value in [
        "600",
        "0.5s",
        "120m",
        "48h120s",
        "10y24M",
        "10y60d120s",
        "0y0M2d",
        "1y1M2d",
    ] {
        let c = Criteria::parse(&format!("lastplayed<{value} time"));
        assert!(c.last_played.has_filter(), "{value}");
        assert_eq!(c.terms().len(), 1, "{value}");
    }
}

#[test]
fn invalid_date_ago_queries() {
    for value in [
        ".5s",
        "7m27",
        "7m7m7m",
        "5s6m",
        "7d7y",
        "0:3:6",
        "0:3:",
        "\"three days\"",
        "0.1y0.1M2d",
        "0.99y0.99M2d",
        "",
    ] {
        let c = Criteria::parse(&format!("lastplayed<{value} time"));
        assert!(!c.last_played.has_filter(), "{value}");
    }
}

#[test]
fn date_ago_operators_flip() {
    let days = |n| {
        Some(PlayedBound::Ago(Ago {
            days: Some(n),
            ..Ago::default()
        }))
    };
    // "More than 50 days ago" is before 50 days ago.
    let c = Criteria::parse("lastplayed>50");
    assert_eq!((c.last_played.min, c.last_played.max), (None, days(50.0)));
    let c = Criteria::parse("lastplayed<50");
    assert_eq!((c.last_played.min, c.last_played.max), (days(50.0), None));
    let c = Criteria::parse("lastplayed>3M lastplayed<1y6M");
    let ago = |years, months| {
        Some(PlayedBound::Ago(Ago {
            years,
            months: Some(months),
            ..Ago::default()
        }))
    };
    assert_eq!(c.last_played.min, ago(Some(1), 6));
    assert_eq!(c.last_played.max, ago(None, 3));
    assert!(!Criteria::parse("lastplayed=50").last_played.has_filter());
    assert!(
        Criteria::parse("lastplayed<10000y")
            .last_played
            .has_filter()
    );
}

#[test]
fn ranked_date_queries() {
    let date = |year, month, day| Some(Date { year, month, day });
    // (query, min, max)
    let cases = [
        ("ranked<2012", None, date(2012, 1, 1)),
        ("ranked<2012.03", None, date(2012, 3, 1)),
        ("ranked<2012/03/05", None, date(2012, 3, 5)),
        ("ranked<2012-3-5", None, date(2012, 3, 5)),
        ("ranked<=2012", None, date(2013, 1, 1)),
        ("ranked<=2012.03", None, date(2012, 4, 1)),
        ("ranked<=2012/03/05", None, date(2012, 3, 6)),
        ("ranked<=2012-3-5", None, date(2012, 3, 6)),
        ("ranked>2012", date(2013, 1, 1), None),
        ("ranked>2012.03", date(2012, 4, 1), None),
        ("ranked>2012/03/05", date(2012, 3, 6), None),
        ("ranked>2012-3-5", date(2012, 3, 6), None),
        ("ranked>=2012", date(2012, 1, 1), None),
        ("ranked>=2012.03", date(2012, 3, 1), None),
        ("ranked>=2012/03/05", date(2012, 3, 5), None),
        ("ranked>=2012-3-5", date(2012, 3, 5), None),
        ("ranked=2012", date(2012, 1, 1), date(2013, 1, 1)),
        ("ranked=2012-03", date(2012, 3, 1), date(2012, 4, 1)),
        ("ranked=2012-03-05", date(2012, 3, 5), date(2012, 3, 6)),
    ];
    for (query, min, max) in cases {
        let r = Criteria::parse(query).date_ranked;
        assert_eq!((r.min, r.max), (min, max), "{query}");
    }
    // The bounds a period ends with are exclusive; the ones it starts with, inclusive.
    let r = Criteria::parse("ranked=2012-12-31").date_ranked;
    assert_eq!((r.min, r.max), (date(2012, 12, 31), date(2013, 1, 1)));
    assert!(r.lower_inclusive && !r.upper_inclusive);
    assert_eq!(
        Criteria::parse("ranked<=2012-02-29").date_ranked.max,
        date(2012, 3, 1)
    );
    for query in [
        "ranked<0",
        "ranked=99999",
        "ranked>=2012-03-05-04",
        "ranked=2013-02-29",
    ] {
        assert!(!Criteria::parse(query).date_ranked.has_filter(), "{query}");
    }
    for key in ["submitted", "created"] {
        for value in ["<2012", "<2012.03", "<2012/03/05", "<2012-3-5"] {
            let query = format!("{key}{value}");
            assert!(
                Criteria::parse(&query).date_submitted.has_filter(),
                "{query}"
            );
        }
        for value in ["<0", "=99999", ">=2012-03-05-04", ">=2012/03.05-04"] {
            let query = format!("{key}{value}");
            assert!(
                !Criteria::parse(&query).date_submitted.has_filter(),
                "{query}"
            );
        }
    }
}

#[test]
fn played_queries() {
    // (value, min inclusive, max is "never")
    for value in ["0", "false", "no"] {
        let r = Criteria::parse(&format!("played={value}")).last_played;
        assert_eq!(
            (r.min, r.max),
            (Some(PlayedBound::Never), Some(PlayedBound::Never))
        );
        assert!(r.lower_inclusive && r.upper_inclusive, "{value}");
    }
    for value in ["1", "true", "yes"] {
        let r = Criteria::parse(&format!("played={value}")).last_played;
        assert_eq!(
            (r.min, r.max),
            (Some(PlayedBound::Never), Some(PlayedBound::Last))
        );
        assert!(!r.lower_inclusive, "{value}");
    }
    let r = Criteria::parse("played!=1").last_played;
    assert_eq!(r.max, Some(PlayedBound::Never));
    assert!(!Criteria::parse("played=maybe").last_played.has_filter());
}

#[test]
fn multiple_tag_filters() {
    let c = Criteria::parse("tag=\"simple\" tag=\"clean\"!");
    let tags: Vec<_> = c.tags().iter().map(|t| (c.term(t), t.mode)).collect();
    assert_eq!(
        tags,
        [
            ("simple", MatchMode::IsolatedPhrase),
            ("clean", MatchMode::FullPhrase)
        ]
    );
    assert_eq!(c.terms().len(), 0);
}

#[test]
fn difficulty_in_brackets() {
    let diff = |query: &str| {
        let c = Criteria::parse(query);
        (c.term(&c.difficulty_name).to_owned(), terms(&c).len())
    };
    assert_eq!(diff("[1]"), ("1".into(), 0));
    assert_eq!(diff("[1"), ("1".into(), 0));
    assert_eq!(diff("Title1 [Diff]"), ("diff".into(), 1));
    // Without white space before it, a bracket is part of a word.
    assert_eq!(diff("Title1[Diff]"), ("".into(), 1));
    assert_eq!(
        diff("[diff ]with [[ brackets]]]]"),
        ("diff ]with [[ brackets".into(), 0)
    );
    // The bracket overrides `diff=`.
    assert_eq!(diff("diff=x [y]"), ("y".into(), 0));
}

#[test]
fn a_single_number_is_also_an_id() {
    assert_eq!(Criteria::parse("202010").search_number, Some(202_010));
    assert_eq!(Criteria::parse("\"1535\"").search_number, Some(1535));
    assert_eq!(Criteria::parse("-1").search_number, Some(-1));
    assert_eq!(Criteria::parse("1 2").search_number, None);
    assert_eq!(Criteria::parse("12a").search_number, None);
    assert_eq!(Criteria::parse("99999999999").search_number, None);
}

#[test]
fn keywords_are_case_insensitive_and_cut_everywhere() {
    let c = Criteria::parse("AR>9 x AR>9");
    assert!(c.approach_rate.has_filter());
    assert_eq!(terms(&c), [("x", MatchMode::Substring)]);
    // Text that looks like a keyword inside a word is not one.
    let c = Criteria::parse("x.ar>9");
    assert!(c.approach_rate.has_filter());
    assert_eq!(terms(&c), [("x.", MatchMode::Substring)]);
}
