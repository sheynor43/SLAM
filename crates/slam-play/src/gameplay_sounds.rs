//! Gameplay sounds: hit samples queued on the mixer in the update tick that judges them, and
//! the looping samples of tracked sliders.
//!
//! Follows lazer's drawables:
//! - a judgement plays its object's samples only on the transition to a hit
//!   (`DrawableHitObject.UpdateState`), so a miss is silent;
//! - a slider judged a hit plays its tail samples, and only if its tail was hit too unless
//!   Classic's `AlwaysPlayTailSample` is on (`DrawableSlider.PlaySamples`);
//! - a slider plays `sliderslide`/`sliderwhistle` while it is tracked from its start time on,
//!   panned with its ball, and stops them when tracking is lost or the slider expires
//!   (`DrawableSlider.Update`, `OnKilled`).
//!
//! Gameplay rate mods do not change hit samples: lazer applies `IApplicableToSample` to
//! storyboard samples only.
//!
//! Samples are panned by the horizontal position of the drawable that plays them, as a
//! fraction of the playfield width (`CalculateSamplePlaybackBalance`). Positions are taken in
//! playfield space rather than through lazer's screen space round trip, which can move a
//! balance by 0.01 where it rounds at a midpoint.
//!
//! The spinner's spinning and bonus-max sounds are not played yet; its bonus ticks are,
//! through their judgements.

use std::sync::Arc;

use slam_audio::{MixerHandle, PlayError, Sample, VoiceId, VoiceParams};
use slam_osu::judgement::{Judgement, JudgementLog};
use slam_osu::objects::{OsuHitObject, OsuHitObjectKind, Slider, curve_position_at};
use slam_osu::rules::OsuRules;
use slam_osu::{Beatmap, Mod, ModSet, SliderPath, Vec2};

use crate::hitsounds::{Hitsounds, Sound};

/// Width of the playfield in osu!pixels.
const PLAYFIELD_WIDTH: f32 = 512.0;

/// Lazer's default `PositionalHitsoundsLevel`.
pub const DEFAULT_POSITIONAL_LEVEL: f32 = 0.2;

/// Most looping samples of one slider: `sliderslide` and `sliderwhistle`.
const MAX_SLIDING_SAMPLES: usize = 2;

/// Where gameplay sounds go: the mixer, or a recorder in tests.
pub trait VoiceSink {
    /// Starts a voice (see [`MixerHandle::play_voice`]).
    fn play_voice(
        &mut self,
        sample: &Arc<Sample>,
        params: VoiceParams,
    ) -> Result<VoiceId, PlayError>;
    /// Stops a voice (see [`MixerHandle::stop_voice`]).
    fn stop_voice(&mut self, voice: VoiceId) -> Result<(), PlayError>;
    /// Sets the balance of a voice (see [`MixerHandle::set_voice_balance`]).
    fn set_voice_balance(&mut self, voice: VoiceId, balance: f32) -> Result<(), PlayError>;
}

impl VoiceSink for MixerHandle {
    fn play_voice(
        &mut self,
        sample: &Arc<Sample>,
        params: VoiceParams,
    ) -> Result<VoiceId, PlayError> {
        MixerHandle::play_voice(self, sample, params)
    }

    fn stop_voice(&mut self, voice: VoiceId) -> Result<(), PlayError> {
        MixerHandle::stop_voice(self, voice)
    }

    fn set_voice_balance(&mut self, voice: VoiceId, balance: f32) -> Result<(), PlayError> {
        MixerHandle::set_voice_balance(self, voice, balance)
    }
}

/// Settings of gameplay sounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundSettings {
    /// Lazer's `PositionalHitsoundsLevel`, 0..=1: how far samples are panned by position.
    pub positional_level: f32,
    /// Classic's `AlwaysPlayTailSample`: a hit slider plays its tail samples even if its tail
    /// was missed.
    pub always_play_tail_sample: bool,
}

