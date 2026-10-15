//! Spinner judgement on synthetic beatmaps through the replay simulator.
//!
//! The cases are lazer's `TestSceneSpinnerJudgement` and `TestSceneSpinnerInput`, with spins
//! from a port of lazer's `SpinFramesGenerator`, plus the result thresholds, the rate scaling and
//! the spin counts. Lazer's rewinding cases have no counterpart: time only moves forward here.

use std::f32::consts::PI;

use slam_formats::osr::SettingValue;
use slam_formats::osu::{Vec2, decode_str};
use slam_osu::Beatmap;
use slam_osu::judgement::HitResult;
use slam_osu::objects::{OsuHitObjectKind, Spinner};
use slam_osu::replay::{Actions, InputFrame, ReplayInput, Rules, Simulator};
use slam_osu::rules::{HitPolicy, LiveSpinner, OsuRules, RulesOptions};
use slam_osu::{Mod, ModSet};

use HitResult::{Great, IgnoreMiss, LargeBonus, Meh, Miss, Ok, SmallBonus};

const NONE: Actions = Actions::NONE;
const LEFT: Actions = Actions::LEFT;

/// One judgement of a play.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Judged {
    index: u32,
    /// The tick, or `None` for the object itself.
    tick: Option<u32>,
    result: HitResult,
    max_result: HitResult,
    time: f64,
}

/// A beatmap of `objects` with lazer's default difficulty (everything 5) but `od`.
fn beatmap(od: f32, objects: &str) -> Beatmap {
    let text = format!(
        "osu file format v14\n\n[Difficulty]\nHPDrainRate:5\nCircleSize:5\nOverallDifficulty:{od}\n\
         ApproachRate:5\nSliderMultiplier:1.4\nSliderTickRate:1\n\n[TimingPoints]\n\
         0,1000,4,2,0,100,1,0\n\n[HitObjects]\n{objects}"
    );
    Beatmap::from_file(decode_str(&text).unwrap().beatmap).unwrap()
}

/// A spinner at the playfield centre from `start` to `end`, at OD 5.
fn spinner_map(start: f64, end: f64) -> Beatmap {
    beatmap(5.0, &format!("256,192,{start},12,0,{end}\n"))
}

fn spinner(map: &Beatmap, index: usize) -> &Spinner {
    match &map.hit_objects[index].kind {
        OsuHitObjectKind::Spinner(s) => s,
        _ => panic!("object {index} is not a spinner"),
    }
}

fn frame(time: f64, x: f32, y: f32, actions: Actions) -> InputFrame {
    InputFrame {
        time,
        position: Vec2::new(x, y),
        actions,
    }
}

fn play(map: &Beatmap, options: RulesOptions, frames: &[InputFrame]) -> Vec<Judged> {
    let rules = OsuRules::with_options(map, options);
    let capacity = rules.max_judgements();
    let mut sim = Simulator::new(map, ReplayInput::new(frames.to_vec()), rules);
    sim.run();
    assert!(sim.is_finished());
    let events = sim.log().events();
    assert!(events.len() <= capacity);
    events
        .iter()
        .map(|j| Judged {
            index: j.object.index,
            tick: j.object.nested,
            result: j.result,
            max_result: j.max_result,
            time: j.time,
        })
        .collect()
}

/// [`play`] from the start of the beatmap. Lazer's tests play their replays in real time from
/// before the first object, so the spinner is alive when the first frame arrives; a catching up
/// simulation would jump straight to the first frame instead. A frame at 0 at the position of
/// the first one starts the replay early without moving the cursor.
fn play_from_start(map: &Beatmap, options: RulesOptions, frames: &[InputFrame]) -> Vec<Judged> {
    let mut all = vec![frame(0.0, frames[0].position.x, frames[0].position.y, NONE)];
    all.extend_from_slice(frames);
    play(map, options, &all)
}

