//! Data model of the `[HitObjects]` section.
//!
//! The types mirror what osu!lazer's `ConvertHitObjectParser` produces (`ConvertHitObject`,
//! `ConvertSlider`, `ConvertSpinner`, `ConvertHold`, `LegacyHitSampleInfo`, `PathControlPoint`).
//! Fields lazer fills in later (`LegacyBeatmapDecoder.applySamples`, `SliderPath` evaluation,
//! `ApplyDefaults`) are left exactly as the parser leaves them.

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObject.cs
/// A 2D point or vector with `f32` components, like osuTK's `Vector2`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Vec2 {
    /// Horizontal component.
    pub x: f32,
    /// Vertical component.
    pub y: f32,
}

impl Vec2 {
    /// The zero vector.
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    /// Creates a vector.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// `X * X + Y * Y`, in `f32` (osuTK's `LengthSquared`).
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    /// The Euclidean length (osuTK's `Length`: `(float)Math.Sqrt(X * X + Y * Y)`).
    ///
    /// The `f64` square root of an `f32`, rounded back to `f32`, equals the `f32` square root.
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// The distance between two points (osuTK's `Vector2.Distance`).
    pub fn distance(self, other: Vec2) -> f32 {
        (other - self).length()
    }

    /// The dot product (osuTK's `Vector2.Dot`).
    pub fn dot(self, other: Vec2) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// The vector scaled to unit length (osuTK's `Normalized`: multiplies by `1 / Length`).
    pub fn normalized(self) -> Vec2 {
        let scale = 1.0 / self.length();
        Vec2::new(self.x * scale, self.y * scale)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;

    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;

    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Vec2;

    fn mul(self, scale: f32) -> Vec2 {
        Vec2::new(self.x * scale, self.y * scale)
    }
}

impl std::ops::Mul<Vec2> for f32 {
    type Output = Vec2;

    fn mul(self, vec: Vec2) -> Vec2 {
        Vec2::new(self * vec.x, self * vec.y)
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Vec2;

    /// Component-wise division (osuTK's `operator /(Vector2, float)`).
    fn div(self, scale: f32) -> Vec2 {
        Vec2::new(self.x / scale, self.y / scale)
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Types/PathType.cs
/// A slider curve type (lazer's `PathType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathType {
    /// `C`, and any other unrecognised letter.
    Catmull,
    /// `B` without a valid degree suffix (lazer's `PathType.BEZIER`, a B-spline without degree).
    Bezier,
    /// `B<degree>` with a positive degree.
    BSpline {
        /// The spline degree, always positive.
        degree: i32,
    },
    /// `L`.
    Linear,
    /// `P`.
    PerfectCurve,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/PathControlPoint.cs
/// A slider path control point (lazer's `PathControlPoint`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathControlPoint {
    /// Position relative to the slider's start position. The first point is always `(0, 0)`
    /// unless the point string had points before its first curve type letter.
    pub position: Vec2,
    /// The type of the path segment starting at this point, or `None` when the point continues
    /// the previous segment.
    pub path_type: Option<PathType>,
}

/// The name of a hit sample (lazer's `HitSampleInfo.Name` for the legacy sounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitSampleName {
    /// `hitnormal`.
    Normal,
    /// `hitwhistle`.
    Whistle,
    /// `hitfinish`.
    Finish,
    /// `hitclap`.
    Clap,
}

impl HitSampleName {
    /// Lazer's sample name string (`hitnormal`, `hitwhistle`, `hitfinish`, `hitclap`).
    pub fn as_str(self) -> &'static str {
        match self {
            HitSampleName::Normal => "hitnormal",
            HitSampleName::Whistle => "hitwhistle",
            HitSampleName::Finish => "hitfinish",
            HitSampleName::Clap => "hitclap",
        }
    }
}

/// A sample bank (lazer's `HitSampleInfo.BANK_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleBank {
    /// `normal`; also the bank used when none was specified.
    Normal,
    /// `soft`.
    Soft,
    /// `drum`.
    Drum,
}

