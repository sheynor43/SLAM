//! The fail animation (ADR-0054): lazer's `FailAnimationContainer`.
//!
//! The update side ([`crate::session`]) asks the track for [`TrackEffects`] and puts a
//! [`FailView`] in the snapshot; the draw side turns it into the objects' fall and the
//! game layer's shrink and grey.

use slam_audio::{
    FilterKind, MAX_LOW_PASS_CUTOFF, MIN_HIGH_PASS_CUTOFF, PlayError, TrackAdjustment, TrackControl,
};
use slam_osu::Vec2;
use slam_render::sprite::Affine;

use crate::circles::{to_linear, to_srgb};
use crate::playfield::PlayfieldLayout;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (duration)
/// Length of the fail animation, milliseconds: of real time for the track and the game
/// layer, of gameplay time for the objects' fall.
pub const FAIL_DURATION: f64 = 2_500.0;

/// [`FAIL_DURATION`] in nanoseconds.
pub const FAIL_DURATION_NS: u64 = 2_500_000_000;
const _: () = assert!(FAIL_DURATION_NS == FAIL_DURATION as u64 * 1_000_000);

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
/// Gameplay time the objects take to fade out, milliseconds.
const OBJECTS_FADE: f64 = FAIL_DURATION / 2.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (applyToPlayfield, dropOffScreen)
/// Largest turn of a falling object either way, degrees.
const DROP_ROTATION: f32 = 90.0;
/// Scale a falling object reaches.
const DROP_SCALE: f32 = 0.5;
/// How far a falling object goes down, osu!pixels.
const DROP_DISTANCE: f32 = 400.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
/// Scale and turn (degrees) the game layer reaches.
const LAYER_SCALE: f32 = 0.85;
const LAYER_ROTATION: f32 = 1.0;
/// osuTK's `Color4.Gray`, (128, 128, 128): the game layer's colour at the end.
const LAYER_GREY: f32 = 128.0 / 255.0;
/// `OsuColour.Gray(0.3f)`: the background's colour, reached in [`BACKGROUND_FADE`] ms.
const BACKGROUND_GREY: f32 = 0.3;
const BACKGROUND_FADE: f64 = 60.0;

const NS_PER_MS: f64 = 1_000_000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
/// Cutoff of both track filters at the end of the animation, Hz.
const FAIL_CUTOFF: f32 = 300.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (volumeAdjustment)
/// Track volume multiplier while failing.
const FAIL_VOLUME: f32 = 0.5;

/// What the fail animation does to the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrackEffects {
    /// The track as it is (a new player).
    Neutral,
    /// The animation runs: the frequency falls to 0, the volume halves, the filters
    /// close in.
    Failing,
    /// The animation is over (or cut short): the frequency is 0 and stays, the filters
    /// and the volume multiplier are gone.
    Failed,
}

impl TrackEffects {
    /// The track's adjustments with the mods' frequency `mods_frequency` (ADR-0055): the
    /// animation's frequency multiplies it, so the fall from 1 to 0 is one from it to 0.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start, removeFilters)
    pub(crate) fn adjustments(self, mods_frequency: f64) -> [TrackAdjustment; 4] {
        let (frequency, duration_ms, volume, low, high) = match self {
            TrackEffects::Neutral => (
                mods_frequency,
                0.0,
                1.0,
                MAX_LOW_PASS_CUTOFF,
                MIN_HIGH_PASS_CUTOFF,
            ),
            TrackEffects::Failing => (0.0, FAIL_DURATION, FAIL_VOLUME, FAIL_CUTOFF, FAIL_CUTOFF),
            // Lazer leaves the frequency at 0 so that the menu does not sweep it again.
            TrackEffects::Failed => (0.0, 0.0, 1.0, MAX_LOW_PASS_CUTOFF, MIN_HIGH_PASS_CUTOFF),
        };
        [
            TrackAdjustment::Frequency {
                frequency,
                duration_ms,
            },
            TrackAdjustment::Volume(volume),
            // The low-pass closes along OutCubic; the high-pass is at its cutoff at once.
            TrackAdjustment::Filter {
                kind: FilterKind::LowPass,
                cutoff_hz: low,
                duration_ms,
            },
            TrackAdjustment::Filter {
                kind: FilterKind::HighPass,
                cutoff_hz: high,
                duration_ms: 0.0,
            },
        ]
    }
}