fn lazer() -> RulesOptions {
    RulesOptions::with_policy(HitPolicy::StartTimeOrdered)
}

fn at_rate(rate: f64) -> RulesOptions {
    RulesOptions { rate, ..lazer() }
}

/// The results of the ticks of object `index`, in judgement order.
fn ticks(j: &[Judged], index: u32) -> Vec<HitResult> {
    j.iter()
        .filter(|x| x.index == index && x.tick.is_some())
        .map(|x| x.result)
        .collect()
}

/// The number of ticks hit (lazer's `assertTicksHit`; bonus ticks are ticks too).
fn ticks_hit(j: &[Judged], index: u32) -> usize {
    ticks(j, index).iter().filter(|r| r.is_hit()).count()
}

/// The judgement of object `index` itself.
fn result(j: &[Judged], index: u32) -> Judged {
    let mut it = j.iter().filter(|x| x.index == index && x.tick.is_none());
    let r = *it.next().expect("object judged");
    assert!(it.next().is_none(), "object judged twice");
    r
}

/// Lazer's `SpinFramesGenerator`: spins around the playfield centre, 50 px out, with a frame
/// every 10 ms and the left button held, then releases.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu.Tests/SpinFramesGenerator.cs
struct SpinFrames {
    start_time: f64,
    start_angle: f32,
    sequences: Vec<(f32, f64)>,
}

/// A little extra spin past each sequence, against floating point error.
const SPIN_ERROR: f32 = PI / 8.0;
const CENTRE_SPIN_OFFSET: f32 = 50.0;

/// `Math.Sign(float)`.
fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

impl SpinFrames {
    /// Spins starting at `start_time` from straight up the screen (the negative y axis).
    fn new(start_time: f64) -> SpinFrames {
        SpinFrames {
            start_time,
            start_angle: -PI / 2.0,
            sequences: Vec::new(),
        }
    }

    /// Spins `delta` degrees over `duration` ms.
    fn spin(mut self, delta: f32, duration: f64) -> SpinFrames {
        self.sequences.push((delta / 360.0 * 2.0 * PI, duration));
        self
    }

    fn build(&self) -> Vec<InputFrame> {
        let mut frames = Vec::new();
        let mut last_time = self.start_time;
        let mut last_angle = self.start_angle;
        let mut last_direction = 0;

        for &(delta_angle, duration) in &self.sequences {
            let seq_direction = sign(delta_angle);
            let mut seq_error = SPIN_ERROR * seq_direction as f32;
            if seq_direction == last_direction {
                // The error was already added by the previous sequence.
                seq_error = 0.0;
            } else if last_direction != 0 {
                // Changing direction: undo the previous error too.
                seq_error *= 2.0;
            }

            let seq_start_time = last_time;
            let seq_end_time = last_time + duration;
            let seq_start_angle = last_angle;
            let seq_end_angle = seq_start_angle + delta_angle + seq_error;

            while last_time < seq_end_time {
                let p = (last_time - seq_start_time) / (seq_end_time - seq_start_time);
                let pos = offset_at(p, seq_start_angle, seq_end_angle);
                frames.push(frame(last_time, pos.x, pos.y, LEFT));
                last_time += 10.0;
            }
            let pos = offset_at(1.0, seq_start_angle, seq_end_angle);
            frames.push(frame(seq_end_time, pos.x, pos.y, LEFT));

            last_time = seq_end_time;
            last_angle = seq_end_angle;
            last_direction = seq_direction;
        }

        // Release.
        if let Some(&last) = frames.last() {
            frames.push(frame(last.time, last.position.x, last.position.y, NONE));
        }
        frames
    }
}

fn offset_at(p: f64, start_angle: f32, end_angle: f32) -> Vec2 {
    let angle = start_angle + (end_angle - start_angle) * p as f32;
    // Correctly rounded, so the input is the same on every platform (ADR-0031).
    Vec2::new(256.0, 192.0)
        + Vec2::new(pxfm::f_cosf(angle), pxfm::f_sinf(angle)) * CENTRE_SPIN_OFFSET
}