impl SampleBank {
    /// Lazer's bank string (`normal`, `soft`, `drum`).
    pub fn as_str(self) -> &'static str {
        match self {
            SampleBank::Normal => "normal",
            SampleBank::Soft => "soft",
            SampleBank::Drum => "drum",
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObjectParser.cs (LegacyHitSampleInfo), osu.Game/Audio/HitSampleInfo.cs
/// A hit sample as lazer's `ConvertHitObjectParser` creates it (`LegacyHitSampleInfo`, or
/// `FileHitSampleInfo` when [`filename`](Self::filename) is set).
///
/// Banks and volumes that the file does not specify are left unresolved
/// ([`bank_specified`](Self::bank_specified) is false, [`volume`](Self::volume) is 0); lazer
/// fills them from the sample control points afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitSample {
    /// Sample name.
    pub name: HitSampleName,
    /// Bank; [`SampleBank::Normal`] when none was specified.
    pub bank: SampleBank,
    /// Whether a bank was specified on the hit object. If false, the bank comes from the
    /// closest sample control point.
    pub bank_specified: bool,
    /// Lookup suffix: the custom sample bank index when it is 2 or above.
    pub suffix: Option<i32>,
    /// Volume 0..=100 as read from the file; 0 means "take it from the control point".
    pub volume: i32,
    /// Whether the addition bank follows the normal bank in the editor.
    pub editor_auto_bank: bool,
    /// Whether the sample can be looked up in the beatmap's own samples (custom sample bank >= 1).
    pub use_beatmap_samples: bool,
    /// Whether this is a layered normal sample (the sound type had no `Normal` flag).
    pub is_layered: bool,
    /// Custom sample file name; replaces all bank-based lookups.
    pub filename: Option<String>,
}

impl HitSample {
    /// Lazer's `LegacyHitSampleInfo.CustomSampleBank`.
    pub fn custom_sample_bank(&self) -> i32 {
        match self.suffix {
            Some(s) => s,
            None => i32::from(self.use_beatmap_samples),
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertSlider.cs
/// Slider-specific data (lazer's `ConvertSlider`, before any path evaluation).
#[derive(Debug, Clone, PartialEq)]
pub struct Slider {
    /// Path control points, relative to the object position.
    pub control_points: Vec<PathControlPoint>,
    /// The pixel length from the file (`SliderPath.ExpectedDistance`); `None` when absent or
    /// not positive.
    pub expected_distance: Option<f64>,
    /// Number of repeats: the file's slide count minus one, at least 0.
    ///
    /// Lazer additionally resets this to 0 for sliders whose evaluated path length is zero,
    /// and trims `node_samples` to `[first, last]`; that needs the path calculation and is
    /// applied by `slam-osu`, not here.
    pub repeat_count: i32,
    /// Samples of each node (head, repeats, tail): `repeat_count + 2` entries. This holds only
    /// before the zero-length-slider rule described at [`repeat_count`](Self::repeat_count).
    pub node_samples: Vec<Vec<HitSample>>,
}

impl Slider {
    /// Number of spans: `repeat_count + 1`.
    pub fn span_count(&self) -> i32 {
        self.repeat_count.saturating_add(1)
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertSpinner.cs, ConvertHold.cs, ConvertHitCircle.cs
/// The type-specific part of a [`HitObject`].
#[derive(Debug, Clone, PartialEq)]
pub enum HitObjectKind {
    /// A hit circle.
    Circle,
    /// A slider.
    Slider(Slider),
    /// A spinner.
    Spinner {
        /// `ConvertSpinner.Duration` in ms; the end time is `start_time + duration`.
        duration: f64,
    },
    /// A mania hold note (only generated by BMS converts).
    Hold {
        /// `ConvertHold.Duration` in ms; the end time is `start_time + duration`.
        duration: f64,
    },
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Objects/Legacy/ConvertHitObject.cs
/// A hit object as parsed from one `[HitObjects]` line.
#[derive(Debug, Clone, PartialEq)]
pub struct HitObject {
    /// Start time in ms, including the timing offset of old maps.
    pub start_time: f64,
    /// Position in osu!pixels. Spinners are always at `(256, 192)`.
    pub position: Vec2,
    /// Whether the object starts a new combo (forced for the first object and after a spinner;
    /// never set for holds). Combos forced after breaks (`postProcessBreaks`) are applied later.
    pub new_combo: bool,
    /// Combo colour skip count; 0 unless `new_combo` is set by the file's flag. Always 0 for
    /// spinners and holds.
    pub combo_offset: i32,
    /// The legacy type bits with the new-combo and combo-offset bits removed.
    pub legacy_type: i32,
    /// Samples of the object (a slider's body samples; see [`Slider::node_samples`] for nodes).
    pub samples: Vec<HitSample>,
    /// Type-specific data.
    pub kind: HitObjectKind,
}

impl HitObject {
    /// The end time for spinners and holds, `start_time + duration` as lazer computes it;
    /// `None` for circles and sliders (a slider's end time needs path evaluation).
    pub fn end_time(&self) -> Option<f64> {
        match self.kind {
            HitObjectKind::Spinner { duration } | HitObjectKind::Hold { duration } => {
                Some(self.start_time + duration)
            }
            _ => None,
        }
    }
}
