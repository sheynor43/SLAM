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

fn da(m: &Mod) -> DifficultyAdjustSettings {
    match m {
        Mod::DifficultyAdjust(d) => *d,
        _ => panic!("not DA: {m:?}"),
    }
}

#[test]
fn difficulty_adjust_reads_nullable_floats() {
    let mut m = Mod::from_acronym("DA");
    assert!(m.uses_default_configuration());

    assert!(m.set_setting("circle_size", &SettingValue::Float(4.2)));
    assert!(m.set_setting("approach_rate", &SettingValue::Int(-11)));
    assert!(m.set_setting("drain_rate", &SettingValue::String(" 12.5 ".into())));
    assert!(m.set_setting("overall_difficulty", &SettingValue::Bool(true)));
    let d = da(&m);
    assert_eq!(d.circle_size, Some(4.2_f64 as f32));
    // Clamped to the extended bounds even without `extended_limits`.
    assert_eq!(d.approach_rate, Some(-10.0));
    assert_eq!(d.drain_rate, Some(11.0));
    assert_eq!(d.overall_difficulty, Some(1.0));
    assert!(!d.extended_limits);

    // `null` and the empty string unset a value; garbage keeps it.
    assert!(m.set_setting("circle_size", &SettingValue::Null));
    assert!(m.set_setting("drain_rate", &SettingValue::String(String::new())));
    assert!(!m.set_setting("approach_rate", &SettingValue::String("fast".into())));
    let d = da(&m);
    assert_eq!((d.circle_size, d.drain_rate), (None, None));
    assert_eq!(d.approach_rate, Some(-10.0));

    // Circle Size has no extended minimum.
    assert!(m.set_setting("circle_size", &SettingValue::Float(-3.0)));
    assert_eq!(da(&m).circle_size, Some(0.0));
    assert!(m.set_setting("circle_size", &SettingValue::String("NaN".into())));
    assert!(da(&m).circle_size.unwrap().is_nan());
}

#[test]
fn mirror_reads_enum_names_and_numbers() {
    let read = |v: SettingValue| {
        let mut m = Mod::from_acronym("MR");
        m.set_setting("reflection", &v);
        match m {
            Mod::Mirror { reflection } => reflection,
            _ => unreachable!(),
        }
    };
    let s = |t: &str| SettingValue::String(t.into());
    assert_eq!(read(SettingValue::Int(1)), MirrorType::Vertical);
    assert_eq!(read(SettingValue::Int(2)), MirrorType::Both);
    assert_eq!(read(SettingValue::Int(7)), MirrorType::Undefined(7));
    assert_eq!(read(SettingValue::Float(2.0)), MirrorType::Both);
    assert_eq!(read(SettingValue::Float(1.5)), MirrorType::Horizontal);
    assert_eq!(read(SettingValue::Bool(true)), MirrorType::Horizontal);
    assert_eq!(read(SettingValue::Null), MirrorType::Horizontal);
    assert_eq!(read(s("Vertical")), MirrorType::Vertical);
    assert_eq!(read(s(" Both ")), MirrorType::Both);
    // Names are case-sensitive; a list combines its values.
    assert_eq!(read(s("vertical")), MirrorType::Horizontal);
    assert_eq!(read(s("Vertical, Both")), MirrorType::Undefined(3));
    assert_eq!(read(s(" 1 ")), MirrorType::Vertical);
    assert_eq!(read(s("-1")), MirrorType::Undefined(-1));
}

#[test]
fn map_mod_settings_round_trip_through_api() {
    let mods = [
        Mod::DifficultyAdjust(DifficultyAdjustSettings {
            circle_size: Some(8.3),
            approach_rate: Some(-5.5),
            drain_rate: None,
            overall_difficulty: Some(10.7),
            extended_limits: true,
        }),
        Mod::Mirror {
            reflection: MirrorType::Both,
        },
        Mod::Mirror {
            reflection: MirrorType::Undefined(9),
        },
    ];
    for m in mods {
        let api = m.to_api();
        assert!(!api.settings.is_empty());
        assert_eq!(Mod::from_api(&api), m);
    }

    // A float is written as the double of its shortest text, as Newtonsoft writes it.
    let m = Mod::DifficultyAdjust(DifficultyAdjustSettings {
        circle_size: Some(8.3),
        ..DifficultyAdjustSettings::default()
    });
    assert_eq!(
        m.settings(),
        [("circle_size".to_owned(), SettingValue::Float(8.3))]
    );
    for acronym in ["DA", "MR"] {
        assert!(Mod::from_acronym(acronym).uses_default_configuration());
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModHardRock.cs, osu.Game.Rulesets.Osu/Mods/OsuModEasy.cs
#[test]
fn difficulty_mods_change_settings() {
    let apply = |mods: &[Mod]| {
        let mut d = difficulty();
        for m in mods {
            m.apply_to_difficulty(&mut d);
        }
        (
            d.circle_size,
            d.approach_rate,
            d.drain_rate,
            d.overall_difficulty,
        )
    };

    assert_eq!(apply(&[Mod::HardRock]), (4.0 * 1.3, 10.0, 5.0 * 1.4, 10.0));
    assert_eq!(apply(&[Mod::from_acronym("EZ")]), (2.0, 4.5, 2.5, 4.0));
    // In mod order: Easy halves what Hard Rock raised.
    assert_eq!(
        apply(&[Mod::HardRock, Mod::from_acronym("EZ")]),
        (4.0 * 1.3 * 0.5, 5.0, 5.0 * 1.4 * 0.5, 5.0)
    );
    let mut da = Mod::from_acronym("DA");
    da.set_setting("approach_rate", &SettingValue::Float(10.5));
    da.set_setting("drain_rate", &SettingValue::Float(2.0));
    assert_eq!(apply(&[da.clone()]), (4.0, 10.5, 2.0, 8.0));
    assert_eq!(
        apply(&[da, Mod::HardRock]),
        (4.0 * 1.3, 10.0, 2.0 * 1.4, 10.0)
    );
    // Mirror changes no difficulty.
    assert_eq!(apply(&[Mod::from_acronym("MR")]), (4.0, 9.0, 5.0, 8.0));
}

#[test]
fn difficulty_adjust_multipliers() {
    let base = difficulty();
    let context = |version| MultiplierContext {
        version,
        difficulty_without_mods: &base,
    };
    let m = |settings| Mod::DifficultyAdjust(settings);

    let unchanged = m(DifficultyAdjustSettings::default());
    assert_eq!(
        unchanged.score_multiplier(&context(MultiplierVersion::V1)),
        0.5
    );
    assert_eq!(
        unchanged.score_multiplier(&context(MultiplierVersion::V2)),
        1.0
    );

    // 0.05x per 0.1 of change, per setting.
    let ar = m(DifficultyAdjustSettings {
        approach_rate: Some(9.3),
        ..DifficultyAdjustSettings::default()
    });
    let v2 = ar.score_multiplier(&context(MultiplierVersion::V2));
    assert!((v2 - 0.85).abs() < 1e-6, "{v2}");
    let far = m(DifficultyAdjustSettings {
        circle_size: Some(11.0),
        ..DifficultyAdjustSettings::default()
    });
    assert_eq!(far.score_multiplier(&context(MultiplierVersion::V2)), 0.1);
    assert_eq!(
        Mod::from_acronym("MR").score_multiplier(&context(MultiplierVersion::V2)),
        1.0
    );
}