// ---- Spin counts ----

#[test]
fn spin_counts_follow_od_and_duration() {
    // OD 5: 150 rpm to clear, 380 rpm to complete.
    let map = spinner_map(2000.0, 4000.0);
    let s = spinner(&map, 0);
    assert_eq!((s.spins_required, s.maximum_bonus_spins), (5, 5));
    assert_eq!(s.spins_required_for_bonus(), 7);
    assert_eq!(s.tick_count(), 12);
    assert!(!s.is_bonus_tick(6) && s.is_bonus_tick(7));

    // OD 10 for one second: 3.75 and 7.17 spins.
    let map = beatmap(10.0, "256,192,1000,12,0,2000\n");
    let s = spinner(&map, 0);
    assert_eq!((s.spins_required, s.maximum_bonus_spins), (3, 2));

    // Too short to require a spin: no bonus, but the two gap ticks remain.
    let map = beatmap(0.0, "256,192,1000,12,0,1300\n");
    let s = spinner(&map, 0);
    assert_eq!((s.spins_required, s.maximum_bonus_spins), (0, 0));
    assert_eq!(s.tick_count(), 2);

    // 0.1 ms of leniency: 399.97 ms at 2.5 rotations per second is one spin.
    let mut s = Spinner::new(1000.0, 399.97);
    s.apply_defaults(&map_difficulty(5.0));
    assert_eq!(s.spins_required, 1);
}

fn map_difficulty(od: f32) -> slam_osu::Difficulty {
    let mut d = spinner_map(0.0, 1.0).difficulty;
    d.overall_difficulty = od;
    d
}

#[test]
fn ticks_are_spread_evenly_up_to_the_end() {
    let map = spinner_map(2000.0, 4000.0);
    let s = spinner(&map, 0);
    // A float fraction of the duration, as lazer computes it.
    assert_eq!(
        s.tick_time(2000.0, 0),
        2000.0 + f64::from(1.0f32 / 12.0) * 2000.0
    );
    assert_eq!(s.tick_time(2000.0, 11), 4000.0);
}

// ---- TestSceneSpinnerJudgement ----

const JUDGEMENT_START: f64 = 2000.0;
const JUDGEMENT_END: f64 = 4000.0;

fn judgement_replay(spins: i32) -> Vec<InputFrame> {
    SpinFrames::new(JUDGEMENT_START)
        .spin(spins as f32 * 360.0, JUDGEMENT_END - JUDGEMENT_START)
        .build()
}

#[test]
fn hit_nothing_gives_min_results() {
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let j = play(&map, lazer(), &[]);
    assert_eq!(j.len(), 13);
    for x in &j {
        assert_eq!(x.result, x.max_result.min_result(), "{x:?}");
    }
    // The ticks are missed before the spinner is judged, once its end time has passed.
    let spinner = result(&j, 0);
    assert_eq!(j.last(), Some(&spinner));
    assert_eq!(spinner.result, Miss);
    assert!(spinner.time >= JUDGEMENT_END);
    assert!(j.iter().all(|x| x.time == spinner.time));
}

#[test]
fn number_of_spins_hits_as_many_ticks() {
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    for spins in [1, 2, 5] {
        let j = play_from_start(&map, lazer(), &judgement_replay(spins));
        let t = ticks(&j, 0);
        let spins = spins as usize;
        assert!(
            t[..spins].iter().all(|&r| r == SmallBonus),
            "{spins}: {t:?}"
        );
        assert_eq!(t[spins], IgnoreMiss, "{spins}");
    }
}

