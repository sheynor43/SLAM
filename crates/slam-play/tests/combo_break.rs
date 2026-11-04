//! The combo break sound (lazer's `ComboEffects`).

mod common;

use std::sync::Arc;

use common::skin::hitsounds;
use slam_audio::{PlayError, Sample, VoiceGroup, VoiceId, VoiceParams};
use slam_osu::{Beatmap, ModSet};
use slam_play::gameplay_sounds::{GameplaySounds, SoundSettings, VoiceSink};

const MAP: &str = "osu file format v14

[Difficulty]
OverallDifficulty:5

[TimingPoints]
0,500,4,1,0,100,1,0

[HitObjects]
64,192,1000,1,0
";

/// Counts the combo break sounds; nothing else plays here.
#[derive(Default)]
struct Sink {
    plays: u64,
    full: bool,
}

impl VoiceSink for Sink {
    fn play_voice(&mut self, _: &Arc<Sample>, params: VoiceParams) -> Result<VoiceId, PlayError> {
        if self.full {
            return Err(PlayError::QueueFull);
        }
        // Under the effects volume, centred, as it is (ADR-0046).
        assert_eq!(params.group, VoiceGroup::Effects);
        assert_eq!((params.gain, params.balance, params.rate), (1.0, 0.0, 1.0));
        assert!(!params.looping);
        self.plays += 1;
        Ok(VoiceId::from_play(self.plays))
    }

    fn stop_voice(&mut self, _: VoiceId) -> Result<(), PlayError> {
        Ok(())
    }

    fn set_voice_balance(&mut self, _: VoiceId, _: f32) -> Result<(), PlayError> {
        unreachable!()
    }

    fn set_voice_gain(&mut self, _: VoiceId, _: f32, _: f64) -> Result<(), PlayError> {
        unreachable!()
    }

    fn set_voice_rate(&mut self, _: VoiceId, _: f64) -> Result<(), PlayError> {
        unreachable!()
    }
}

fn sounds(always_play_first: bool, sample: bool) -> GameplaySounds {
    let map = Beatmap::decode(MAP.as_bytes()).unwrap();
    let (hitsounds, _) = hitsounds(&map, &[]);
    let settings = SoundSettings {
        always_play_first_combo_break: always_play_first,
        ..SoundSettings::for_mods(&ModSet::default())
    };
    GameplaySounds::new(&map, hitsounds, settings).with_combo_break_sound(
        sample.then(|| Arc::new(Sample::from_interleaved(vec![0.1; 20], 48_000, 2).unwrap())),
    )
}

#[test]
fn lazer_plays_the_first_break_by_default() {
    assert!(SoundSettings::for_mods(&ModSet::default()).always_play_first_combo_break);
}

#[test]
fn the_first_break_sounds_then_only_breaks_from_above_twenty() {
    let mut s = sounds(true, true);
    let mut sink = Sink::default();
    // No change: a miss at no combo.
    s.combo_changed(0, 0, 100.0, &mut sink);
    assert_eq!(sink.plays, 0);
    s.combo_changed(0, 1, 200.0, &mut sink);
    s.combo_changed(1, 0, 300.0, &mut sink);
    assert_eq!(sink.plays, 1, "the first break");
    s.combo_changed(20, 0, 400.0, &mut sink);
    assert_eq!(sink.plays, 1, "20 is not above 20");
    s.combo_changed(21, 0, 500.0, &mut sink);
    assert_eq!(sink.plays, 2);
    // A growing combo is silent.
    s.combo_changed(21, 22, 600.0, &mut sink);
    assert_eq!(sink.plays, 2);
}

#[test]
fn without_always_play_first_only_breaks_from_above_twenty_sound() {
    let mut s = sounds(false, true);
    let mut sink = Sink::default();
    s.combo_changed(5, 0, 100.0, &mut sink);
    assert_eq!(sink.plays, 0);
    s.combo_changed(21, 0, 200.0, &mut sink);
    assert_eq!(sink.plays, 1);
}

#[test]
fn a_restart_makes_the_next_break_the_first_again() {
    let mut s = sounds(true, true);
    let mut sink = Sink::default();
    s.combo_changed(3, 0, 100.0, &mut sink);
    s.combo_changed(3, 0, 200.0, &mut sink);
    assert_eq!(sink.plays, 1);
    assert!(s.reset(&mut sink));
    s.combo_changed(3, 0, 50.0, &mut sink);
    assert_eq!(sink.plays, 2);
}

#[test]
fn a_skin_without_the_sample_is_silent_and_a_full_queue_drops_it() {
    let mut s = sounds(true, false);
    let mut sink = Sink::default();
    s.combo_changed(30, 0, 100.0, &mut sink);
    assert_eq!(sink.plays, 0);
    assert_eq!(s.failed_commands(), 0);

    let mut s = sounds(true, true);
    let mut sink = Sink {
        full: true,
        ..Sink::default()
    };
    s.combo_changed(30, 0, 100.0, &mut sink);
    assert_eq!(s.failed_commands(), 1);
    // The break counted as the first even though it was dropped.
    sink.full = false;
    s.combo_changed(3, 0, 200.0, &mut sink);
    assert_eq!(sink.plays, 0);
}