/// The track as a new player without mods finds it.
pub fn neutral_track() -> [TrackAdjustment; 4] {
    TrackEffects::Neutral.adjustments(1.0)
}

/// Takes the play's effects and rate mods off the track: tempo 1 and the adjustments of a
/// play without mods, for the shell when a play goes (ADR-0055). On an error, call again
/// until the queue takes both.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/MusicController.cs (ResetTrackAdjustments)
pub fn reset_track(track: &mut impl TrackControl) -> Result<(), PlayError> {
    track.set_rate(1.0)?;
    track.adjust(&neutral_track())
}

/// When the play failed, for the draw side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FailView {
    /// Gameplay time of the step that failed, milliseconds.
    pub time: f64,
    /// Engine time of the tick that failed.
    pub at_ns: u64,
    /// Back cut the animation short: everything jumps to its end (lazer's
    /// `FinishTransforms`).
    pub cut: bool,
}

/// The fail animation in one frame, from [`FailView`]. The objects and their fall go by
/// gameplay time, the game layer and the background by real time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FailFrame {
    fail: FailView,
    /// Gameplay time drawn.
    time: f64,
    /// Alpha of every object (lazer's `HitObjectContainer` fading out).
    pub objects_alpha: f32,
    /// Where the game layer goes (the playfield, HUD and cursor): scaled and turned about
    /// the window's centre.
    pub layer: Affine,
    /// The game layer's colour, sRGB grey.
    pub layer_grey: f32,
    /// The background's colour, sRGB grey, on top of its dim.
    pub background_grey: f32,
}

impl FailFrame {
    /// The animation at gameplay `time` and engine time `now_ns` in a window of `width` ×
    /// `height` pixels; `None` before a fail.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
    pub fn new(
        fail: Option<FailView>,
        time: f64,
        now_ns: u64,
        width: f32,
        height: f32,
    ) -> Option<Self> {
        let fail = fail?;
        let real = now_ns.saturating_sub(fail.at_ns) as f64 / NS_PER_MS;
        let progress = |elapsed: f64, duration: f64| {
            if fail.cut {
                1.0
            } else {
                (elapsed / duration).clamp(0.0, 1.0)
            }
        };
        let layer = progress(real, FAIL_DURATION);
        let eased = out_quart(layer) as f32;
        let scale = 1.0 + (LAYER_SCALE - 1.0) * eased;
        let rotation = (LAYER_ROTATION * eased).to_radians();
        Some(Self {
            fail,
            time,
            objects_alpha: 1.0 - progress(time - fail.time, OBJECTS_FADE) as f32,
            layer: Affine::about([width / 2.0, height / 2.0], scale, rotation, [0.0, 0.0]),
            layer_grey: grey(1.0, LAYER_GREY, layer as f32),
            // Lazer starts it at the fail too (it is not one of the container's transforms).
            background_grey: grey(
                1.0,
                BACKGROUND_GREY,
                (real / BACKGROUND_FADE).clamp(0.0, 1.0) as f32,
            ),
        })
    }

    /// The layer's colour multiplier, straight alpha.
    pub fn layer_tint(&self) -> [f32; 4] {
        [self.layer_grey, self.layer_grey, self.layer_grey, 1.0]
    }

    /// Where the object with this index falls to, pivoting at its `position` (osu!pixels):
    /// from gameplay time `appear` or the fail, whichever is later (lazer applies the fall to
    /// objects as they become alive), turned by its own random angle.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (applyToPlayfield, dropOffScreen)
    pub fn drop(
        &self,
        index: u32,
        appear: f64,
        position: Vec2,
        layout: &PlayfieldLayout,
    ) -> Affine {
        let from = appear.max(self.fail.time);
        let t = if self.fail.cut {
            1.0
        } else {
            ((self.time - from) / FAIL_DURATION).clamp(0.0, 1.0) as f32
        };
        let pivot = layout.to_window(position);
        Affine::about(
            [pivot.x, pivot.y],
            1.0 + (DROP_SCALE - 1.0) * t,
            (drop_rotation(index) * t).to_radians(),
            [0.0, DROP_DISTANCE * layout.scale() * t],
        )
    }
}

