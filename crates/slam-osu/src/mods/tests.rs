use super::*;

fn acronyms(set: &ModSet) -> Vec<&str> {
    set.mods().iter().map(GameplayMod::acronym).collect()
}

fn difficulty() -> Difficulty {
    Difficulty {
        drain_rate: 5.0,
        circle_size: 4.0,
        overall_difficulty: 8.0,
        approach_rate: 9.0,
        slider_multiplier: 1.4,
        slider_tick_rate: 1.0,
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/OsuLegacyModConversionTest.cs
#[test]
fn legacy_bitmask_converts_like_lazer() {
    use legacy::*;
    let cases: &[(u32, &[&str])] = &[
        (NO_FAIL, &["NF"]),
        (EASY, &["EZ"]),
        (TOUCH_DEVICE, &["TD"]),
        (HIDDEN, &["HD"]),
        (HARD_ROCK, &["HR"]),
        (SUDDEN_DEATH, &["SD"]),
        (DOUBLE_TIME, &["DT"]),
        (RELAX, &["RX"]),
        (HALF_TIME, &["HT"]),
        (FLASHLIGHT, &["FL"]),
        (AUTOPLAY, &["AT"]),
        (SPUN_OUT, &["SO"]),
        (AUTOPILOT, &["AP"]),
        (TARGET, &["TP"]),
        (HARD_ROCK | DOUBLE_TIME, &["DT", "HR"]),
        (SCORE_V2, &["SV2"]),
        (CINEMA, &["CN"]),
        (CINEMA | AUTOPLAY, &["CN"]),
        (NIGHTCORE, &["NC"]),
        (NIGHTCORE | DOUBLE_TIME, &["NC"]),
        (PERFECT, &["PF"]),
        (PERFECT | SUDDEN_DEATH, &["PF"]),
    ];
    for (mask, expected) in cases {
        assert_eq!(
            acronyms(&ModSet::from_legacy(*mask)),
            *expected,
            "mask {mask:#x}"
        );
    }

    // Bits lazer does not convert for osu!standard: keys, Fade In, Random, Mirror.
    assert!(
        ModSet::from_legacy((1 << 15) | (1 << 20) | (1 << 21) | (1 << 30))
            .mods()
            .is_empty()
    );

    // The order of the yields in `ConvertFromLegacyMods`.
    let all = ModSet::from_legacy(u32::MAX);
    assert_eq!(
        acronyms(&all),
        [
            "NC", "PF", "AP", "CN", "EZ", "FL", "HT", "HR", "HD", "NF", "RX", "SO", "TP", "TD",
            "SV2"
        ]
    );
}

#[test]
fn stable_replays_get_classic() {
    let mask = legacy::HIDDEN | legacy::DOUBLE_TIME;
    let stable = ModSet::from_replay_parts(20_240_101, mask, None).unwrap();
    assert_eq!(acronyms(&stable), ["DT", "HD", "CL"]);
    assert_eq!(
        stable.get("CL"),
        Some(&Mod::Classic(ClassicSettings::default()))
    );

    // A lazer replay without a block keeps the bitmask only.
    let lazer = ModSet::from_replay_parts(FIRST_LAZER_VERSION, mask, None).unwrap();
    assert_eq!(acronyms(&lazer), ["DT", "HD"]);
}

#[test]
fn score_block_replaces_bitmask() {
    let info = ScoreInfo::from_json(
        r#"{"mods":[{"acronym":"HT","settings":{"speed_change":0.6,"adjust_pitch":true}},{"acronym":"XX","settings":{"a":1}}]}"#,
    )
    .unwrap();
    let set = ModSet::from_replay_parts(30_000_016, legacy::HARD_ROCK, Some(&info)).unwrap();
    assert_eq!(acronyms(&set), ["HT", "XX"]);
    assert_eq!(
        set.mods()[0],
        Mod::HalfTime(RateAdjust {
            speed_change: 0.6,
            adjust_pitch: true
        })
    );
    assert!(!set.is_supported());
    // Unknown mods keep their settings for writing back.
    assert_eq!(set.to_api(), info.mods);
}

#[test]
fn duplicate_acronyms_are_rejected() {
    let api = [
        Mod::from_acronym("DT").to_api(),
        Mod::from_acronym("DT").to_api(),
    ];
    assert!(matches!(ModSet::from_api(&api), Err(ModError::Duplicate(a)) if a == "DT"));
}

#[test]
fn settings_round_trip_through_api() {
    let mods = [
        Mod::DoubleTime(RateAdjust {
            speed_change: 1.35,
            adjust_pitch: true,
        }),
        Mod::Nightcore { speed_change: 1.2 },
        Mod::Easy { retries: 0 },
        Mod::Hidden {
            only_fade_approach_circles: true,
        },
        Mod::Classic(ClassicSettings {
            classic_note_lock: false,
            ..ClassicSettings::default()
        }),
    ];
    for m in mods {
        let api = m.to_api();
        assert!(!api.settings.is_empty());
        assert_eq!(Mod::from_api(&api), m);
    }

    // Defaults are not written, as in lazer's `APIMod(Mod)`.
    for acronym in ["NF", "EZ", "HT", "DC", "HR", "DT", "NC", "HD", "TC", "CL"] {
        let m = Mod::from_acronym(acronym);
        assert!(m.is_supported());
        assert!(m.uses_default_configuration(), "{acronym}");
        assert_eq!(m.to_api().acronym, acronym);
    }

    let dc = Mod::Classic(ClassicSettings {
        classic_health: false,
        ..ClassicSettings::default()
    });
    assert_eq!(
        dc.settings(),
        [("classic_health".to_owned(), SettingValue::Bool(false))]
    );
}

#[test]
fn unknown_keys_and_bad_values_keep_defaults() {
    let mut m = Mod::from_acronym("DT");
    assert!(!m.set_setting("speed", &SettingValue::Float(1.2)));
    assert!(!m.set_setting("speed_change", &SettingValue::Null));
    assert!(!m.set_setting("adjust_pitch", &SettingValue::String("yes".into())));
    assert_eq!(m, Mod::from_acronym("DT"));

    // Classic's last two settings are plain `Bindable<bool>`, which reject "1".
    let mut cl = Mod::from_acronym("CL");
    assert!(cl.set_setting("classic_note_lock", &SettingValue::String("0".into())));
    assert!(!cl.set_setting("classic_health", &SettingValue::String("0".into())));
    let Mod::Classic(c) = cl else { unreachable!() };
    assert!(!c.classic_note_lock && c.classic_health);
}

#[test]
fn rate_and_pitch() {
    let rate = |m: Mod| m.rate().unwrap();
    assert_eq!(
        rate(Mod::DoubleTime(RateAdjust {
            speed_change: 1.5,
            adjust_pitch: false
        })),
        RateChange {
            rate: 1.5,
            frequency: 1.0
        }
    );
    assert_eq!(
        rate(Mod::HalfTime(RateAdjust {
            speed_change: 0.6,
            adjust_pitch: true
        })),
        RateChange {
            rate: 0.6,
            frequency: 0.6
        }
    );
    assert_eq!(
        rate(Mod::Nightcore { speed_change: 1.2 }),
        RateChange {
            rate: 1.2,
            frequency: 1.5
        }
    );
    assert_eq!(
        rate(Mod::Daycore { speed_change: 0.9 }),
        RateChange {
            rate: 0.9,
            frequency: 0.75
        }
    );
    assert_eq!(Mod::HardRock.rate(), None);

    let set = ModSet::from_legacy(legacy::NIGHTCORE | legacy::HIDDEN);
    assert_eq!(
        set.rate(),
        RateChange {
            rate: 1.5,
            frequency: 1.5
        }
    );
    assert_eq!(
        ModSet::default().rate(),
        RateChange {
            rate: 1.0,
            frequency: 1.0
        }
    );
}

#[test]
fn no_fail_prevents_failing() {
    assert!(!ModSet::from_legacy(legacy::NO_FAIL | legacy::HIDDEN).perform_fail());
    assert!(ModSet::from_legacy(legacy::HIDDEN | legacy::EASY).perform_fail());
    assert!(ModSet::default().perform_fail());
}

#[test]
fn acronyms_ignore_letter_case() {
    assert_eq!(Mod::from_acronym("dt"), Mod::from_acronym("DT"));
    assert_eq!(Mod::from_acronym("dt").acronym(), "DT");
    assert_eq!(Mod::from_acronym("xx").acronym(), "xx");

    let api = [
        Mod::from_acronym("DT").to_api(),
        ApiMod {
            acronym: "dt".into(),
            ..ApiMod::default()
        },
    ];
    assert!(matches!(
        ModSet::from_api(&api),
        Err(ModError::Duplicate(_))
    ));
    let api = [
        ApiMod {
            acronym: "xx".into(),
            ..ApiMod::default()
        },
        ApiMod {
            acronym: "XX".into(),
            ..ApiMod::default()
        },
    ];
    assert!(matches!(
        ModSet::from_api(&api),
        Err(ModError::Duplicate(_))
    ));
}

#[test]
fn speed_defaults_allow_half_the_precision() {
    let dt = |speed_change| {
        Mod::DoubleTime(RateAdjust {
            speed_change,
            adjust_pitch: false,
        })
    };
    assert!(dt(1.504).uses_default_configuration());
    assert!(!dt(1.51).uses_default_configuration());
    assert!(
        Mod::Nightcore {
            speed_change: 1.4951
        }
        .uses_default_configuration()
    );
}

#[test]
fn set_multiplier_is_the_product_in_mod_order() {
    let difficulty = difficulty();
    let v2 = MultiplierContext {
        version: MultiplierVersion::V2,
        difficulty_without_mods: &difficulty,
    };
    let v1 = MultiplierContext {
        version: MultiplierVersion::V1,
        ..v2
    };

    // HDHRDT, as stable writes it: DT, HR, HD.
    let set = ModSet::from_legacy(legacy::HIDDEN | legacy::HARD_ROCK | legacy::DOUBLE_TIME);
    let dt = multiplier::double_time_v2(1.5);
    assert_eq!(set.score_multiplier(&v2), 1.0 * dt * 1.09 * 1.04);
    assert_eq!(
        set.score_multiplier(&v1),
        1.0 * multiplier::rate_adjust_v1(1.5) * 1.06 * 1.06
    );
    assert_eq!(ModSet::default().score_multiplier(&v2), 1.0);

    // Unsupported mods count as 1; Classic depends on its note lock in V2 only.
    let set = ModSet::from_replay_parts(20_240_101, legacy::SPUN_OUT, None).unwrap();
    assert_eq!(set.score_multiplier(&v2), 0.985);
    assert_eq!(set.score_multiplier(&v1), 0.96);
    let cl = Mod::Classic(ClassicSettings {
        classic_note_lock: false,
        ..ClassicSettings::default()
    });
    assert_eq!(cl.score_multiplier(&v2), 0.96);

    // V1 gives Hidden 1.06 only with its default settings; Traceable counts in V2 only.
    assert_eq!(
        Mod::Hidden {
            only_fade_approach_circles: true
        }
        .score_multiplier(&v1),
        1.0
    );
    assert_eq!(Mod::Traceable.score_multiplier(&v1), 1.0);
    assert_eq!(Mod::Traceable.score_multiplier(&v2), 1.02);
    assert_eq!(Mod::NoFail.score_multiplier(&v1), 0.5);
    assert_eq!(Mod::Easy { retries: 2 }.score_multiplier(&v1), 0.5);
}
