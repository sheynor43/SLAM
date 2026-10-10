//! osu!standard hit objects after conversion.

use slam_formats::osu::{PathControlPoint, PathType, Vec2};

use crate::beatmap::{CONTROL_POINT_LENIENCY, Difficulty};
use crate::control_points::{ControlPoints, TimingPoint};
use crate::difficulty::{self, DifficultyRange};
use crate::dotnet;
use crate::events::{self, SliderEventKind};
use crate::path::SliderPath;
use crate::precision;
use crate::samples::{HitSample, SampleName};

/// Lazer's `OsuHitObject.OBJECT_RADIUS`: radius of an object at scale 1 in osu!pixels.
pub const OBJECT_RADIUS: f32 = 64.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuPlayfield.cs (BASE_SIZE)
/// Size of the playfield in osu!pixels.
pub const PLAYFIELD_SIZE: Vec2 = Vec2 { x: 512.0, y: 384.0 };

/// Lazer's `OsuHitObject.BASE_SCORING_DISTANCE` (a float in lazer): osu!pixels a slider
/// travels per beat at slider multiplier 1.
pub const BASE_SCORING_DISTANCE: f32 = 100.0;

/// Lazer's `OsuHitObject.PREEMPT_MIN`: preempt at AR 10.
pub const PREEMPT_MIN: f64 = 450.0;
/// Lazer's `OsuHitObject.PREEMPT_MID`: preempt at AR 5.
pub const PREEMPT_MID: f64 = 1200.0;
/// Lazer's `OsuHitObject.PREEMPT_MAX`: preempt at AR 0.
pub const PREEMPT_MAX: f64 = 1800.0;
/// Lazer's `OsuHitObject.PREEMPT_RANGE`: preempt by approach rate.
pub const PREEMPT_RANGE: DifficultyRange =
    DifficultyRange::new(PREEMPT_MAX, PREEMPT_MID, PREEMPT_MIN);

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Types/IHasComboInformation.cs
/// Combo position of an object (lazer's `IHasComboInformation`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ComboInfo {
    /// Index of the object's combo, starting at 1. Spinners never start a combo, so a spinner
    /// at the start of the map has index 0.
    pub combo_index: i32,
    /// [`combo_index`](Self::combo_index) including the combo colour skips of
    /// [`OsuHitObject::combo_offset`]; selects the combo colour.
    pub combo_index_with_offsets: i32,
    /// Index of the object within its combo, starting at 0.
    pub index_in_current_combo: i32,
    /// Whether the next object starts a new combo. False for the last object of the map.
    pub last_in_combo: bool,
}

/// Timing and size values every osu!standard object gets from the difficulty settings
/// (`OsuHitObject.ApplyDefaultsToSelf`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectDefaults {
    /// How long before the start time the object appears, in ms.
    pub time_preempt: f64,
    /// How long the object takes to fade in, in ms.
    pub time_fade_in: f64,
    /// Scale of the object; the radius is [`OBJECT_RADIUS`] times this.
    pub scale: f32,
}

impl Default for ObjectDefaults {
    /// Lazer's initial values, before defaults are applied.
    fn default() -> Self {
        ObjectDefaults {
            time_preempt: 600.0,
            time_fade_in: 400.0,
            scale: 1.0,
        }
    }
}

impl ObjectDefaults {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/OsuHitObject.cs (ApplyDefaultsToSelf)
    /// Preempt and fade-in from the approach rate, scale from the circle size.
    pub fn new(difficulty: &Difficulty) -> ObjectDefaults {
        let time_preempt = f64::from(PREEMPT_RANGE.value_int(f64::from(difficulty.approach_rate)));

        ObjectDefaults {
            time_preempt,
            // Preempt below 450 ms (AR above 10 through mods) shortens the fade-in, so that
            // circles still fade in fully.
            time_fade_in: 400.0 * dotnet::min(1.0, time_preempt / PREEMPT_MIN),
            scale: difficulty::scale_from_circle_size(difficulty.circle_size, true),
        }
    }

