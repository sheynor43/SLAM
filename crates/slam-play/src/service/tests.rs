use super::*;

/// A beatmap with `countdown`, `offset`, a 500 ms beat from 0 and circles at `objects`, and
/// `breaks`.
fn beatmap(countdown: i32, offset: i32, objects: &[i32], breaks: &[(i32, i32)]) -> Beatmap {
    let mut osu = format!(
        "osu file format v14\n\n[General]\nCountdown: {countdown}\nCountdownOffset: {offset}\n\n\
         [Difficulty]\nApproachRate:5\n\n[Events]\n"
    );
    for (start, end) in breaks {
        osu.push_str(&format!("2,{start},{end}\n"));
    }
    osu.push_str("\n[TimingPoints]\n0,500,4,1,0,100,1,0\n\n[HitObjects]\n");
    for time in objects {
        osu.push_str(&format!("256,192,{time},1,0\n"));
    }
    Beatmap::decode(osu.as_bytes()).expect("test beatmap decodes")
}

#[test]
fn countdown_speeds_scale_the_beat() {
    // Five steps ending at the first object.
    assert_eq!(
        Countdown::new(1, 0, 10_000.0, 500.0, 0.0),
        Some(Countdown {
            start: 7_500.0,
            beat: 500.0
        })
    );
    assert_eq!(
        Countdown::new(2, 0, 10_000.0, 500.0, 0.0),
        Some(Countdown {
            start: 5_000.0,
            beat: 1_000.0
        })
    );
    assert_eq!(
        Countdown::new(3, 0, 10_000.0, 500.0, 0.0),
        Some(Countdown {
            start: 8_750.0,
            beat: 250.0
        })
    );
}

#[test]
fn no_countdown_without_the_setting() {
    assert_eq!(Countdown::new(0, 0, 10_000.0, 500.0, 0.0), None);
    assert_eq!(Countdown::new(4, 0, 10_000.0, 500.0, 0.0), None);
    assert_eq!(Countdown::new(-1, 0, 10_000.0, 500.0, 0.0), None);
}

#[test]
fn countdown_offset_ends_it_beats_earlier() {
    let countdown = Countdown::new(1, 2, 10_000.0, 500.0, 0.0).unwrap();
    assert_eq!(countdown.start, 6_500.0);
    // `go` ends two beats before the object.
    assert_eq!(countdown.step_at(8_999.0), Some((4, 8_500.0)));
    assert_eq!(countdown.step_at(9_000.0), None);
}

#[test]
fn countdown_that_does_not_fit_is_left_out() {
    // Needs 2500 ms before the object.
    assert!(Countdown::new(1, 0, 3_000.0, 500.0, 500.0).is_some());
    assert_eq!(Countdown::new(1, 0, 3_000.0, 500.0, 500.1), None);
}

#[test]
fn countdown_steps_follow_the_beats() {
    let countdown = Countdown::new(1, 0, 10_000.0, 500.0, 0.0).unwrap();
    assert_eq!(countdown.step_at(7_499.9), None);
    assert_eq!(countdown.step_at(7_500.0), Some((0, 7_500.0)));
    assert_eq!(countdown.step_at(8_000.0), Some((1, 8_000.0)));
    assert_eq!(countdown.step_at(8_700.0), Some((2, 8_500.0)));
    assert_eq!(countdown.step_at(9_000.0), Some((3, 9_000.0)));
    assert_eq!(countdown.step_at(9_999.0), Some((4, 9_500.0)));
    assert_eq!(countdown.step_at(10_000.0), None);
}

#[test]
fn countdown_step_fades_over_its_last_quarter() {
    let countdown = Countdown::new(1, 0, 10_000.0, 500.0, 0.0).unwrap();
    assert_eq!(countdown.sprite_at(7_500.0), Some((0, 1.0)));
    assert_eq!(countdown.sprite_at(7_875.0), Some((0, 1.0)));
    assert_eq!(countdown.sprite_at(7_937.5), Some((0, 0.5)));
}

#[test]
fn countdown_uses_the_timing_point_at_the_first_object() {
    let map = beatmap(1, 0, &[10_000], &[]);
    assert_eq!(
        Countdown::of_beatmap(&map, 0.0),
        Some(Countdown {
            start: 7_500.0,
            beat: 500.0
        })
    );
    assert_eq!(
        Countdown::of_beatmap(&beatmap(0, 0, &[10_000], &[]), 0.0),
        None
    );
}

#[test]
fn each_countdown_step_sounds_once() {
    let mut tracker = ServiceTracker::new(&beatmap(1, 0, &[10_000], &[]), 0.0);
    let mut played = Vec::new();
    let mut time = 7_000.0;
    while time < 10_000.0 {
        let (_, sounds) = tracker.update(time, 1.0, false, None);
        played.extend(sounds.into_iter().flatten());
        time += 1.0;
    }
    let steps: Vec<_> = (0..COUNTDOWN_STEPS).map(ServiceSound::Countdown).collect();
    assert_eq!(played, steps);
}