#[test]
fn hit_everything_gives_max_results() {
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let j = play_from_start(&map, lazer(), &judgement_replay(20));
    assert_eq!(j.len(), 13);
    for x in &j {
        assert_eq!(x.result, x.max_result, "{x:?}");
    }
    let t = ticks(&j, 0);
    assert_eq!(t.iter().filter(|&&r| r == SmallBonus).count(), 7);
    assert_eq!(t.iter().filter(|&&r| r == LargeBonus).count(), 5);
    assert_eq!(result(&j, 0).result, Great);
}

#[test]
fn ticks_are_judged_on_whole_spins() {
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let j = play_from_start(&map, lazer(), &judgement_replay(5));
    // Five spins (and the generator's extra 22.5°) over two seconds at a steady pace: each tick
    // is hit at the first step after its whole spin.
    let times: Vec<f64> = j
        .iter()
        .filter(|x| x.tick.is_some() && x.result.is_hit())
        .map(|x| x.time)
        .collect();
    assert_eq!(times.len(), 5);
    let duration = JUDGEMENT_END - JUDGEMENT_START;
    for (k, t) in times.iter().enumerate() {
        let expected = JUDGEMENT_START + duration * 360.0 * (k + 1) as f64 / (5.0 * 360.0 + 22.5);
        assert!(
            (t - expected).abs() <= 2.0,
            "spin {k} at {t}, expected {expected}"
        );
    }
}

// ---- Results by progress ----

#[test]
fn result_follows_the_fraction_of_required_spins() {
    // Five spins are required; the generator adds 22.5° to each spin.
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    for (degrees, expected, expected_ticks) in [
        (1800.0, Great, 5),
        // 1662.5°: over 90%.
        (1640.0, Ok, 4),
        // 1422.5°: over 75%.
        (1400.0, Meh, 3),
        // 1322.5°: under 75%.
        (1300.0, Miss, 3),
    ] {
        let frames = SpinFrames::new(JUDGEMENT_START)
            .spin(degrees, 1000.0)
            .build();
        let j = play_from_start(&map, lazer(), &frames);
        assert_eq!(result(&j, 0).result, expected, "{degrees}°");
        assert_eq!(ticks_hit(&j, 0), expected_ticks, "{degrees}°");
    }
}

#[test]
fn a_spinner_too_short_for_a_spin_is_hit_without_spinning() {
    let map = beatmap(0.0, "256,192,1000,12,0,1300\n");
    let j = play(&map, lazer(), &[]);
    assert_eq!(result(&j, 0).result, Great);
    assert_eq!(ticks(&j, 0), [IgnoreMiss, IgnoreMiss]);
}

// ---- TestSceneSpinnerInput ----

const INPUT_START: f64 = 1500.0;
const INPUT_END: f64 = 8000.0;
const CENTRE_X: f32 = 256.0;
const CENTRE_Y: f32 = 192.0;

fn input_map() -> Beatmap {
    spinner_map(INPUT_START, INPUT_END)
}

/// Moves back and forth along the x axis between `CENTRE_X - 50` and `CENTRE_X + 50` at `y`,
/// every `vibrate_time` ms.
fn vibrate(y: f32, vibrate_time: f64) -> Vec<InputFrame> {
    let mut frames = Vec::new();
    let mut direction = -1.0;
    let mut i = INPUT_START;
    while i <= INPUT_END {
        frames.push(frame(i, CENTRE_X + direction * 50.0, y, LEFT));
        frames.push(frame(
            i + vibrate_time,
            CENTRE_X - direction * 50.0,
            y,
            LEFT,
        ));
        direction *= -1.0;
        i += vibrate_time;
    }
    frames
}

#[test]
fn vibrating_off_centre_does_not_spin() {
    let j = play_from_start(&input_map(), lazer(), &vibrate(CENTRE_Y - 50.0, 50.0));
    assert_eq!(ticks_hit(&j, 0), 0);
    assert!(!result(&j, 0).result.is_hit());
}

#[test]
fn vibrating_on_centre_does_not_spin() {
    let j = play_from_start(&input_map(), lazer(), &vibrate(CENTRE_Y, 50.0));
    assert_eq!(ticks_hit(&j, 0), 0);
    assert!(!result(&j, 0).result.is_hit());
}

