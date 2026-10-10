//! osu!standard hit objects after conversion.

use slam_formats::osu::{PathType, Vec2};

use crate::beatmap::{CONTROL_POINT_LENIENCY, Difficulty};
use crate::control_points::ControlPoints;
use crate::path::SliderPath;
use crate::precision;
use crate::samples::HitSample;

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
    /// Samples of each node (head, repeats, tail) as parsed. Their sample points depend on the
    /// node times, so they are resolved with [`Slider::resolve_node_samples`].
    pub node_samples: Vec<Vec<slam_formats::osu::HitSample>>,
    /// False when the difficulty point at the start disables tick generation (NaN beat length).
    pub generate_ticks: bool,
    /// Slider velocity of the difficulty point at the start.
    pub slider_velocity_multiplier: f64,
    /// Multiplier of the tick distance: `1 / slider velocity` for format versions below 8,
    /// where velocity changes did not change the tick count over a distance, otherwise 1.
    pub tick_distance_multiplier: f64,
}

impl Slider {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (createSlider)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Objects/Slider.cs (Path)
    /// Evaluates the path of a parsed slider. The difficulty-dependent fields are left at their
    /// defaults (velocity 1, ticks generated, tick distance multiplier 1).
    ///
    /// A slider whose path has (almost) zero length gets no repeats and keeps only its first
    /// and last node samples: lazer's guard against zero-length sliders with repeats, which
    /// deliberately differs from osu!stable.
    pub fn new(parsed: slam_formats::osu::Slider) -> Slider {
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

        Slider {
            path,
            legacy_distance,
            repeat_count,
            node_samples,
            generate_ticks: true,
            slider_velocity_multiplier: 1.0,
            tick_distance_multiplier: 1.0,
        }
    }

    /// Number of spans: `repeat_count + 1`.
    pub fn span_count(&self) -> i32 {
        self.repeat_count.wrapping_add(1)
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertSlider.cs (ApplyDefaultsToSelf, Duration)
    /// The duration lazer's decoder uses to place the node samples (`ConvertSlider.Duration`).
    ///
    /// This is not the gameplay duration of the osu!standard slider, whose velocity uses a
    /// precision-adjusted beat length; node sample lookup must use this one.
    pub fn legacy_duration(
        &self,
        control_points: &ControlPoints,
        difficulty: &Difficulty,
        start_time: f64,
    ) -> f64 {
        /// `ConvertSlider.base_scoring_distance` (a float in lazer).
        const BASE_SCORING_DISTANCE: f32 = 100.0;

        let timing_point = control_points.timing_point_at(start_time);
        let scoring_distance = f64::from(BASE_SCORING_DISTANCE)
            * difficulty.slider_multiplier
            * self.slider_velocity_multiplier;
        let velocity = scoring_distance / timing_point.beat_length;

        f64::from(self.span_count()) * self.legacy_distance / velocity
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/Formats/LegacyBeatmapDecoder.cs (applySamples)
    /// Resolves the node samples: node `i` takes the sample
    /// point active at `start_time + i * duration / span_count` plus the control point
    /// leniency, with the duration from [`Slider::legacy_duration`].
    pub fn resolve_node_samples(
        &self,
        control_points: &ControlPoints,
        difficulty: &Difficulty,
        start_time: f64,
    ) -> Vec<Vec<HitSample>> {
        let duration = self.legacy_duration(control_points, difficulty, start_time);
        let span_count = f64::from(self.span_count());

        self.node_samples
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
}

/// The type-specific part of an [`OsuHitObject`].
#[derive(Debug, Clone, PartialEq)]
pub enum OsuHitObjectKind {
    /// A hit circle.
    Circle,
    /// A slider.
    Slider(Slider),
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
}