    /// Lazer's `OsuHitObject.Radius`: radius in osu!pixels.
    pub fn radius(&self) -> f64 {
        f64::from(OBJECT_RADIUS * self.scale)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/OsuHitObject.cs (StackOffset)
    /// Lazer's `OsuHitObject.StackOffset`: how far an object with `stack_height` is drawn from
    /// its position, up and left for positive heights. Spinners override it with zero
    /// ([`OsuHitObject::stack_offset`]).
    pub fn stack_offset(&self, stack_height: i32) -> Vec2 {
        // C# converts the int height to float before multiplying.
        let offset = stack_height as f32 * self.scale * -6.4;
        Vec2::new(offset, offset)
    }
}

/// The type-specific part of a [`NestedObject`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NestedKind {
    /// The slider head (lazer's `SliderHeadCircle`).
    Head,
    /// A tick (lazer's `SliderTick`).
    Tick {
        /// Index of the span the tick is in.
        span_index: i32,
        /// Start time of that span in ms.
        span_start_time: f64,
        /// Progress along the path, 0..=1.
        path_progress: f64,
    },
    /// A repeat (lazer's `SliderRepeat`).
    Repeat {
        /// Index of the span the repeat ends.
        repeat_index: i32,
        /// Progress along the path: 0 or 1.
        path_progress: f64,
    },
    /// The tail (lazer's `SliderTailCircle`).
    Tail {
        /// Index of the last span.
        repeat_index: i32,
    },
}

/// An object nested in a slider. Combo information is the slider's.
#[derive(Debug, Clone, PartialEq)]
pub struct NestedObject {
    /// Type-specific data.
    pub kind: NestedKind,
    /// Start time in ms.
    pub start_time: f64,
    /// Position in osu!pixels (before stacking).
    pub position: Vec2,
    /// The slider's stack height.
    pub stack_height: i32,
    /// Timing and size.
    pub defaults: ObjectDefaults,
    /// Samples played when the object is hit. The tail has none: the slider plays its
    /// [`Slider::tail_samples`] at its end time.
    pub samples: Vec<HitSample>,
}

impl NestedObject {
    /// Lazer's `StackedPosition`: the position shifted by the stack offset.
    pub fn stacked_position(&self) -> Vec2 {
        self.position + self.defaults.stack_offset(self.stack_height)
    }
}

/// A slider.
#[derive(Debug, Clone, PartialEq)]
pub struct Slider {
    /// The evaluated path, relative to the object position, with lazer's osu!standard Catmull
    /// optimisation.
    pub path: SliderPath,
    /// Distance of the path the decoder evaluates (`ConvertSlider.Path`, without the Catmull
    /// optimisation). Node sample times and the zero-length-slider rule use this one.
    pub legacy_distance: f64,
    /// Number of repeats. Zero for a zero-length slider (see [`Slider::new`]).
    pub repeat_count: i32,
    /// Resolved samples of each node (head, repeats, tail).
    pub node_samples: Vec<Vec<HitSample>>,
    /// False when the difficulty point at the start disables tick generation (NaN beat length).
    pub generate_ticks: bool,
    /// Slider velocity of the difficulty point at the start.
    pub slider_velocity_multiplier: f64,
    /// Multiplier of the tick distance: `1 / slider velocity` for format versions below 8,
    /// where velocity changes did not change the tick count over a distance, otherwise 1.
    pub tick_distance_multiplier: f64,
    /// Path distance travelled per ms. Set by [`OsuHitObject::apply_defaults`].
    pub velocity: f64,
    /// Path distance between ticks; infinite when ticks are not generated. Set by
    /// [`OsuHitObject::apply_defaults`].
    pub tick_distance: f64,
    /// Head, ticks, repeats and tail sorted by start time. Created by
    /// [`OsuHitObject::apply_defaults`].
    pub nested: Vec<NestedObject>,
    /// Samples the slider plays at its end time (the tail node's samples). Set by
    /// [`OsuHitObject::apply_defaults`].
    pub tail_samples: Vec<HitSample>,
}