#[test]
fn vibrating_on_centre_with_double_time_does_not_spin() {
    // At twice the speed, the same real-time vibration takes twice the gameplay time.
    //
    // Lazer marks this case flaky. Exactly on the axis, each crossing of the centre turns the
    // angle by exactly ±180°, which double speed makes a whole spin. In lazer the round trip
    // through screen space leaves the cursor a hair off the axis, so a crossing turns a hair
    // less than 180° and never completes a spin; the cursor is put a hair off the axis here.
    let j = play_from_start(&input_map(), at_rate(2.0), &vibrate(CENTRE_Y + 0.01, 100.0));
    assert!(!result(&j, 0).result.is_hit());
}

#[test]
fn spin_single_direction() {
    for (amount, expected_ticks) in [
        (180.0, 0),
        (-180.0, 0),
        (360.0, 1),
        (-360.0, 1),
        (540.0, 1),
        (-540.0, 1),
        (720.0, 2),
        (-720.0, 2),
    ] {
        let frames = SpinFrames::new(INPUT_START).spin(amount, 500.0).build();
        let j = play_from_start(&input_map(), lazer(), &frames);
        assert_eq!(ticks_hit(&j, 0), expected_ticks, "{amount}°");
        assert!(!result(&j, 0).result.is_hit(), "{amount}°");
    }
}

#[test]
fn spin_half_both_directions() {
    // Half a spin one way, then a full spin back: half a spin the other way.
    let frames = SpinFrames::new(INPUT_START)
        .spin(180.0, 500.0)
        .spin(-360.0, 500.0)
        .build();
    let j = play_from_start(&input_map(), lazer(), &frames);
    assert_eq!(ticks_hit(&j, 0), 0);
    assert!(!result(&j, 0).result.is_hit());
}

#[test]
fn spin_one_direction_then_change_direction() {
    for (direction1, direction2, expected_ticks) in [
        (180.0, -540.0, 1),
        (-180.0, 540.0, 1),
        (180.0, -900.0, 2),
        (-180.0, 900.0, 2),
    ] {
        let frames = SpinFrames::new(INPUT_START)
            .spin(direction1, 500.0)
            .spin(direction2, 500.0)
            .build();
        let j = play_from_start(&input_map(), lazer(), &frames);
        assert_eq!(
            ticks_hit(&j, 0),
            expected_ticks,
            "{direction1}°, {direction2}°"
        );
        assert!(!result(&j, 0).result.is_hit());
    }
}

// ---- Tracking ----

#[test]
fn rotation_scales_with_the_rate() {
    // 382.5° of cursor rotation: one spin at the normal rate, two at double speed.
    let frames = SpinFrames::new(INPUT_START).spin(360.0, 500.0).build();
    assert_eq!(
        ticks_hit(&play_from_start(&input_map(), lazer(), &frames), 0),
        1
    );
    assert_eq!(
        ticks_hit(&play_from_start(&input_map(), at_rate(2.0), &frames), 0),
        2
    );
    assert_eq!(
        ticks_hit(&play_from_start(&input_map(), at_rate(0.5), &frames), 0),
        0
    );
}

#[test]
fn spinning_needs_a_button() {
    let frames: Vec<InputFrame> = SpinFrames::new(INPUT_START)
        .spin(1080.0, 500.0)
        .build()
        .into_iter()
        .map(|f| InputFrame { actions: NONE, ..f })
        .collect();
    let j = play_from_start(&input_map(), lazer(), &frames);
    assert_eq!(ticks_hit(&j, 0), 0);
}

#[test]
fn spinning_before_the_start_does_not_count() {
    // Two and a half spins, of which the first two are before the spinner starts.
    let frames = SpinFrames::new(INPUT_START - 400.0)
        .spin(900.0, 500.0)
        .build();
    let j = play_from_start(&input_map(), lazer(), &frames);
    assert_eq!(ticks_hit(&j, 0), 0);
}