#[test]
fn countdown_steps_skipped_over_stay_quiet() {
    let mut tracker = ServiceTracker::new(&beatmap(1, 0, &[10_000], &[]), 0.0);
    // A seek lands late in `count3`: it stays quiet, `count2` sounds.
    let (_, sounds) = tracker.update(8_400.0, 1.0, false, None);
    assert_eq!(sounds, [None, None]);
    let (_, sounds) = tracker.update(8_500.0, 1.0, false, None);
    assert_eq!(sounds[0], Some(ServiceSound::Countdown(2)));
    // A restart starts over: the steps sound again.
    tracker.reset();
    let (_, sounds) = tracker.update(7_500.0, 1.0, false, None);
    assert_eq!(sounds[0], Some(ServiceSound::Countdown(0)));
}

#[test]
fn arrows_blink_seven_times_before_the_next_object() {
    let windows: Vec<f64> = (0..2_000)
        .map(f64::from)
        .filter(|&delta| arrows_visible(10_000.0, 10_000.0 - delta))
        .collect();
    assert_eq!(windows.len(), 7 * 100);
    assert!(!arrows_visible(10_000.0, 10_000.0 - 400.0));
    assert!(arrows_visible(10_000.0, 10_000.0 - 401.0));
    assert!(arrows_visible(10_000.0, 10_000.0 - 500.0));
    assert!(!arrows_visible(10_000.0, 10_000.0 - 501.0));
    assert!(arrows_visible(10_000.0, 10_000.0 - 1_700.0));
    assert!(!arrows_visible(10_000.0, 10_000.0 - 1_701.0));
}

#[test]
fn arrows_need_a_long_enough_gap() {
    // From the end of an object the gap counts a second later: 2000 ms is not enough.
    let mut tracker = ServiceTracker::new(&beatmap(0, 0, &[1_500, 3_500, 6_500], &[]), 0.0);
    assert_eq!(
        tracker.update(2_000.0, 1.0, false, None).0.arrows_before,
        None
    );
    assert_eq!(
        tracker.update(4_000.0, 1.0, false, None).0.arrows_before,
        Some(6_500.0)
    );
    // Before the first object the gap counts from 0.
    tracker.reset();
    assert_eq!(
        tracker.update(0.0, 1.0, false, None).0.arrows_before,
        Some(1_500.0)
    );
    // After the last object there are none.
    assert_eq!(
        tracker.update(7_000.0, 1.0, false, None).0.arrows_before,
        None
    );
}

/// A play with circles at 1000 and 11 000 and a break in between.
fn break_map() -> Beatmap {
    beatmap(0, 0, &[1_000, 11_000, 12_000], &[(1_500, 10_500)])
}

#[test]
fn section_starts_in_the_middle_of_the_gap() {
    let mut tracker = ServiceTracker::new(&break_map(), 0.0);
    // Halfway between 1000 and 11 000.
    assert_eq!(tracker.update(5_999.0, 1.0, false, None).0.section, None);
    assert_eq!(
        tracker.update(6_001.0, 1.0, false, None).0.section,
        Some(Section {
            start: 6_000.0,
            pass: true
        })
    );
}

#[test]
fn section_passes_from_half_health() {
    let mut tracker = ServiceTracker::new(&break_map(), 0.0);
    let pass = |health| {
        let mut tracker = ServiceTracker::new(&break_map(), 0.0);
        tracker
            .update(6_500.0, health, false, None)
            .0
            .section
            .map(|s| s.pass)
    };
    assert_eq!(pass(0.5), Some(true));
    assert_eq!(pass(0.49), Some(false));
    // No section once the play failed.
    assert_eq!(tracker.update(6_500.0, 0.0, true, None).0.section, None);
}

#[test]
fn section_needs_a_long_break() {
    let short = beatmap(0, 0, &[1_000, 11_000], &[(5_000, 7_000)]);
    let mut tracker = ServiceTracker::new(&short, 0.0);
    assert_eq!(tracker.update(6_500.0, 1.0, false, None).0.section, None);
    let none = beatmap(0, 0, &[1_000, 11_000], &[]);
    let mut tracker = ServiceTracker::new(&none, 0.0);
    assert_eq!(tracker.update(6_500.0, 1.0, false, None).0.section, None);
}