impl Slider {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (createSlider)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (applyDefaults, applySamples)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Beatmaps/OsuBeatmapConverter.cs
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (Path)
    /// Converts a parsed slider starting at `start_time`: evaluates its path, takes the
    /// velocity and tick settings of the difficulty point at the start and resolves the node
    /// samples (see [`Slider::legacy_duration`]). Velocity, ticks and nested objects come later
    /// from [`OsuHitObject::apply_defaults`].
    ///
    /// A slider whose path has (almost) zero length gets no repeats and keeps only its first
    /// and last node samples: lazer's guard against zero-length sliders with repeats, which
    /// deliberately differs from osu!stable.
    pub fn new(
        parsed: slam_formats::osu::Slider,
        start_time: f64,
        control_points: &ControlPoints,
        difficulty: &Difficulty,
        format_version: i32,
    ) -> Slider {
        let legacy_path = SliderPath::new(parsed.control_points, parsed.expected_distance, false);
        let legacy_distance = legacy_path.distance();

        let mut repeat_count = parsed.repeat_count;
        let mut node_samples = parsed.node_samples;

        if precision::almost_equals_f64(legacy_distance, 0.0) {
            repeat_count = 0;
            // The parser always gives `repeat_count + 2` nodes; lazer would throw on none.
            if let (Some(first), Some(last)) = (node_samples.first(), node_samples.last()) {
                node_samples = vec![first.clone(), last.clone()];
            }
        }

        // The Catmull optimisation only changes Catmull segments.
        let has_catmull = legacy_path
            .control_points()
            .iter()
            .any(|p| p.path_type == Some(PathType::Catmull));
        let path = if has_catmull {
            SliderPath::new(
                legacy_path.control_points().to_vec(),
                legacy_path.expected_distance(),
                true,
            )
        } else {
            legacy_path
        };

        let difficulty_point = control_points.difficulty_point_at(start_time);
        // Prior to v8, speed multipliers don't adjust for how many ticks are generated over
        // the same distance.
        let tick_distance_multiplier = if format_version < 8 {
            1.0 / difficulty_point.slider_velocity
        } else {
            1.0
        };

        let mut slider = Slider {
            path,
            legacy_distance,
            repeat_count,
            node_samples: Vec::new(),
            generate_ticks: difficulty_point.generate_ticks,
            slider_velocity_multiplier: difficulty_point.slider_velocity,
            tick_distance_multiplier,
            velocity: 0.0,
            tick_distance: 0.0,
            nested: Vec::new(),
            tail_samples: Vec::new(),
        };
        slider.node_samples =
            slider.resolve_node_samples(&node_samples, control_points, difficulty, start_time);
        slider
    }

    /// Number of spans: `repeat_count + 1`.
    pub fn span_count(&self) -> i32 {
        self.repeat_count.wrapping_add(1)
    }

    /// Lazer's `Slider.EndTime`: the time the last span ends. Needs the velocity from
    /// [`OsuHitObject::apply_defaults`]; before that the result is infinite or NaN.
    pub fn end_time(&self, start_time: f64) -> f64 {
        start_time + f64::from(self.span_count()) * self.path.distance() / self.velocity
    }

    /// Lazer's `Slider.Duration`.
    pub fn duration(&self, start_time: f64) -> f64 {
        self.end_time(start_time) - start_time
    }