// ---- Hit policies ----

#[test]
fn spinners_do_not_block_later_hits() {
    // A circle in the middle of a spinner can be hit while the spinner is unjudged.
    let map = beatmap(5.0, "256,192,1000,12,0,3000\n100,100,2000,1,0\n");
    for policy in [
        HitPolicy::StartTimeOrdered,
        HitPolicy::Legacy {
            hittable_range: 400.0,
        },
    ] {
        let frames = [
            frame(1990.0, 100.0, 100.0, NONE),
            frame(2000.0, 100.0, 100.0, LEFT),
            frame(2050.0, 100.0, 100.0, NONE),
        ];
        let j = play_from_start(&map, RulesOptions::with_policy(policy), &frames);
        let circle = result(&j, 1);
        assert_eq!((circle.result, circle.time), (Great, 2000.0), "{policy:?}");
        assert_eq!(result(&j, 0).result, Miss);
    }
}

#[test]
fn a_still_cursor_reaches_the_spinner_only_once_it_moves() {
    // The rotation tracker only receives mouse moves: a cursor still since before the spinner
    // gives it no position, so the first move of the spin turns nothing. 360.3° over 500 ms,
    // 0.72° per millisecond step: without the first step's move, 359.58° is short of a spin.
    let frames = SpinFrames::new(INPUT_START).spin(337.8, 500.0).build();
    let j = play_from_start(&input_map(), lazer(), &frames);
    assert_eq!(ticks_hit(&j, 0), 0);

    // A cursor already moving when the spin starts turns it by the whole first move.
    let mut moving = vec![frame(INPUT_START - 10.0, 256.0, 141.0, NONE)];
    moving.extend(frames);
    let j = play_from_start(&input_map(), lazer(), &moving);
    assert_eq!(ticks_hit(&j, 0), 1);
}

#[test]
fn spinning_stops_at_the_end_time() {
    // 451.7° over 500 ms, 0.9034° per millisecond step, 400 ms of which fall within the
    // spinner. The first step's move reaches the tracker as its first position and the update
    // at the end time no longer spins, which leaves 398 steps: 359.55°, short of the one spin
    // required. The move at the end time would make it 360.46° and the spinner a Great (though
    // its tick would still be missed, as the remaining ticks are missed before whole spins
    // turn into hits).
    let map = spinner_map(INPUT_START, INPUT_START + 400.0);
    assert_eq!(spinner(&map, 0).spins_required, 1);
    let frames = SpinFrames::new(INPUT_START).spin(429.2, 500.0).build();
    let j = play_from_start(&map, lazer(), &frames);
    assert_eq!(ticks_hit(&j, 0), 0);
    let spinner = result(&j, 0);
    assert_eq!((spinner.result, spinner.time), (Ok, INPUT_START + 400.0));
}

#[test]
fn spinners_without_duration_are_hit() {
    // No duration (the decoder clamps negative ones to zero): no spin required, and the two
    // gap ticks are missed.
    let map = beatmap(5.0, "256,192,1000,12,0,1000\n");
    let s = spinner(&map, 0);
    assert_eq!((s.spins_required, s.tick_count()), (0, 2));
    let j = play(&map, lazer(), &[]);
    assert_eq!(result(&j, 0).result, Great);
    assert_eq!(ticks(&j, 0), [IgnoreMiss, IgnoreMiss]);

    // A negative duration truncates towards zero.
    let mut s = Spinner::new(1000.0, -100.0);
    s.apply_defaults(&map_difficulty(5.0));
    assert_eq!((s.spins_required, s.maximum_bonus_spins), (0, 0));
}