impl SoundSettings {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Mods/OsuModClassic.cs (ApplyToDrawableHitObject)
    /// The settings for a play with `mods`, at lazer's default positional level.
    pub fn for_mods(mods: &ModSet) -> SoundSettings {
        SoundSettings {
            positional_level: DEFAULT_POSITIONAL_LEVEL,
            always_play_tail_sample: mods.mods().iter().any(|m| match m {
                Mod::Classic(c) => c.always_play_tail_sample,
                _ => false,
            }),
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (CalculateSamplePlaybackBalance)
/// The balance of a sample played at `position` (0 at the left edge of the playfield, 1 at the
/// right) with positional level `level`, rounded to hundredths.
pub fn sample_balance(level: f32, position: f32) -> f32 {
    let balance_adjust_amount = level * 2.0;
    let returned_value = f64::from(balance_adjust_amount) * (f64::from(position) - 0.5);
    // `Math.Round(value, 2)`: scaled, rounded half to even, scaled back.
    ((returned_value * 100.0).round_ties_even() / 100.0) as f32
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableOsuHitObject.cs (CalculateDrawableRelativePosition)
/// The playback position of a drawable centred at `x` in the playfield.
fn playback_position(x: f32) -> f32 {
    x / PLAYFIELD_WIDTH
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (UpdateAfterChildren)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/SnakingSliderBody.cs (Refresh)
/// The x centre of a slider's drawable: its size is the bounding box of the whole path
/// (widened evenly by the path radius) and its origin the first vertex. The framework's
/// bounds also take in the path origin, which the first vertex of a decoded slider path
/// always is, so they are those of the vertices.
fn slider_centre_x(object: &OsuHitObject, slider: &Slider) -> f32 {
    let path = slider.path.calculated_path();
    let Some(first) = path.first() else {
        return object.stacked_position().x;
    };
    let (min, max) = path
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), v| {
            (min.min(v.x), max.max(v.x))
        });
    object.stacked_position().x - first.x + (min + max) / 2.0
}

/// A slider's ball, for panning its sliding samples.
#[derive(Debug, Clone)]
struct Ball {
    start_time: f64,
    duration: f64,
    span_count: i32,
    /// Stacked position of the slider.
    position: Vec2,
    path: SliderPath,
}

impl Ball {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (UpdateAfterChildren)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSliderBall.cs (UpdateProgress)
    /// The ball's x position in the playfield after an update at `time`.
    fn x_at(&self, time: f64) -> f32 {
        let completion_progress = ((time - self.start_time) / self.duration).clamp(0.0, 1.0);
        self.position.x + curve_position_at(&self.path, self.span_count, completion_progress).x
    }
}

/// Where an object's samples play from.
#[derive(Debug, Clone, Copy)]
struct ObjectInfo {
    /// Playback position of the object's own samples.
    hit: f32,
    /// Range of `GameplaySounds::nested`: playback positions of a slider's nested objects.
    nested_start: u32,
    nested_len: u32,
    /// Index into `GameplaySounds::balls` of a slider.
    ball: Option<u32>,
}

/// The sliding samples of a slider that plays them.
#[derive(Debug, Clone, Copy)]
struct Sliding {
    index: u32,
    voices: [VoiceId; MAX_SLIDING_SAMPLES],
    /// Bit `k` set: starting `voices[k]` failed; it is tried again in the next update.
    failed_starts: u8,
    balance: f32,
    /// Whether the slider was found alive in the current update.
    alive: bool,
}

/// The sounds of a play: call [`GameplaySounds::update`] once per update tick, after the
/// rules, and [`GameplaySounds::stop_all`] when gameplay pauses or ends.
///
/// Not `Clone`: it owns the looping voices it started.
#[derive(Debug)]
pub struct GameplaySounds {
    hitsounds: Hitsounds,
    settings: SoundSettings,
    objects: Vec<ObjectInfo>,
    nested: Vec<f32>,
    balls: Vec<Ball>,
    /// Sliders playing their sliding samples; never more than the beatmap's sliders.
    sliding: Vec<Sliding>,
    /// Looping voices whose stop failed, tried again every update and in `stop_all`; a
    /// looping voice left playing would never end. Holds two per slider of the beatmap, enough
    /// unless tracking keeps being lost and regained while the queue stays full; voices beyond
    /// that are left playing (counted in `failed`).
    pending_stops: Vec<VoiceId>,
    /// Judgements of the log already played.
    seen: usize,
    /// Time of the previous update: lazer's slider reads its ball as placed after the
    /// previous frame.
    previous_time: f64,
    /// Mixer commands that failed (queue full or a sample in the wrong format).
    failed: u64,
}

impl GameplaySounds {
    /// The sounds of `beatmap` (with its mods applied) from its resolved `hitsounds`. Call it
    /// off the hot paths.
    pub fn new(beatmap: &Beatmap, hitsounds: Hitsounds, settings: SoundSettings) -> Self {
        let mut objects = Vec::with_capacity(beatmap.hit_objects.len());
        let mut nested = Vec::new();
        let mut balls = Vec::new();
        for object in &beatmap.hit_objects {
            let info = match &object.kind {
                OsuHitObjectKind::Slider(slider) => {
                    let nested_start = nested.len();
                    nested.extend(
                        slider
                            .nested
                            .iter()
                            .map(|n| playback_position(n.stacked_position().x)),
                    );
                    balls.push(Ball {
                        start_time: object.start_time,
                        duration: slider.duration(object.start_time),
                        span_count: slider.span_count(),
                        position: object.stacked_position(),
                        path: slider.path.clone(),
                    });
                    // Object and sample counts of a beatmap fit in `u32`, as
                    // `ObjectRef` assumes.
                    ObjectInfo {
                        hit: playback_position(slider_centre_x(object, slider)),
                        nested_start: nested_start as u32,
                        nested_len: (nested.len() - nested_start) as u32,
                        ball: Some((balls.len() - 1) as u32),
                    }
                }
                // A spinner and its ticks are centred on its position.
                OsuHitObjectKind::Circle | OsuHitObjectKind::Spinner(_) => ObjectInfo {
                    hit: playback_position(object.stacked_position().x),
                    nested_start: 0,
                    nested_len: 0,
                    ball: None,
                },
            };
            objects.push(info);
        }
        GameplaySounds {
            hitsounds,
            settings,
            objects,
            nested,
            sliding: Vec::with_capacity(balls.len()),
            pending_stops: Vec::with_capacity(balls.len() * MAX_SLIDING_SAMPLES),
            balls,
            seen: 0,
            previous_time: f64::NEG_INFINITY,
            failed: 0,
        }
    }

    /// The settings in use.
    pub fn settings(&self) -> SoundSettings {
        self.settings
    }

    /// Mixer commands that failed so far. Hit samples and balance changes are dropped;
    /// starting and stopping looping samples is tried again in later updates.
    pub fn failed_commands(&self) -> u64 {
        self.failed
    }

    /// Plays the sounds of an update at `time`: the judgements `rules` added to `log` since the
    /// previous update, then the sliding samples of the sliders `rules` has alive.
    pub fn update(
        &mut self,
        time: f64,
        log: &JudgementLog,
        rules: &OsuRules,
        sink: &mut impl VoiceSink,
    ) {
        self.retry_stops(sink);
        for judgement in log.events_since(self.seen) {
            self.play_judgement(judgement, rules, sink);
        }
        self.seen = log.events().len();
        self.update_sliding(time, rules, sink);
        self.previous_time = time;
    }

    /// Stops every looping sample, as when gameplay pauses or ends; the next update starts
    /// the sliding samples of tracked sliders again. Returns whether every stop went through;
    /// if not, call it again (or keep updating) until it does.
    pub fn stop_all(&mut self, sink: &mut impl VoiceSink) -> bool {
        self.retry_stops(sink);
        for k in 0..self.sliding.len() {
            let sliding = self.sliding[k];
            self.stop(&sliding, sink);
        }
        self.sliding.clear();
        self.pending_stops.is_empty()
    }

    /// Tries the stops that failed before again.
    fn retry_stops(&mut self, sink: &mut impl VoiceSink) {
        self.pending_stops
            .retain(|&voice| sink.stop_voice(voice).is_err());
    }

    /// Stops the voices of `sliding`; a failed stop is kept to be tried again.
    fn stop(&mut self, sliding: &Sliding, sink: &mut impl VoiceSink) {
        for &voice in &sliding.voices {
            if voice != VoiceId::NONE && sink.stop_voice(voice).is_err() {
                self.failed += 1;
                if self.pending_stops.len() < self.pending_stops.capacity() {
                    self.pending_stops.push(voice);
                }
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (UpdateState, PlaySamples)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (PlaySamples)
    fn play_judgement(
        &mut self,
        judgement: &Judgement,
        rules: &OsuRules,
        sink: &mut impl VoiceSink,
    ) {
        let sounds = self.hitsounds.for_judgement(judgement);
        if sounds.is_empty() {
            return;
        }
        let object = judgement.object;
        let Some(info) = self.objects.get(object.index as usize) else {
            return;
        };
        let position = match object.nested {
            Some(k) if k < info.nested_len => self.nested[(info.nested_start + k) as usize],
            // A spinner's ticks.
            Some(_) => info.hit,
            None => {
                if info.ball.is_some()
                    && !self.settings.always_play_tail_sample
                    && !rules.slider_tail_hit(object.index)
                {
                    return;
                }
                info.hit
            }
        };
        let balance = sample_balance(self.settings.positional_level, position);
        for sound in sounds {
            let params = VoiceParams {
                gain: sound.gain,
                balance,
                ..VoiceParams::default()
            };
            if sink.play_voice(&sound.sample, params).is_err() {
                self.failed += 1;
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Drawables/DrawableSlider.cs (Update, StopAllSamples)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Drawables/DrawableHitObject.cs (OnKilled)
    fn update_sliding(&mut self, time: f64, rules: &OsuRules, sink: &mut impl VoiceSink) {
        for sliding in &mut self.sliding {
            sliding.alive = false;
        }
        for live in rules.live_sliders() {
            let Some(ball) = self
                .objects
                .get(live.index as usize)
                .and_then(|info| info.ball)
                .map(|b| &self.balls[b as usize])
            else {
                continue;
            };
            let playing = self.sliding.iter().position(|s| s.index == live.index);
            if live.tracking && time >= ball.start_time {
                let balance = sample_balance(
                    self.settings.positional_level,
                    playback_position(ball.x_at(self.previous_time)),
                );
                let sounds = self.hitsounds.get(live.index, Sound::Sliding);
                debug_assert!(sounds.len() <= MAX_SLIDING_SAMPLES);
                let p = match playing {
                    Some(p) => p,
                    None => {
                        self.sliding.push(Sliding {
                            index: live.index,
                            voices: [VoiceId::NONE; MAX_SLIDING_SAMPLES],
                            failed_starts: (1 << sounds.len().min(MAX_SLIDING_SAMPLES)) - 1,
                            balance,
                            alive: true,
                        });
                        self.sliding.len() - 1
                    }
                };
                let sliding = &mut self.sliding[p];
                sliding.alive = true;
                // Lazer's bindable compares with `double.Equals`: NaN (a slider without
                // duration) equals NaN, and −0 equals 0.
                if !(sliding.balance == balance || sliding.balance.is_nan() && balance.is_nan()) {
                    sliding.balance = balance;
                    for &voice in &sliding.voices {
                        if voice != VoiceId::NONE && sink.set_voice_balance(voice, balance).is_err()
                        {
                            self.failed += 1;
                        }
                    }
                }
                for (k, sound) in sounds.iter().enumerate() {
                    if sliding.failed_starts & (1 << k) == 0 {
                        continue;
                    }
                    let params = VoiceParams {
                        gain: sound.gain,
                        balance,
                        looping: true,
                        ..VoiceParams::default()
                    };
                    match sink.play_voice(&sound.sample, params) {
                        Ok(voice) => {
                            sliding.voices[k] = voice;
                            sliding.failed_starts &= !(1 << k);
                        }
                        Err(_) => self.failed += 1,
                    }
                }
            } else if let Some(p) = playing {
                let sliding = self.sliding.swap_remove(p);
                self.stop(&sliding, sink);
            }
        }
        // Sliders that expired since the previous update.
        let mut k = 0;
        while k < self.sliding.len() {
            if self.sliding[k].alive {
                k += 1;
            } else {
                let sliding = self.sliding.swap_remove(k);
                self.stop(&sliding, sink);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_is_centred_and_scaled_by_the_level() {
        assert_eq!(sample_balance(0.2, 0.5), 0.0);
        assert_eq!(sample_balance(0.2, 0.0), -0.2);
        assert_eq!(sample_balance(0.2, 1.0), 0.2);
        assert_eq!(sample_balance(1.0, 1.0), 1.0);
        assert_eq!(sample_balance(0.0, 1.0), 0.0);
        // 0.4 × (64 / 512 − 0.5) = −0.15.
        assert_eq!(sample_balance(0.2, playback_position(64.0)), -0.15);
    }

    #[test]
    fn balance_rounds_half_to_even() {
        // 2 × (0.5625 − 0.5) = 0.125 exactly: 12.5 hundredths round down to 0.12.
        assert_eq!(sample_balance(1.0, 0.5625), 0.12);
        // 2 × (0.6875 − 0.5) = 0.375 exactly: 37.5 hundredths round up to 0.38.
        assert_eq!(sample_balance(1.0, 0.6875), 0.38);
    }
}