    /// Lazer's `Slider.SpanDuration`: duration of one span.
    pub fn span_duration(&self, start_time: f64) -> f64 {
        self.duration(start_time) / f64::from(self.span_count())
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Types/IHasPathWithRepeats.cs (CurvePositionAt, ProgressAt, SpanAt)
    /// Position along the path relative to the slider position at `progress` of the whole
    /// slider (0 = start time, 1 = end time), following the path back and forth over the spans.
    pub fn curve_position_at(&self, progress: f64) -> Vec2 {
        let span_count = f64::from(self.span_count());
        let mut p = progress * span_count % 1.0;
        // C#'s (int) cast, which saturates like `as` since .NET 9.
        if ((progress * span_count) as i32) % 2 == 1 {
            p = 1.0 - p;
        }
        self.path.position_at(p)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertSlider.cs (ApplyDefaultsToSelf, Duration)
    /// The duration lazer's decoder uses to place the node samples (`ConvertSlider.Duration`).
    ///
    /// This is not the gameplay [`duration`](Self::duration), whose velocity uses a
    /// precision-adjusted beat length; node sample lookup must use this one.
    pub fn legacy_duration(
        &self,
        control_points: &ControlPoints,
        difficulty: &Difficulty,
        start_time: f64,
    ) -> f64 {
        let timing_point = control_points.timing_point_at(start_time);
        let scoring_distance = f64::from(BASE_SCORING_DISTANCE)
            * difficulty.slider_multiplier
            * self.slider_velocity_multiplier;
        let velocity = scoring_distance / timing_point.beat_length;

        f64::from(self.span_count()) * self.legacy_distance / velocity
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (applySamples)
    /// Resolves parsed node samples: node `i` takes the sample point active at
    /// `start_time + i * duration / span_count` plus the control point leniency, with the
    /// duration from [`Slider::legacy_duration`].
    fn resolve_node_samples(
        &self,
        node_samples: &[Vec<slam_formats::osu::HitSample>],
        control_points: &ControlPoints,
        difficulty: &Difficulty,
        start_time: f64,
    ) -> Vec<Vec<HitSample>> {
        let duration = self.legacy_duration(control_points, difficulty, start_time);
        let span_count = f64::from(self.span_count());

        node_samples
            .iter()
            .enumerate()
            .map(|(i, samples)| {
                // Node counts come from an i32 repeat count.
                let i = i as f64;
                let time = start_time + i * duration / span_count + CONTROL_POINT_LENIENCY;
                let point = control_points.sample_point_at(time);
                samples.iter().map(|s| point.apply_to(s)).collect()
            })
            .collect()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (ApplyDefaultsToSelf)
    /// Velocity and tick distance from the timing point and difficulty.
    fn apply_defaults_to_self(&mut self, timing_point: &TimingPoint, difficulty: &Difficulty) {
        self.velocity = f64::from(BASE_SCORING_DISTANCE) * difficulty.slider_multiplier
            / precision_adjusted_beat_length(self.slider_velocity_multiplier, timing_point);
        // Intentionally not `BASE_SCORING_DISTANCE * slider_multiplier`: lazer keeps the
        // floating point error of osu!stable.
        let scoring_distance = self.velocity * timing_point.beat_length;

        self.tick_distance = if self.generate_ticks {
            scoring_distance / difficulty.slider_tick_rate * self.tick_distance_multiplier
        } else {
            f64::INFINITY
        };
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (CreateNestedHitObjects, UpdateNestedSamples)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/HitObject.cs (ApplyDefaults)
    /// Creates the nested objects, sorts them by start time and applies their defaults.
    fn create_nested(
        &mut self,
        start_time: f64,
        position: Vec2,
        stack_height: i32,
        samples: &[HitSample],
        defaults: ObjectDefaults,
    ) {
        let span_duration = self.span_duration(start_time);
        let span_count = self.span_count();

        let mut slider_events = Vec::new();
        events::generate(
            &mut slider_events,
            start_time,
            span_duration,
            self.velocity,
            self.tick_distance,
            self.path.distance(),
            span_count,
        );

        self.populate_node_samples(samples);

        // The tick sample is the body's `hitnormal`, or its first sample, renamed.
        let tick_sample = samples
            .iter()
            .find(|s| s.name == SampleName::Normal)
            .or(samples.first())
            .map(|s| s.with_name(SampleName::SliderTick));

        let end_position = position + self.curve_position_at(1.0);

        let mut nested = Vec::with_capacity(slider_events.len());
        for e in &slider_events {
            let (kind, time, pos, nested_samples) = match e.kind {
                SliderEventKind::Tick => (
                    NestedKind::Tick {
                        span_index: e.span_index,
                        span_start_time: e.span_start_time,
                        path_progress: e.path_progress,
                    },
                    e.time,
                    position + self.path.position_at(e.path_progress),
                    tick_sample.iter().cloned().collect(),
                ),
                SliderEventKind::Head => (
                    NestedKind::Head,
                    e.time,
                    position,
                    self.node_samples_at(0, samples),
                ),
                SliderEventKind::Tail => (
                    NestedKind::Tail {
                        repeat_index: e.span_index,
                    },
                    e.time,
                    end_position,
                    Vec::new(),
                ),
                SliderEventKind::Repeat => (
                    NestedKind::Repeat {
                        repeat_index: e.span_index,
                        path_progress: e.path_progress,
                    },
                    start_time + f64::from(e.span_index.wrapping_add(1)) * span_duration,
                    position + self.path.position_at(e.path_progress),
                    // Node counts come from an i32 repeat count.
                    self.node_samples_at(e.span_index.wrapping_add(1) as usize, samples),
                ),
                SliderEventKind::LegacyLastTick => continue,
            };

            nested.push(NestedObject {
                kind,
                start_time: time,
                position: pos,
                stack_height,
                defaults,
                samples: nested_samples,
            });
        }

        self.tail_samples =
            self.node_samples_at(self.repeat_count.wrapping_add(1) as usize, samples);

        dotnet::list_sort(&mut nested, |a, b| {
            dotnet::compare_f64(a.start_time, b.start_time)
        });

        for n in &mut nested {
            apply_nested_defaults(n, start_time, span_duration);
        }

        self.nested = nested;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Utils/OsuHitObjectGenerationUtils.cs (modifySlider)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (Path)
    /// Changes every control point and re-evaluates the path with the same expected distance.
    /// Velocity, tick distance and nested objects stay; call
    /// [`update_nested_positions`](Self::update_nested_positions) afterwards.
    fn modify_path(&mut self, modify: impl Fn(Vec2) -> Vec2) {
        let control_points = self
            .path
            .control_points()
            .iter()
            .map(|p| PathControlPoint {
                position: modify(p.position),
                path_type: p.path_type,
            })
            .collect();
        self.path = SliderPath::new(control_points, self.path.expected_distance(), true);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (updateNestedPositions)
    /// Moves the nested objects to the slider at `position` along the current path, as lazer
    /// does whenever the position or the path of a slider changes.
    fn update_nested_positions(&mut self, position: Vec2) {
        let end_position = position + self.curve_position_at(1.0);
        for n in &mut self.nested {
            n.position = match n.kind {
                NestedKind::Head => position,
                NestedKind::Tail { .. } => end_position,
                NestedKind::Repeat { path_progress, .. }
                | NestedKind::Tick { path_progress, .. } => {
                    position + self.path.position_at(path_progress)
                }
            };
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Types/IHasRepeats.cs (PopulateNodeSamples)
    /// Gives nodes without samples copies of the body samples.
    fn populate_node_samples(&mut self, samples: &[HitSample]) {
        let wanted = i64::from(self.repeat_count) + 2;
        while (self.node_samples.len() as i64) < wanted {
            // `With()` without changes rebuilds the same sample.
            self.node_samples.push(samples.to_vec());
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Types/IHasRepeats.cs (GetNodeSamples)
    /// Samples of node `index`, or the body samples if there is no such node.
    fn node_samples_at(&self, index: usize, samples: &[HitSample]) -> Vec<HitSample> {
        self.node_samples
            .get(index)
            .map_or_else(|| samples.to_vec(), Clone::clone)
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/LegacyRulesetExtensions.cs (GetPrecisionAdjustedBeatLength)
/// The beat length scaled by the slider velocity the way osu!stable stores it: as a negative
/// beat length clamped to 10..=1000 in single precision.
pub fn precision_adjusted_beat_length(slider_velocity: f64, timing_point: &TimingPoint) -> f64 {
    let slider_velocity_as_beat_length = -100.0 / slider_velocity;

    // In osu!stable the division happens on floats, but with optimisations it seems to occur
    // on doubles.
    let bpm_multiplier = if slider_velocity_as_beat_length < 0.0 {
        f64::from(dotnet::clamp_f32(
            (-slider_velocity_as_beat_length) as f32,
            10.0,
            1000.0,
        )) / 100.0
    } else {
        1.0
    };

    timing_point.beat_length * bpm_multiplier
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderTick.cs (ApplyDefaultsToSelf)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/SliderEndCircle.cs (ApplyDefaultsToSelf)
/// The type-specific defaults of a nested object, on top of the [`ObjectDefaults`] it shares
/// with the slider.
fn apply_nested_defaults(n: &mut NestedObject, slider_start_time: f64, span_duration: f64) {
    let d = &mut n.defaults;
    match n.kind {
        NestedKind::Head => {}
        NestedKind::Tick {
            span_index,
            span_start_time,
            ..
        } => {
            let offset = if span_index > 0 {
                // osu!stable's offset, so that ticks on repeats don't appear too late to see.
                200.0
            } else {
                d.time_preempt * f64::from(0.66f32)
            };
            d.time_preempt = (n.start_time - span_start_time) / 2.0 + offset;
        }
        NestedKind::Repeat { repeat_index, .. } | NestedKind::Tail { repeat_index } => {
            if repeat_index > 0 {
                // Later end circles appear behind the still visible one, right after the
                // previous circle on the same end is hit.
                d.time_fade_in = 0.0;
                d.time_preempt = span_duration * 2.0;
            } else {
                // The first end circle fades in with the slider.
                d.time_preempt += n.start_time - slider_start_time;
            }
        }
    }
}

/// The type-specific part of an [`OsuHitObject`].
#[derive(Debug, Clone, PartialEq)]
pub enum OsuHitObjectKind {
    /// A hit circle.
    Circle,
    /// A slider (boxed: it is much larger than the other kinds).
    Slider(Box<Slider>),
    /// A spinner.
    Spinner {
        /// End time in ms.
        end_time: f64,
    },
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/OsuHitObject.cs
/// An osu!standard hit object.
#[derive(Debug, Clone, PartialEq)]
pub struct OsuHitObject {
    /// Start time in ms.
    pub start_time: f64,
    /// Position in osu!pixels (before stacking).
    pub position: Vec2,
    /// Whether the object starts a new combo.
    pub new_combo: bool,
    /// Combo colour skip count.
    pub combo_offset: i32,
    /// Combo position.
    pub combo: ComboInfo,
    /// Stack height, set by stacking ([`Beatmap::apply_stacking`](crate::Beatmap::apply_stacking)).
    /// Change it with [`set_stack_height`](Self::set_stack_height), which also updates the
    /// nested objects.
    pub stack_height: i32,
    /// Timing and size. Set by [`OsuHitObject::apply_defaults`].
    pub defaults: ObjectDefaults,
    /// Resolved samples (a slider's body samples).
    pub samples: Vec<HitSample>,
    /// Type-specific data.
    pub kind: OsuHitObjectKind,
}

impl OsuHitObject {
    /// Whether this is a spinner.
    pub fn is_spinner(&self) -> bool {
        matches!(self.kind, OsuHitObjectKind::Spinner { .. })
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/HitObject.cs (GetEndTime)
    /// End time in ms: the start time for a circle.
    pub fn end_time(&self) -> f64 {
        match &self.kind {
            OsuHitObjectKind::Circle => self.start_time,
            OsuHitObjectKind::Slider(slider) => slider.end_time(self.start_time),
            OsuHitObjectKind::Spinner { end_time } => *end_time,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (EndPosition)
    /// Lazer's `EndPosition`: where the object ends, before stacking. For a slider this is the
    /// end of its last span.
    pub fn end_position(&self) -> Vec2 {
        match &self.kind {
            OsuHitObjectKind::Slider(slider) => self.position + slider.curve_position_at(1.0),
            _ => self.position,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Spinner.cs (StackOffset)
    /// Lazer's `StackOffset`: how far the object is drawn from its position. Spinners are never
    /// moved, whatever their stack height.
    pub fn stack_offset(&self) -> Vec2 {
        if self.is_spinner() {
            Vec2::ZERO
        } else {
            self.defaults.stack_offset(self.stack_height)
        }
    }

    /// Lazer's `StackedPosition`: the position shifted by the stack offset.
    pub fn stacked_position(&self) -> Vec2 {
        self.position + self.stack_offset()
    }

    /// Lazer's `StackedEndPosition`: the end position shifted by the stack offset.
    pub fn stacked_end_position(&self) -> Vec2 {
        self.end_position() + self.stack_offset()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/OsuHitObject.cs (constructor, StackHeightBindable)
    /// Sets the stack height of the object and of its nested objects.
    pub fn set_stack_height(&mut self, stack_height: i32) {
        self.stack_height = stack_height;
        if let OsuHitObjectKind::Slider(slider) = &mut self.kind {
            for n in &mut slider.nested {
                n.stack_height = stack_height;
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Utils/OsuHitObjectGenerationUtils.cs (ReflectHorizontallyAlongPlayfield)
    /// Mirrors the object left to right across the playfield; a slider's path is mirrored
    /// and its nested objects follow. Used by Mirror.
    pub fn reflect_horizontally(&mut self) {
        self.position = Vec2::new(PLAYFIELD_SIZE.x - self.position.x, self.position.y);
        if let OsuHitObjectKind::Slider(slider) = &mut self.kind {
            slider.modify_path(|p| Vec2::new(-p.x, p.y));
            slider.update_nested_positions(self.position);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Utils/OsuHitObjectGenerationUtils.cs (ReflectVerticallyAlongPlayfield)
    /// Mirrors the object top to bottom across the playfield; a slider's path is mirrored and
    /// its nested objects follow. Used by Hard Rock and Mirror.
    pub fn reflect_vertically(&mut self) {
        self.position = Vec2::new(self.position.x, PLAYFIELD_SIZE.y - self.position.y);
        if let OsuHitObjectKind::Slider(slider) = &mut self.kind {
            slider.modify_path(|p| Vec2::new(p.x, -p.y));
            slider.update_nested_positions(self.position);
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/HitObject.cs (ApplyDefaults)
    /// Applies the difficulty settings: preempt, fade-in and scale, and for a slider its
    /// velocity, tick distance and nested objects. Can be called again after the difficulty
    /// changes (mods); nested objects are recreated with the current stack height. Stacking
    /// reads the preempt and the slider end times, so run
    /// [`Beatmap::apply_stacking`](crate::Beatmap::apply_stacking) afterwards, as lazer runs
    /// `PostProcess` after the defaults and the mods.
    ///
    /// Spinner rules (required spins, spinner ticks) are not applied yet.
    pub fn apply_defaults(&mut self, control_points: &ControlPoints, difficulty: &Difficulty) {
        self.defaults = ObjectDefaults::new(difficulty);

        if let OsuHitObjectKind::Slider(slider) = &mut self.kind {
            let timing_point = control_points.timing_point_at(self.start_time);
            slider.apply_defaults_to_self(&timing_point, difficulty);
            slider.create_nested(
                self.start_time,
                self.position,
                self.stack_height,
                &self.samples,
                self.defaults,
            );
        }
    }
}