#[test]
fn rate_mods_set_the_rate() {
    let options = |acronym| {
        RulesOptions::for_mods(&ModSet::new(vec![Mod::from_acronym(acronym)]).unwrap()).rate
    };
    assert_eq!(options("DT"), 1.5);
    assert_eq!(options("NC"), 1.5);
    assert_eq!(options("HT"), 0.75);
    assert_eq!(RulesOptions::for_mods(&ModSet::default()).rate, 1.0);

    // Nightcore's frequency stays at its default; the tempo makes up the rest, and their
    // product is the rate lazer's clock reports.
    let mut nightcore = Mod::from_acronym("NC");
    assert!(nightcore.set_setting("speed_change", &SettingValue::Float(1.51)));
    let set = ModSet::new(vec![nightcore]).unwrap();
    assert_eq!(set.true_gameplay_rate(), 1.0 * 1.5 * (1.51 / 1.5));
}

// ---- Sample state ----

/// The state of spinner 0 after every step of a play from the start, while it is alive.
fn live_states(map: &Beatmap, frames: &[InputFrame]) -> Vec<(f64, LiveSpinner)> {
    let mut all = vec![frame(0.0, frames[0].position.x, frames[0].position.y, NONE)];
    all.extend_from_slice(frames);
    let rules = OsuRules::with_options(map, lazer());
    let mut sim = Simulator::new(map, ReplayInput::new(all), rules);
    let mut states = Vec::new();
    while sim.step() {
        if let Some(live) = sim.rules().live_spinners().next() {
            states.push((sim.time(), live));
        }
    }
    states
}

#[test]
fn spinning_follows_the_drawn_rotation() {
    // Three spins in a second, then the cursor rests until the end time.
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let frames = SpinFrames::new(JUDGEMENT_START)
        .spin(1080.0, 1000.0)
        .build();
    let states = live_states(&map, &frames);
    let spinning = |from: f64, to: f64| {
        states
            .iter()
            .filter(|(t, _)| (from..to).contains(t))
            .map(|(_, s)| s.is_spinning)
            .collect::<Vec<_>>()
    };
    // Alive before its start time, but not spinnable.
    assert!(states[0].0 < JUDGEMENT_START);
    assert!(
        spinning(f64::NEG_INFINITY, JUDGEMENT_START)
            .iter()
            .all(|&s| !s)
    );
    // At about 1°/ms, the drawn rotation settles about 100° behind (it closes 1% of the gap
    // per millisecond): spinning soon after the start, throughout.
    let first = states.iter().find(|(_, s)| s.is_spinning).unwrap().0;
    assert!(first < JUDGEMENT_START + 30.0, "spinning from {first}");
    assert!(spinning(first, JUDGEMENT_START + 1000.0).iter().all(|&s| s));
    // Once the cursor rests, the gap shrinks by 1% per millisecond: below 10° within about
    // 230 ms.
    let last = states.iter().rfind(|(_, s)| s.is_spinning).unwrap().0;
    assert!(
        (JUDGEMENT_START + 1150.0..JUDGEMENT_START + 1300.0).contains(&last),
        "spinning until {last}"
    );
    assert!(spinning(last + 1.0, f64::INFINITY).iter().all(|&s| !s));
    // The simulation ends with the spinner's judgement at its end time.
    let (end, live) = *states.last().unwrap();
    assert_eq!((end, live.is_spinning), (JUDGEMENT_END, false));
}

#[test]
fn slow_spinning_is_not_spinning() {
    // 30°/s: the drawn rotation keeps within 3° of the rotation added. The rotation still
    // counts towards the progress.
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let frames = SpinFrames::new(JUDGEMENT_START).spin(60.0, 2000.0).build();
    let states = live_states(&map, &frames);
    assert!(states.iter().all(|(_, s)| !s.is_spinning));
    let progress = states.last().unwrap().1.progress_unclamped;
    assert!(progress > 0.0, "{progress}");
}