/// The object's turn in −90..90 degrees: lazer's `RNG.NextSingle(-90, 90)`, here a hash of
/// the index so that every frame and every run agree.
fn drop_rotation(index: u32) -> f32 {
    // SplitMix64.
    let mut z = u64::from(index).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let unit = (z >> 40) as f32 / (1u64 << 24) as f32;
    -DROP_ROTATION + 2.0 * DROP_ROTATION * unit
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (OutQuart)
fn out_quart(time: f64) -> f64 {
    let time = time - 1.0;
    1.0 - time * time * time * time
}

// Ported from osu-framework 2026.921.1: osu.Framework/Utils/Interpolation.cs (ValueAt for Color4)
/// An sRGB grey from `from` to `to` at `t`, blended in linear light.
fn grey(from: f32, to: f32, t: f32) -> f32 {
    let (a, b) = (to_linear(from), to_linear(to));
    to_srgb(a + t * (b - a))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn view(cut: bool) -> FailView {
        FailView {
            time: 10_000.0,
            at_ns: 1_000 * MS,
            cut,
        }
    }

    /// Logs the tempo and the adjustments; refuses the first `refuse` calls.
    #[derive(Default)]
    struct Track {
        rates: Vec<f64>,
        adjustments: Vec<TrackAdjustment>,
        refuse: usize,
    }

    impl Track {
        fn refused(&mut self) -> bool {
            let refused = self.refuse > 0;
            self.refuse = self.refuse.saturating_sub(1);
            refused
        }
    }

    impl TrackControl for Track {
        fn play(&mut self) -> Result<(), PlayError> {
            unreachable!()
        }
        fn play_at(&mut self, _: u64) -> Result<(), PlayError> {
            unreachable!()
        }
        fn pause(&mut self) -> Result<(), PlayError> {
            unreachable!()
        }
        fn seek(&mut self, _: u64) -> Result<(), PlayError> {
            unreachable!()
        }
        fn set_rate(&mut self, rate: f64) -> Result<(), PlayError> {
            if self.refused() {
                return Err(PlayError::QueueFull);
            }
            self.rates.push(rate);
            Ok(())
        }
        fn adjust(&mut self, adjustments: &[TrackAdjustment]) -> Result<(), PlayError> {
            if self.refused() {
                return Err(PlayError::QueueFull);
            }
            self.adjustments.extend_from_slice(adjustments);
            Ok(())
        }
        fn generation(&self) -> u32 {
            0
        }
    }

    fn frequency(adjustments: [TrackAdjustment; 4]) -> (f64, f64) {
        match adjustments[0] {
            TrackAdjustment::Frequency {
                frequency,
                duration_ms,
            } => (frequency, duration_ms),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_mods_frequency_is_the_neutral_one_and_the_fall_starts_from_it() {
        assert_eq!(
            frequency(TrackEffects::Neutral.adjustments(1.5)),
            (1.5, 0.0)
        );
        // The move to 0 starts where the track is: at the mods' frequency.
        assert_eq!(
            frequency(TrackEffects::Failing.adjustments(1.5)),
            (0.0, FAIL_DURATION)
        );
        assert_eq!(frequency(TrackEffects::Failed.adjustments(1.5)), (0.0, 0.0));
        assert_eq!(
            TrackEffects::Failing.adjustments(1.5)[1..],
            TrackEffects::Failing.adjustments(1.0)[1..]
        );
        assert_eq!(frequency(neutral_track()), (1.0, 0.0));
    }

    #[test]
    fn a_reset_takes_the_tempo_and_the_effects_off_until_both_are_taken() {
        let mut track = Track::default();
        assert!(reset_track(&mut track).is_ok());
        assert_eq!(track.rates, [1.0]);
        assert_eq!(track.adjustments, neutral_track());
        // A refused adjustment fails the reset: the shell calls again.
        let mut track = Track {
            refuse: 1,
            ..Track::default()
        };
        assert_eq!(reset_track(&mut track), Err(PlayError::QueueFull));
        assert!(reset_track(&mut track).is_ok());
        assert_eq!(track.rates, [1.0]);
    }

    #[test]
    fn nothing_before_a_fail() {
        assert_eq!(FailFrame::new(None, 0.0, 0, 800.0, 600.0), None);
    }

    #[test]
    fn the_layer_shrinks_turns_and_greys_in_real_time() {
        let start = FailFrame::new(Some(view(false)), 10_000.0, 1_000 * MS, 800.0, 600.0).unwrap();
        assert_eq!(start.layer.apply([0.0, 0.0]), [0.0, 0.0]);
        assert_eq!((start.layer_grey, start.objects_alpha), (1.0, 1.0));
        // Gameplay time hardly moves; the layer goes by real time.
        let end = FailFrame::new(Some(view(false)), 11_250.0, 3_500 * MS, 800.0, 600.0).unwrap();
        let centre = end.layer.apply([400.0, 300.0]);
        assert!((centre[0] - 400.0).abs() < 1e-3 && (centre[1] - 300.0).abs() < 1e-3);
        // A point 100 px right of the centre: 85 px, turned 1° clockwise (down).
        let [x, y] = end.layer.apply([500.0, 300.0]);
        let (sin, cos) = 1f32.to_radians().sin_cos();
        assert!((x - (400.0 + 85.0 * cos)).abs() < 1e-3, "{x}");
        assert!((y - (300.0 + 85.0 * sin)).abs() < 1e-3, "{y}");
        assert!((end.layer_grey - LAYER_GREY).abs() < 1e-6);
        assert!((end.background_grey - BACKGROUND_GREY).abs() < 1e-6);
        // The objects are gone after 1250 ms of gameplay time.
        assert_eq!(end.objects_alpha, 0.0);
    }

    #[test]
    fn objects_fall_from_the_fail_or_from_appearing_by_gameplay_time() {
        let layout = PlayfieldLayout::new(1024.0, 768.0);
        let position = Vec2::new(256.0, 192.0);
        let pivot = layout.to_window(position);
        let at = |time: f64, appear: f64| {
            let frame = FailFrame::new(Some(view(false)), time, 2_000 * MS, 1024.0, 768.0).unwrap();
            frame
                .drop(7, appear, position, &layout)
                .apply([pivot.x, pivot.y])
        };
        // The pivot only moves down.
        assert_eq!(at(10_000.0, 0.0), [pivot.x, pivot.y]);
        let half = at(11_250.0, 0.0);
        assert!((half[0] - pivot.x).abs() < 1e-3);
        assert!((half[1] - (pivot.y + 200.0 * layout.scale())).abs() < 1e-3);
        // An object appearing later starts falling then.
        assert_eq!(at(11_000.0, 11_000.0), [pivot.x, pivot.y]);
        // A point beside the pivot turns and shrinks with it.
        let frame = FailFrame::new(Some(view(true)), 10_000.0, 1_000 * MS, 1024.0, 768.0).unwrap();
        let map = frame.drop(7, 0.0, position, &layout);
        let [x, y] = map.apply([pivot.x + 10.0, pivot.y]);
        let turn = drop_rotation(7).to_radians();
        let down = 400.0 * layout.scale();
        assert!((x - (pivot.x + 5.0 * turn.cos())).abs() < 1e-3);
        assert!((y - (pivot.y + down + 5.0 * turn.sin())).abs() < 1e-3);
        assert_eq!(frame.objects_alpha, 0.0);
    }

    #[test]
    fn turns_spread_over_both_ways_and_repeat() {
        let turns: Vec<f32> = (0..1_000).map(drop_rotation).collect();
        assert!(turns.iter().all(|t| (-90.0..90.0).contains(t)));
        assert!(turns.iter().any(|&t| t < -80.0) && turns.iter().any(|&t| t > 80.0));
        let mean = turns.iter().sum::<f32>() / turns.len() as f32;
        assert!(mean.abs() < 10.0, "{mean}");
        assert_eq!(drop_rotation(42), drop_rotation(42));
        assert_ne!(drop_rotation(42), drop_rotation(43));
    }
}