#[test]
fn section_sound_plays_once_by_the_health_then() {
    let mut tracker = ServiceTracker::new(&break_map(), 0.0);
    let mut played = Vec::new();
    for ms in 6_000..8_000 {
        // Health drops below half after the pass sound came due.
        let health = if ms < 6_100 { 1.0 } else { 0.2 };
        let (_, sounds) = tracker.update(f64::from(ms), health, false, None);
        played.extend(sounds.into_iter().flatten());
    }
    assert_eq!(played, [ServiceSound::SectionPass]);

    let mut tracker = ServiceTracker::new(&break_map(), 0.0);
    assert_eq!(tracker.update(6_129.0, 0.2, false, None).1, [None, None]);
    assert_eq!(
        tracker.update(6_130.0, 0.2, false, None).1,
        [None, Some(ServiceSound::SectionFail)]
    );
}

#[test]
fn section_sound_skipped_over_stays_quiet() {
    let mut tracker = ServiceTracker::new(&break_map(), 0.0);
    assert_eq!(tracker.update(8_000.0, 1.0, false, None).1, [None, None]);
}

#[test]
fn section_blinks_then_fades() {
    let pass = Section {
        start: 0.0,
        pass: true,
    };
    let fail = Section {
        start: 0.0,
        pass: false,
    };
    assert_eq!(pass.alpha_at(10.0), 0.0);
    assert_eq!(pass.alpha_at(50.0), 1.0);
    assert_eq!(pass.alpha_at(130.0), 0.0);
    assert_eq!(fail.alpha_at(50.0), 0.0);
    assert_eq!(fail.alpha_at(150.0), 1.0);
    assert_eq!(pass.alpha_at(250.0), 0.0);
    assert_eq!(pass.alpha_at(1_000.0), 1.0);
    assert_eq!(pass.alpha_at(1_380.0), 0.5);
    assert_eq!(pass.alpha_at(1_500.0), 0.0);
}

#[test]
fn view_draws_the_mirrored_arrows_and_the_countdown_centred() {
    use slam_render::atlas::UvRect;
    use slam_render::textures::TextureId;

    let sprite = |w: f32, h: f32| SkinSprite {
        texture: TextureId(1),
        uv: UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
        size: slam_osu::Vec2::new(w, h),
    };
    let textures = ServiceTextures {
        countdown: [Some(sprite(200.0, 100.0)); COUNTDOWN_STEPS],
        warning_arrow: Some(sprite(64.0, 64.0)),
        ..ServiceTextures::default()
    };
    let state = ServiceState {
        countdown: Countdown::new(1, 0, 10_000.0, 500.0, 0.0),
        arrows_before: Some(10_000.0),
        ..ServiceState::default()
    };
    let view = ServiceView::new(1024.0, 768.0);
    let mut batch = SpriteBatch::with_capacity(MAX_SERVICE_SPRITES);
    // `count1` with the arrows' blink 850 ms before the object.
    view.push(&mut batch, &state, 9_150.0, &textures);
    // Four corners per sprite, the first the top left.
    let corner = |i: usize| batch.vertices()[i * 4];
    assert_eq!(batch.len(), 5);
    // At 1024×768 a skin pixel is a window pixel.
    assert_eq!(corner(0).position, [412.0, 334.0]);
    assert_eq!(batch.vertices()[2].position, [612.0, 434.0]);
    // 78 McOsu units (1.6 px each) square, 28 from the sides; the right ones mirrored.
    let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
    let side = 78.0 * 1.6;
    assert!(near(corner(1).position[0], 44.8));
    assert!(near(batch.vertices()[4 + 2].position[0], 44.8 + side));
    assert!(near(corner(2).position[0] + side, 1024.0 - 44.8));
    assert_eq!(corner(1).uv[0], 0.0);
    assert_eq!(corner(2).uv[0], 1.0);
}

#[test]
fn countdown_with_a_negative_offset_ends_after_the_first_object() {
    let countdown = Countdown::new(1, -1, 10_000.0, 500.0, 0.0).unwrap();
    assert_eq!(countdown.start, 8_000.0);
}

#[test]
fn arrows_gap_must_exceed_a_second() {
    // Before the first object: a gap of exactly 1000 ms is not enough.
    let mut tracker = ServiceTracker::new(&beatmap(0, 0, &[1_000], &[]), 0.0);
    assert_eq!(tracker.update(0.0, 1.0, false, None).0.arrows_before, None);
    let mut tracker = ServiceTracker::new(&beatmap(0, 0, &[1_001], &[]), 0.0);
    assert_eq!(
        tracker.update(0.0, 1.0, false, None).0.arrows_before,
        Some(1_001.0)
    );
}

#[test]
fn section_in_a_short_gap_starts_2880_ms_before_the_next_object() {
    // A 5000 ms gap: half of it is under 2880 ms.
    let map = beatmap(0, 0, &[1_000, 6_000, 7_000], &[(1_200, 5_800)]);
    let mut tracker = ServiceTracker::new(&map, 0.0);
    assert_eq!(
        tracker.update(3_500.0, 1.0, false, None).0.section,
        Some(Section {
            start: 3_120.0,
            pass: true
        })
    );
}