#[test]
fn spins_past_the_last_tick_are_counted_for_the_bonus_max_sample() {
    // Twenty spins: the 12 ticks (7 plain, 5 bonus) take the first twelve, the remaining
    // eight each play the bonus-max sample, one per whole spin as it completes.
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let states = live_states(&map, &judgement_replay(20));
    let mut counts: Vec<u32> = states.iter().map(|(_, s)| s.bonus_max_spins).collect();
    assert!(counts.windows(2).all(|w| w[1] == w[0] || w[1] == w[0] + 1));
    counts.dedup();
    assert_eq!(counts, (0..=8).collect::<Vec<_>>());
    // Spun past the five spins required: 20 spins over five, short of the generator's last
    // 22.5°, which the end time cuts off.
    let progress = states.last().unwrap().1.progress_unclamped;
    assert!(
        (4.0..(20.0 * 360.0 + 22.5) / 360.0 / 5.0).contains(&progress),
        "{progress}"
    );

    // Without ticks (a spinner too short for any), no spin plays it.
    let map = beatmap(5.0, "256,192,1000,12,0,1000\n");
    let frames = SpinFrames::new(900.0).spin(720.0, 100.0).build();
    assert!(
        live_states(&map, &frames)
            .iter()
            .all(|(_, s)| s.bonus_max_spins == 0)
    );
}

// ---- Drawing state ----

#[test]
fn drawing_state_follows_the_judgements() {
    // Twenty spins: five required, so the spinner is complete with the fifth tick; the twelfth
    // and last tick is a bonus tick.
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let frames = judgement_replay(20);
    let mut all = vec![frame(0.0, frames[0].position.x, frames[0].position.y, NONE)];
    all.extend_from_slice(&frames);
    let rules = OsuRules::with_options(&map, lazer());
    let mut sim = Simulator::new(&map, ReplayInput::new(all), rules);
    let mut last = None;
    while sim.step() {
        if let Some(live) = sim.rules().live_spinners().next() {
            assert_eq!(live.progress, live.progress_unclamped.clamp(0.0, 1.0));
            last = Some(live);
        }
    }
    let live = last.unwrap();
    let hits: Vec<f64> = sim
        .log()
        .events()
        .iter()
        .filter(|j| j.object.nested.is_some() && j.result.is_hit())
        .map(|j| j.time)
        .collect();
    assert_eq!(hits.len(), 12);
    assert_eq!(sim.rules().spinner_tick_hits(0), hits.as_slice());
    assert_eq!(live.last_bonus_hit, Some(hits[11]));
    assert_eq!(live.time_completed, Some(hits[4]));
    let judgement = live.judgement.unwrap();
    assert_eq!((judgement.time, judgement.result), (JUDGEMENT_END, Great));
    assert_eq!(live.progress, 1.0);
    // The drawn rotation follows the spins, short of them while damped.
    assert!(
        (0.0..20.0 * 360.0 + 22.5).contains(&live.rotation),
        "{}",
        live.rotation
    );
    assert!(live.rotation > 19.0 * 360.0, "{}", live.rotation);
}

#[test]
fn an_incomplete_spinner_has_no_completion_time() {
    let map = spinner_map(JUDGEMENT_START, JUDGEMENT_END);
    let states = live_states(&map, &judgement_replay(3));
    let (_, live) = *states.last().unwrap();
    assert_eq!(live.time_completed, None);
    assert_eq!(live.last_bonus_hit, None);
    assert!(
        live.progress < 1.0 && live.progress > 0.5,
        "{}",
        live.progress
    );
    assert_eq!(live.judgement.unwrap().result, Miss);
}

#[test]
fn tick_hits_are_empty_for_other_objects() {
    let map = beatmap(5.0, "100,100,500,1,0\n256,192,2000,12,0,4000\n");
    let rules = OsuRules::with_options(&map, lazer());
    assert!(rules.spinner_tick_hits(0).is_empty());
    assert!(rules.spinner_tick_hits(1).is_empty());
    assert!(rules.spinner_tick_hits(7).is_empty());
}