#[test]
fn section_needs_more_than_2880_ms() {
    // A gap of exactly 2880 ms.
    let map = beatmap(0, 0, &[1_000, 3_880, 5_000], &[(1_000, 3_880)]);
    let mut tracker = ServiceTracker::new(&map, 0.0);
    assert_eq!(tracker.update(1_500.0, 1.0, false, None).0.section, None);
    // A long gap with a break of exactly 2880 ms.
    let map = beatmap(0, 0, &[1_000, 11_000, 12_000], &[(5_000, 7_880)]);
    let mut tracker = ServiceTracker::new(&map, 0.0);
    assert_eq!(tracker.update(6_500.0, 1.0, false, None).0.section, None);
}

#[test]
fn section_takes_the_break_that_has_started() {
    // Two breaks in one gap: the second one counts once the time reaches it.
    let map = beatmap(
        0,
        0,
        &[1_000, 21_000, 22_000],
        &[(1_500, 4_000), (5_000, 20_500)],
    );
    let mut tracker = ServiceTracker::new(&map, 0.0);
    assert!(
        tracker
            .update(11_500.0, 1.0, false, None)
            .0
            .section
            .is_some()
    );
    // In the first, short break the section does not show (it has not started anyway).
    let mut tracker = ServiceTracker::new(&map, 0.0);
    assert_eq!(tracker.update(3_000.0, 1.0, false, None).0.section, None);
}

#[test]
fn options_turn_the_elements_off() {
    let map = beatmap(1, 0, &[10_000, 21_000, 22_000], &[(10_500, 20_500)]);
    let off = ServiceOptions {
        warning_arrows: false,
        countdown: false,
        sounds: true,
        break_display: BreakDisplay::Off,
    };
    let mut tracker = ServiceTracker::with_options(&map, 0.0, off);
    let (state, sounds) = tracker.update(7_500.0, 1.0, false, None);
    assert_eq!(state.countdown, None);
    assert_eq!(sounds, [None, None]);
    assert_eq!(tracker.update(0.0, 1.0, false, None).0.arrows_before, None);
    assert_eq!(tracker.update(16_000.0, 1.0, false, None).0.section, None);
    // Turned back on mid-play.
    tracker.set_options(ServiceOptions::default());
    assert!(
        tracker
            .update(16_100.0, 1.0, false, None)
            .0
            .section
            .is_some()
    );
}

#[test]
fn without_sounds_the_images_stay() {
    let quiet = ServiceOptions {
        sounds: false,
        ..ServiceOptions::default()
    };
    let mut tracker = ServiceTracker::with_options(&beatmap(1, 0, &[10_000], &[]), 0.0, quiet);
    let (state, sounds) = tracker.update(7_500.0, 1.0, false, None);
    assert!(state.countdown.is_some());
    assert_eq!(sounds, [None, None]);
}

#[test]
fn the_time_left_shows_the_break_period_until_it_has_faded_out() {
    let lazer = ServiceOptions {
        break_display: BreakDisplay::Lazer,
        ..ServiceOptions::default()
    };
    let mut tracker = ServiceTracker::with_options(&break_map(), 0.0, lazer);
    let period = Period {
        start: 1_500.0,
        end: 10_500.0 - BREAK_FADE_DURATION,
    };
    let (state, _) = tracker.update(5_000.0, 1.0, false, Some(period));
    assert_eq!(state.break_period, Some(period));
    assert_eq!(state.break_display, BreakDisplay::Lazer);
    // No section with the time left.
    assert_eq!(state.section, None);
    // The tracker has left the period; it shows while fading out.
    assert_eq!(
        tracker
            .update(period.end + 100.0, 1.0, false, None)
            .0
            .break_period,
        Some(period)
    );
    assert_eq!(
        tracker
            .update(period.end + BREAK_FADE_DURATION + 1.0, 1.0, false, None)
            .0
            .break_period,
        None
    );
    // With the section display the period is not shown.
    let mut tracker = ServiceTracker::new(&break_map(), 0.0);
    assert_eq!(
        tracker
            .update(5_000.0, 1.0, false, Some(period))
            .0
            .break_period,
        None
    );
}

#[test]
fn a_section_turned_on_mid_break_does_not_sound_late() {
    let lazer = ServiceOptions {
        break_display: BreakDisplay::Lazer,
        ..ServiceOptions::default()
    };
    let mut tracker = ServiceTracker::with_options(&break_map(), 0.0, lazer);
    // The pass sound comes due at 6020 while the time left shows: silent.
    assert_eq!(tracker.update(6_050.0, 1.0, false, None).1, [None, None]);
    tracker.set_options(ServiceOptions::default());
    let (state, sounds) = tracker.update(6_100.0, 1.0, false, None);
    assert!(state.section.is_some());
    assert_eq!(sounds, [None, None]);
}
