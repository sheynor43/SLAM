//! The volume overlay (ADR-0145): four meters at an edge of the window that the wheel with
//! Alt, the volume hotkeys and the pointer change, after lazer's `VolumeOverlay` and
//! `VolumeMeter`.
//!
//! Like the widgets it is input-agnostic: [`VolumeMixer`] is the update side (selection,
//! the step's acceleration, a touchpad's fractions, hiding, sparks), [`VolumeLayout`] is
//! shared by hit testing and drawing, [`VolumeView`] draws a [`VolumeFrame`]. The levels
//! themselves are the application's settings, passed in.

use std::fmt::Write as _;
use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::hal::Blend;
use slam_render::shape::{Gradient, Shape, ShapeBatch};
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use crate::anim::{ease_out_cubic, ease_out_expo, ease_out_quint, progress};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter, push_centred};
use crate::theme::{Theme, faded};
use crate::widgets::{push_left, push_right, push_shadow};

/// Meters of the overlay.
pub const METERS: usize = 4;
/// Sparks' bursts kept: the oldest is replaced.
pub const MAX_BURSTS: usize = 16;
/// Sparks of one burst at most, whatever the theme asks.
pub const MAX_SPARKS: usize = 16;
/// Dots of an arc's fill at most.
pub const ARC_DOTS: usize = 128;
/// Shapes of one frame: the shade; per meter at most six besides an arc's fill (shadow,
/// card, outline, track, glow, a column's or bar's fill), an arc's fill up to `ARC_DOTS`
/// dots; the sparks with their glows.
pub const MAX_VOLUME_SHAPES: usize = 1 + METERS * (6 + ARC_DOTS) + MAX_BURSTS * MAX_SPARKS * 2;
/// Glyph sprites of one frame: the labels and the values.
pub const MAX_VOLUME_GLYPHS: usize = 192;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Volume/VolumeMeter.cs
/// The step of one wheel notch or key press, and the levels' precision.
const STEP: f64 = 0.01;
const MAX_ACCELERATION: f64 = 5.0;
const ACCELERATION_MULTIPLIER: f64 = 1.8;
/// A pause this long resets the acceleration.
const ACCELERATION_RESET_NS: u64 = 150_000_000;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/VolumeOverlay.cs
/// The overlay hides this long after the last change or pointer movement over it.
const HIDE_AFTER_NS: u64 = 1_000_000_000;
/// `Precision.AlmostBigger`'s default tolerance.
const ALMOST: f64 = 1e-7;
/// The level shown as "MAX" from on (lazer's `displayVolume >= 0.995f`).
const MAX_SHOWN: f32 = 0.995;
/// The arc fills clockwise from the top round the whole ring.
const ARC_START: f32 = std::f32::consts::FRAC_PI_2;
const ARC_SWEEP: f32 = std::f32::consts::TAU;
/// Pulses of the full ring's glow per second.
const MAX_PULSE_HZ: f64 = 1.5;

/// A meter, in the order the overlay shows them (top to bottom, left to right).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Meter {
    Hitsounds,
    Effects,
    #[default]
    Master,
    Music,
}

impl Meter {
    pub const ALL: [Meter; METERS] = [
        Meter::Hitsounds,
        Meter::Effects,
        Meter::Master,
        Meter::Music,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The localization key of the meter's label.
    pub fn label_key(self) -> &'static str {
        match self {
            Meter::Hitsounds => "volume-meter-hitsounds",
            Meter::Effects => "volume-meter-effects",
            Meter::Master => "volume-meter-master",
            Meter::Music => "volume-meter-music",
        }
    }
}

/// The window edge the overlay stands at (`general.volume_overlay`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Edge {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    /// The name in settings files.
    pub fn name(self) -> &'static str {
        match self {
            Edge::Left => "left",
            Edge::Right => "right",
            Edge::Top => "top",
            Edge::Bottom => "bottom",
        }
    }

    pub fn from_name(name: &str) -> Option<Edge> {
        Edge::ALL.into_iter().find(|e| e.name() == name)
    }

    /// The localization key of the option's name.
    pub fn label_key(self) -> &'static str {
        match self {
            Edge::Left => "settings-general-volume-overlay-left",
            Edge::Right => "settings-general-volume-overlay-right",
            Edge::Top => "settings-general-volume-overlay-top",
            Edge::Bottom => "settings-general-volume-overlay-bottom",
        }
    }

    /// The meters stand in a column (left, right) rather than a row.
    fn vertical(self) -> bool {
        matches!(self, Edge::Left | Edge::Right)
    }
}

/// The levels of the meters, 0 to 1, in [`Meter::ALL`] order.
pub type Levels = [f64; METERS];

/// Sparks sent from a ring when its level changed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Burst {
    /// When; 0 is an empty slot.
    pub at_ns: u64,
    pub meter: Meter,
    /// The new level: where on the ring the sparks start.
    pub level: f32,
    /// 0 to 1: how many sparks and how fast.
    pub strength: f32,
    pub seed: u32,
}

/// What the draw side needs of the overlay in one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VolumeFrame {
    pub edge: Edge,
    pub style: MeterStyle,
    pub visible: bool,
    /// When `visible` last changed.
    pub since_ns: u64,
    pub selected: Meter,
    pub levels: [f32; METERS],
    pub muted: bool,
    pub bursts: [Burst; MAX_BURSTS],
}

/// The update side of the overlay.
#[derive(Clone, Debug, Default)]
pub struct VolumeMixer {
    visible: bool,
    since_ns: u64,
    /// The last change, pointer movement over the overlay or the pointer leaving it.
    active_ns: u64,
    selected: Meter,
    hovered: bool,
    /// The pointer moved at least once: before, its place is unknown and nothing hovers.
    pointer_known: bool,
    /// Each meter's own acceleration, last change and fractions of a step from precise
    /// scrolling, as lazer's `accelerationModifier` and `scrollAccumulation` of each
    /// `VolumeMeter`.
    acceleration: [f64; METERS],
    last_adjust_ns: [Option<u64>; METERS],
    accumulation: [f64; METERS],
    muted: bool,
    /// A level changed since the overlay showed.
    changed: bool,
    /// The overlay hid after a change: the settings are to be saved.
    save_due: bool,
    bursts: [Burst; MAX_BURSTS],
    next_burst: usize,
    seed: u32,
}

impl VolumeMixer {
    pub fn new() -> VolumeMixer {
        VolumeMixer {
            acceleration: [1.0; METERS],
            ..VolumeMixer::default()
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn selected(&self) -> Meter {
        self.selected
    }

    /// The sound is muted: the master level stays, the output is silent.
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Shows the overlay; hidden, it selects the master meter, as lazer's `Show`.
    pub fn show(&mut self, now_ns: u64) {
        if !self.visible {
            self.visible = true;
            self.since_ns = now_ns;
            self.selected = Meter::Master;
        }
        self.active_ns = now_ns;
    }

    /// Alt + wheel (`amount` notches, positive away from the user; `precise` for a
    /// touchpad's fractions) or a volume hotkey (±1): changes the selected meter. Hidden,
    /// it only shows the overlay, as lazer's `Adjust`. Returns whether a level changed.
    pub fn adjust(&mut self, levels: &mut Levels, amount: f64, precise: bool, now_ns: u64) -> bool {
        if !self.visible {
            self.show(now_ns);
            return false;
        }
        self.active_ns = now_ns;
        self.change(levels, self.selected, amount, precise, now_ns)
    }

    /// The wheel without Alt at (`x`, `y`): over a meter of the shown overlay it selects
    /// and changes that meter, as lazer's `VolumeMeter.OnScroll`. Returns whether it took
    /// the wheel.
    #[allow(clippy::too_many_arguments)]
    pub fn wheel_over(
        &mut self,
        levels: &mut Levels,
        amount: f64,
        precise: bool,
        x: f32,
        y: f32,
        layout: &VolumeLayout,
        now_ns: u64,
    ) -> bool {
        if !self.visible {
            return false;
        }
        let Some(meter) = layout.meter_at(x, y) else {
            return false;
        };
        self.select(meter);
        self.active_ns = now_ns;
        self.change(levels, meter, amount, precise, now_ns);
        true
    }

    /// Selects the next (or previous) meter, wrapping; only while shown. Returns whether it
    /// did.
    pub fn step_meter(&mut self, forward: bool, now_ns: u64) -> bool {
        if !self.visible {
            return false;
        }
        let i = self.selected.index();
        let next = if forward {
            (i + 1) % METERS
        } else {
            (i + METERS - 1) % METERS
        };
        self.select(Meter::ALL[next]);
        self.active_ns = now_ns;
        true
    }

    /// Mutes or unmutes and shows the overlay, as lazer's `ToggleMute`.
    pub fn toggle_mute(&mut self, now_ns: u64) {
        self.show(now_ns);
        self.muted = !self.muted;
    }

    /// The pointer moved to (`x`, `y`): over the shown overlay it keeps it up and selects
    /// the meter under it, as lazer's `OnMouseMove` and `OnHover`.
    pub fn pointer(&mut self, x: f32, y: f32, layout: &VolumeLayout, now_ns: u64) {
        self.pointer_known = true;
        if !self.visible {
            self.hovered = false;
            return;
        }
        let over = layout.bounds.contains(x, y);
        if over || self.hovered {
            // Movement over it, or leaving it, starts the wait to hide again.
            self.active_ns = now_ns;
        }
        self.hovered = over;
        if let Some(meter) = layout.meter_at(x, y) {
            self.select(meter);
        }
    }

    /// The pointer rests at (`x`, `y`): whether it is over the shown overlay, which then
    /// does not hide, as lazer's `IsHovered`, checked every tick without movement. It
    /// selects nothing, so the meter keys keep their choice under a resting pointer.
    pub fn hover(&mut self, x: f32, y: f32, layout: &VolumeLayout, now_ns: u64) {
        let over = self.visible && self.pointer_known && layout.bounds.contains(x, y);
        if self.hovered && !over {
            // Leaving starts the wait to hide again.
            self.active_ns = now_ns;
        }
        self.hovered = over;
    }

    /// Hides the overlay once it waited long enough with the pointer away.
    pub fn tick(&mut self, now_ns: u64) {
        if self.visible && !self.hovered && now_ns.saturating_sub(self.active_ns) >= HIDE_AFTER_NS {
            self.visible = false;
            self.since_ns = now_ns;
            if self.changed {
                self.changed = false;
                self.save_due = true;
            }
        }
    }

    /// Whether the overlay hid after a change since the last call: then the settings are
    /// to be saved.
    pub fn take_save(&mut self) -> bool {
        std::mem::take(&mut self.save_due)
    }

    /// Whether a change is not saved yet, shown or hidden; forgets it.
    pub fn take_unsaved(&mut self) -> bool {
        let unsaved = self.changed || self.save_due;
        self.changed = false;
        self.save_due = false;
        unsaved
    }

    /// The frame for the draw side at `edge` in `style` with `levels`.
    pub fn frame(&self, edge: Edge, style: MeterStyle, levels: &Levels) -> VolumeFrame {
        VolumeFrame {
            edge,
            style,
            visible: self.visible,
            since_ns: self.since_ns,
            selected: self.selected,
            levels: levels.map(|l| l as f32),
            muted: self.muted,
            bursts: self.bursts,
        }
    }

    fn select(&mut self, meter: Meter) {
        self.selected = meter;
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Volume/VolumeMeter.cs
    fn change(
        &mut self,
        levels: &mut Levels,
        meter: Meter,
        amount: f64,
        precise: bool,
        now_ns: u64,
    ) -> bool {
        if amount == 0.0 || !amount.is_finite() {
            return false;
        }
        let i = meter.index();
        // Every change speeds the meter's next up to a cutoff; a pause resets it.
        if self.last_adjust_ns[i].is_none_or(|t| now_ns.saturating_sub(t) >= ACCELERATION_RESET_NS)
        {
            self.acceleration[i] = 1.0;
        }
        self.last_adjust_ns[i] = Some(now_ns);
        let delta = amount * self.acceleration[i];
        self.acceleration[i] =
            (self.acceleration[i] * ACCELERATION_MULTIPLIER).min(MAX_ACCELERATION);
        let old = levels[i];
        let mut level = old;
        if precise {
            let accumulation = &mut self.accumulation[i];
            *accumulation += delta * STEP;
            // `Precision.AlmostBigger(|accumulation|, precision)`.
            while accumulation.abs() > STEP - ALMOST {
                level = rounded(level + accumulation.signum() * STEP);
                *accumulation = if *accumulation < 0.0 {
                    (*accumulation + STEP).min(0.0)
                } else {
                    (*accumulation - STEP).max(0.0)
                };
            }
        } else {
            level = rounded(level + delta.signum() * STEP.max((delta * STEP).abs()));
        }
        levels[i] = level;
        if level == old {
            return false;
        }
        self.changed = true;
        self.burst(meter, old, level, now_ns);
        true
    }

    fn burst(&mut self, meter: Meter, old: f64, level: f64, now_ns: u64) {
        // A step of 5 % or more is a full burst; hitting either end is one too.
        let at_end = level <= 0.0 || level >= 1.0;
        let strength = if at_end {
            1.0
        } else {
            ((level - old).abs() / 0.05).clamp(0.25, 1.0) as f32
        };
        self.seed = self.seed.wrapping_add(0x9e37_79b9);
        self.bursts[self.next_burst] = Burst {
            // 0 marks an empty slot.
            at_ns: now_ns.max(1),
            meter,
            level: level as f32,
            strength,
            seed: self.seed,
        };
        self.next_burst = (self.next_burst + 1) % MAX_BURSTS;
    }
}

/// `level` into `0 ..= 1` and to the precision of a step, as a `BindableDouble` with
/// `Precision = 0.01` (clamped, then rounded half to even in decimal, as
/// `BindableNumber.setValue`): the nearest double to a whole number of hundredths, so that
/// 0.7 stays 0.7 in the settings file.
fn rounded(level: f64) -> f64 {
    (level.clamp(0.0, 1.0) * 100.0).round_ties_even() / 100.0
}

/// How a meter shows its level (`general.volume_meter`, ADR-0145).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MeterStyle {
    /// A ring, filled clockwise from the top, the value in the middle (after lazer's).
    #[default]
    Arc,
    /// A column filling upwards, as a mixing desk's fader.
    Column,
    /// A bar filling rightwards under the label and the value.
    Bar,
}

impl MeterStyle {
    pub const ALL: [MeterStyle; 3] = [MeterStyle::Arc, MeterStyle::Column, MeterStyle::Bar];

    /// The name in settings files.
    pub fn name(self) -> &'static str {
        match self {
            MeterStyle::Arc => "arc",
            MeterStyle::Column => "column",
            MeterStyle::Bar => "bar",
        }
    }

    pub fn from_name(name: &str) -> Option<MeterStyle> {
        MeterStyle::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The localization key of the option's name.
    pub fn label_key(self) -> &'static str {
        match self {
            MeterStyle::Arc => "settings-general-volume-meter-arc",
            MeterStyle::Column => "settings-general-volume-meter-column",
            MeterStyle::Bar => "settings-general-volume-meter-bar",
        }
    }
}

/// Where a meter's parts are, fully shown, unscaled by selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterParts {
    /// The card behind the meter.
    pub card: Rect,
    /// The gauge: the ring's square, the column or the bar.
    pub gauge: Rect,
    /// Where the value is: centred in it, or right-aligned for a bar.
    pub value: Rect,
    /// Where the label is: centred in it, or left-aligned for a bar.
    pub label: Rect,
    /// Size of the value's text.
    pub value_size: f32,
}

/// Where the meters are, fully shown: a pure function of the edge, the style, the window
/// size and the theme, shared by hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeLayout {
    pub edge: Edge,
    pub style: MeterStyle,
    pub scale: f32,
    /// The meters, in [`Meter::ALL`] order.
    pub meters: [MeterParts; METERS],
    /// The strip along the edge the pointer keeps the overlay up in.
    pub bounds: Rect,
    /// The shade's strip.
    pub backdrop: Rect,
}

const NO_RECT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 0.0,
    height: 0.0,
};

impl VolumeLayout {
    pub fn new(
        edge: Edge,
        style: MeterStyle,
        width: f32,
        height: f32,
        theme: &Theme,
    ) -> VolumeLayout {
        let scale = theme.scale(height);
        let pad = theme.volume_card_pad * scale;
        let gap = theme.volume_gap * scale;
        let margin = theme.volume_margin * scale;
        let label_h = theme.volume_label_size * scale * 1.6;
        // Each meter's parts relative to its card's top-left corner, and the card's size.
        let parts = Meter::ALL.map(|m| {
            let big = m == Meter::Master;
            let ring = scale
                * if big {
                    theme.volume_ring_master
                } else {
                    theme.volume_ring
                };
            let small_value = theme.volume_label_size * scale * 1.5;
            match style {
                MeterStyle::Arc => {
                    let w = ring + 2.0 * pad;
                    let gauge = Rect {
                        x: pad,
                        y: pad,
                        width: ring,
                        height: ring,
                    };
                    MeterParts {
                        card: Rect {
                            width: w,
                            height: ring + 2.0 * pad + label_h,
                            ..NO_RECT
                        },
                        gauge,
                        value: gauge,
                        label: Rect {
                            x: 0.0,
                            y: pad + ring,
                            width: w,
                            height: label_h,
                        },
                        value_size: theme.volume_value_size * ring,
                    }
                }
                MeterStyle::Column => {
                    let column_h = ring * 1.1;
                    let column_w = ring * theme.volume_column;
                    let w = ring * 0.85 + 2.0 * pad;
                    let value_h = small_value * 1.3;
                    MeterParts {
                        card: Rect {
                            width: w,
                            height: 2.0 * pad + value_h + column_h + label_h + gap,
                            ..NO_RECT
                        },
                        value: Rect {
                            x: 0.0,
                            y: pad,
                            width: w,
                            height: value_h,
                        },
                        gauge: Rect {
                            x: (w - column_w) / 2.0,
                            y: pad + value_h + gap / 2.0,
                            width: column_w,
                            height: column_h,
                        },
                        label: Rect {
                            x: 0.0,
                            y: pad + value_h + gap + column_h,
                            width: w,
                            height: label_h,
                        },
                        value_size: small_value,
                    }
                }
                MeterStyle::Bar => {
                    let w = ring * 2.4 + 2.0 * pad;
                    let bar_h = ring * theme.volume_bar;
                    let row = Rect {
                        x: pad,
                        y: pad,
                        width: w - 2.0 * pad,
                        height: label_h.max(small_value * 1.2),
                    };
                    MeterParts {
                        card: Rect {
                            width: w,
                            height: 2.0 * pad + row.height + gap / 2.0 + bar_h,
                            ..NO_RECT
                        },
                        value: row,
                        label: row,
                        gauge: Rect {
                            x: pad,
                            y: pad + row.height + gap / 2.0,
                            width: w - 2.0 * pad,
                            height: bar_h,
                        },
                        value_size: small_value,
                    }
                }
            }
        });
        let vertical = edge.vertical();
        let size = |p: &MeterParts| (p.card.width, p.card.height);
        let along: f32 = parts
            .iter()
            .map(|p| if vertical { size(p).1 } else { size(p).0 })
            .sum::<f32>()
            + gap * (METERS - 1) as f32;
        let depth = parts
            .iter()
            .map(|p| if vertical { size(p).0 } else { size(p).1 })
            .fold(0.0, f32::max);
        let length = if vertical { height } else { width };
        let mut at = (length - along) / 2.0;
        let meters = parts.map(|p| {
            let (w, h) = size(&p);
            let origin = match edge {
                Edge::Left => [margin, at],
                Edge::Right => [width - margin - w, at],
                Edge::Top => [at, margin],
                Edge::Bottom => [at, height - margin - h],
            };
            at += if vertical { h } else { w } + gap;
            let moved = |r: Rect| Rect {
                x: r.x + origin[0],
                y: r.y + origin[1],
                ..r
            };
            MeterParts {
                card: moved(p.card),
                gauge: moved(p.gauge),
                value: moved(p.value),
                label: moved(p.label),
                value_size: p.value_size,
            }
        });
        let strip = |depth: f32| match edge {
            Edge::Left => Rect {
                x: 0.0,
                y: 0.0,
                width: depth,
                height,
            },
            Edge::Right => Rect {
                x: width - depth,
                y: 0.0,
                width: depth,
                height,
            },
            Edge::Top => Rect {
                x: 0.0,
                y: 0.0,
                width,
                height: depth,
            },
            Edge::Bottom => Rect {
                x: 0.0,
                y: height - depth,
                width,
                height: depth,
            },
        };
        VolumeLayout {
            edge,
            style,
            scale,
            meters,
            bounds: strip(depth + 2.0 * margin),
            backdrop: strip((theme.volume_backdrop_depth * scale).max(depth + 2.0 * margin)),
        }
    }

    /// The meter whose card contains (`x`, `y`).
    pub fn meter_at(&self, x: f32, y: f32) -> Option<Meter> {
        Meter::ALL
            .into_iter()
            .find(|m| self.meters[m.index()].card.contains(x, y))
    }

    /// How far the overlay moves out of the window to hide.
    fn travel(&self) -> f32 {
        if self.edge.vertical() {
            self.bounds.width
        } else {
            self.bounds.height
        }
    }

    /// The shift towards the window that `hidden` (0 shown, 1 hidden) makes.
    fn shift(&self, hidden: f32) -> [f32; 2] {
        let d = self.travel() * hidden;
        match self.edge {
            Edge::Left => [-d, 0.0],
            Edge::Right => [d, 0.0],
            Edge::Top => [0.0, -d],
            Edge::Bottom => [0.0, d],
        }
    }
}

/// The overlay's localised texts.
#[derive(Clone, Debug, Default)]
pub struct VolumeLabels {
    /// In [`Meter::ALL`] order.
    pub meters: [String; METERS],
    /// Shown for a full level.
    pub max: String,
    /// Shown on the master meter while muted.
    pub muted: String,
}

/// Animation towards a target value.
#[derive(Clone, Copy, Debug, Default)]
struct Towards {
    from: f32,
    to: f32,
    since_ns: u64,
}

impl Towards {
    fn at(&self, now_ns: u64, duration_ns: u64, ease: fn(f32) -> f32) -> f32 {
        let t = ease(progress(self.since_ns, now_ns, duration_ns));
        self.from + (self.to - self.from) * t
    }

    fn aim(&mut self, to: f32, now_ns: u64, duration_ns: u64, ease: fn(f32) -> f32) {
        if self.to != to {
            self.from = self.at(now_ns, duration_ns, ease);
            self.to = to;
            self.since_ns = now_ns;
        }
    }

    fn rest(value: f32, now_ns: u64) -> Towards {
        Towards {
            from: value,
            to: value,
            since_ns: now_ns,
        }
    }
}

/// What a value line shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shown {
    Percent(u8),
    Max,
    Muted,
}

/// A point of the meter as drawn: grown by its selection about the card's centre and
/// shifted by the slide.
#[derive(Clone, Copy)]
struct Placement {
    centre: [f32; 2],
    grow: f32,
    shift: [f32; 2],
}

impl Placement {
    fn point(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [
            self.centre[0] + (x - self.centre[0]) * self.grow + self.shift[0],
            self.centre[1] + (y - self.centre[1]) * self.grow + self.shift[1],
        ]
    }

    fn rect(&self, r: Rect) -> Rect {
        let [x, y] = self.point([r.x, r.y]);
        Rect {
            x,
            y,
            width: r.width * self.grow,
            height: r.height * self.grow,
        }
    }
}

/// The draw side of the overlay. Texts are laid out when they or the scale change.
pub struct VolumeView {
    theme: Theme,
    type_setter: Typesetter,
    scale: f32,
    labels: [Line; METERS],
    values: [Line; METERS],
    /// What each value line shows, at which size.
    shown: [Option<(Shown, f32)>; METERS],
    max: String,
    muted: String,
    /// Every glyph a value can show, so that they are asked for up front.
    charset: Line,
    level: [Towards; METERS],
    emphasis: [Towards; METERS],
    /// A fresh view starts at the first frame's levels without animating.
    started: bool,
}

impl std::fmt::Debug for VolumeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolumeView").finish_non_exhaustive()
    }
}

impl VolumeView {
    /// Lays the texts out once: call it off the hot paths.
    pub fn new(registry: Arc<FontRegistry>, theme: Theme, labels: &VolumeLabels) -> VolumeView {
        let values = std::array::from_fn(|_| {
            let mut line = Line::new("");
            line.text
                .reserve(8 + labels.max.len().max(labels.muted.len()));
            line
        });
        let mut charset = Line::new("0123456789");
        charset.text.push_str(&labels.max);
        charset.text.push_str(&labels.muted);
        let mut view = VolumeView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            scale: 0.0,
            labels: std::array::from_fn(|i| Line::new(&labels.meters[i])),
            values,
            shown: [None; METERS],
            max: labels.max.clone(),
            muted: labels.muted.clone(),
            charset,
            level: [Towards::default(); METERS],
            emphasis: [Towards::default(); METERS],
            started: false,
            theme,
        };
        view.lay_out(1.0);
        view
    }

    /// Takes the theme's new colours and effects.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Lays the texts out for a window `height` pixels high ahead of its first frame there,
    /// with the longest value a meter can show, so that the frames do not allocate.
    /// Allocates: call it off the hot paths.
    pub fn lay_out_for(&mut self, _width: f32, height: f32) {
        self.lay_out(self.theme.scale(height));
        let scale = self.scale;
        let longest = [self.max.len(), self.muted.len(), 3]
            .into_iter()
            .max()
            .unwrap_or(3);
        let size = self.theme.volume_value_size * self.theme.volume_ring_master * scale;
        for line in &mut self.values {
            line.text.clear();
            line.text.reserve(longest);
            for _ in 0..longest {
                line.text.push('0');
            }
            self.type_setter.lay_out(line, size);
        }
        self.shown = [None; METERS];
    }

    /// Asks `glyphs` for every glyph the overlay can show. Returns how many are not ready
    /// yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.labels
            .iter()
            .chain(std::iter::once(&self.charset))
            .flat_map(|l| l.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    fn lay_out(&mut self, scale: f32) {
        self.scale = scale;
        let label = self.theme.volume_label_size * scale;
        for line in &mut self.labels {
            self.type_setter.lay_out(line, label);
        }
        let value = self.theme.volume_value_size * self.theme.volume_ring * scale;
        self.type_setter.lay_out(&mut self.charset, value);
        self.shown = [None; METERS];
    }

    fn update_value(&mut self, meter: Meter, shown: Shown, size: f32) {
        let i = meter.index();
        if self.shown[i] == Some((shown, size)) {
            return;
        }
        self.shown[i] = Some((shown, size));
        let line = &mut self.values[i];
        line.text.clear();
        match shown {
            Shown::Percent(p) => {
                let _ = write!(line.text, "{p}");
            }
            Shown::Max => line.text.push_str(&self.max),
            Shown::Muted => line.text.push_str(&self.muted),
        }
        self.type_setter.lay_out(line, size);
    }

    /// Writes the overlay into `shapes` and its texts into `text` at `now_ns`. Draws
    /// nothing once hidden. Allocation-free once warm and once the batches have room.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        frame: &VolumeFrame,
        width: f32,
        height: f32,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let ms = |ms: f32| (ms.max(0.0) as f64 * 1e6) as u64;
        let value_ns = ms(self.theme.volume_value_ms);
        let select_ns = ms(self.theme.volume_select_ms);
        for meter in Meter::ALL {
            let i = meter.index();
            let target = frame.levels[i].clamp(0.0, 1.0);
            let selected = if frame.selected == meter { 1.0 } else { 0.0 };
            if self.started {
                self.level[i].aim(target, now_ns, value_ns, ease_out_quint);
                self.emphasis[i].aim(selected, now_ns, select_ns, ease_out_expo);
            } else {
                self.level[i] = Towards::rest(target, now_ns);
                self.emphasis[i] = Towards::rest(selected, now_ns);
            }
        }
        self.started = true;
        // In it decelerates onto its place, out it eases away; it never passes its place,
        // so the shade stays on the edge.
        let shown = if frame.visible {
            ease_out_quint(progress(
                frame.since_ns,
                now_ns,
                ms(self.theme.volume_in_ms),
            ))
        } else {
            let t = progress(frame.since_ns, now_ns, ms(self.theme.volume_out_ms));
            if t >= 1.0 {
                return;
            }
            1.0 - ease_out_cubic(t)
        };
        let fade = shown;
        let theme = self.theme;
        let layout = VolumeLayout::new(frame.edge, frame.style, width, height, &theme);
        if layout.scale != self.scale {
            self.lay_out(layout.scale);
        }
        let s = layout.scale;
        let shift = layout.shift(1.0 - shown);
        self.push_backdrop(shapes, white, &layout, shift, fade);
        let pulse = 0.65
            + 0.35
                * ((now_ns as f64 / 1e9 * MAX_PULSE_HZ).fract() * std::f64::consts::TAU).sin()
                    as f32;
        for meter in Meter::ALL {
            let i = meter.index();
            let parts = layout.meters[i];
            let e = self.emphasis[i].at(now_ns, select_ns, ease_out_expo);
            let place = Placement {
                centre: parts.card.centre(),
                grow: 1.0 + (theme.volume_select_scale - 1.0) * e,
                shift,
            };
            let card = place.rect(parts.card);
            let tint = theme.volume_tints[i];
            let muted = frame.muted && meter == Meter::Master;
            let lit = if muted {
                Color {
                    a: tint.a,
                    ..Color::rgb(0.55, 0.55, 0.58)
                }
            } else {
                tint
            };
            // The card.
            let panel = Shape {
                radius: theme.panel_radius * s,
                ..Shape::new(
                    white.texture,
                    white.uv,
                    [card.x, card.y],
                    [card.width, card.height],
                    faded(theme.button_fill, fade),
                )
            };
            if theme.effects {
                push_shadow(shapes, &panel, &theme, s, fade);
            }
            shapes.push_shape(&panel);
            if e > 0.01 {
                shapes.push_shape(&Shape {
                    outline: theme.outline_width * s,
                    colour: faded(lit, fade * e),
                    ..panel
                });
            }
            let level = self.level[i].at(now_ns, value_ns, ease_out_quint);
            let full = level >= MAX_SHOWN && !muted;
            let glow = if theme.effects {
                if full { pulse } else { 0.45 + 0.25 * e }
            } else {
                0.0
            };
            let gauge = place.rect(parts.gauge);
            push_gauge(
                shapes,
                white,
                &theme,
                layout.style,
                &gauge,
                level,
                lit,
                glow,
                full,
                fade,
            );
            // The value and the label.
            let shown_value = if muted {
                Shown::Muted
            } else if level >= MAX_SHOWN {
                Shown::Max
            } else {
                Shown::Percent((level * 100.0).round_ties_even().clamp(0.0, 100.0) as u8)
            };
            self.update_value(meter, shown_value, parts.value_size);
            let value_colour = faded(theme.text, fade);
            let label_colour = faded(theme.text, fade * (0.7 + 0.3 * e));
            let value_box = place.rect(parts.value);
            let label_box = place.rect(parts.label);
            if layout.style == MeterStyle::Bar {
                push_right(text, &self.values[i], &value_box, 0.0, value_colour, glyphs);
                push_left(text, &self.labels[i], &label_box, 0.0, label_colour, glyphs);
            } else {
                push_centred(
                    text,
                    &self.values[i],
                    value_box.centre(),
                    value_colour,
                    glyphs,
                );
                push_centred(
                    text,
                    &self.labels[i],
                    label_box.centre(),
                    label_colour,
                    glyphs,
                );
            }
        }
        if theme.effects {
            self.push_sparks(shapes, white, frame, &layout, shift, fade, now_ns);
        }
    }

    fn push_backdrop(
        &self,
        shapes: &mut ShapeBatch,
        white: AtlasRegion,
        layout: &VolumeLayout,
        [dx, dy]: [f32; 2],
        fade: f32,
    ) {
        let solid = faded(self.theme.volume_backdrop, fade);
        let clear = Color { a: 0.0, ..solid };
        // The shade is strongest at the edge.
        let (from, to, gradient): (Color, Color, fn(Color) -> Gradient) = match layout.edge {
            Edge::Left => (solid, clear, Gradient::Horizontal),
            Edge::Right => (clear, solid, Gradient::Horizontal),
            Edge::Top => (solid, clear, Gradient::Vertical),
            Edge::Bottom => (clear, solid, Gradient::Vertical),
        };
        let b = layout.backdrop;
        shapes.push_shape(&Shape {
            gradient: Some(gradient(to)),
            ..Shape::new(
                white.texture,
                white.uv,
                [b.x + dx, b.y + dy],
                [b.width, b.height],
                from,
            )
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_sparks(
        &self,
        shapes: &mut ShapeBatch,
        white: AtlasRegion,
        frame: &VolumeFrame,
        layout: &VolumeLayout,
        shift: [f32; 2],
        fade: f32,
        now_ns: u64,
    ) {
        let theme = &self.theme;
        let s = layout.scale;
        let life_ms = theme.volume_spark_ms.max(1.0);
        let most = (theme.volume_sparks as usize).min(MAX_SPARKS);
        if most == 0 {
            return;
        }
        for burst in &frame.bursts {
            if burst.at_ns == 0 || now_ns < burst.at_ns {
                continue;
            }
            let age_ms = ((now_ns - burst.at_ns) as f64 / 1e6) as f32;
            if age_ms >= life_ms {
                continue;
            }
            let i = burst.meter.index();
            let gauge = layout.meters[i].gauge;
            let gauge = Rect {
                x: gauge.x + shift[0],
                y: gauge.y + shift[1],
                ..gauge
            };
            let (origin, angle) = gauge_tip(layout.style, &gauge, burst.level.clamp(0.0, 1.0));
            let tint = theme.volume_tints[i];
            let count = ((most as f32 * burst.strength).round() as usize).clamp(most.min(2), most);
            for n in 0..count {
                let r = |k: u32| unit(hash(burst.seed ^ (n as u32).wrapping_mul(0x85eb_ca6b) ^ k));
                let life = life_ms * (0.5 + 0.5 * r(1));
                if age_ms >= life {
                    continue;
                }
                let t = age_ms / life;
                // Away from the gauge's tip, spread around its direction.
                let dir = angle + (r(2) - 0.5) * 1.6;
                let speed = theme.volume_spark_speed
                    * s
                    * (0.35 + 0.65 * r(3))
                    * (0.5 + 0.5 * burst.strength);
                let dist = speed * life / 1000.0 * ease_out_cubic(t);
                let size = theme.volume_spark_size * s * (0.5 + 0.5 * r(4)) * (1.0 - 0.6 * t);
                let at = [
                    origin[0] + dist * dir.cos() - size / 2.0,
                    origin[1] - dist * dir.sin() - size / 2.0,
                ];
                let alpha = (1.0 - t) * (1.0 - t) * fade;
                let spark = Shape {
                    radius: size / 2.0,
                    blend: Blend::Additive,
                    ..Shape::new(
                        white.texture,
                        white.uv,
                        at,
                        [size, size],
                        faded(tint, alpha),
                    )
                };
                shapes.push_shape(&Shape {
                    glow: size * 1.2,
                    ..spark
                });
                shapes.push_shape(&spark);
            }
        }
    }
}

/// The point a gauge's fill ends at for `level`, and the direction sparks leave it in
/// (radians, counter-clockwise from the right, y up).
fn gauge_tip(style: MeterStyle, gauge: &Rect, level: f32) -> ([f32; 2], f32) {
    match style {
        MeterStyle::Arc => {
            let [cx, cy] = gauge.centre();
            let radius = gauge.width / 2.0;
            let angle = ARC_START - ARC_SWEEP * level;
            (
                [cx + radius * angle.cos(), cy - radius * angle.sin()],
                angle,
            )
        }
        MeterStyle::Column => (
            [
                gauge.x + gauge.width / 2.0,
                gauge.y + gauge.height * (1.0 - level),
            ],
            std::f32::consts::FRAC_PI_2,
        ),
        MeterStyle::Bar => (
            [gauge.x + gauge.width * level, gauge.y + gauge.height / 2.0],
            0.0,
        ),
    }
}

/// Draws a gauge of `style` in `gauge` filled to `level` in `lit`: a dim track, the fill
/// and, with `glow` above 0, its glow; `full` lights the whole of it.
#[allow(clippy::too_many_arguments)]
fn push_gauge(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    theme: &Theme,
    style: MeterStyle,
    gauge: &Rect,
    level: f32,
    lit: Color,
    glow: f32,
    full: bool,
    fade: f32,
) {
    let track = faded(
        Color {
            a: theme.volume_track_alpha,
            ..lit
        },
        fade,
    );
    let shape = |r: Rect, radius: f32, colour: Color| Shape {
        radius,
        ..Shape::new(
            white.texture,
            white.uv,
            [r.x, r.y],
            [r.width, r.height],
            colour,
        )
    };
    match style {
        MeterStyle::Arc => {
            let d = gauge.width;
            let stroke = d * theme.volume_stroke;
            // The track: a whole ring, outlined.
            let ring = Shape {
                outline: stroke,
                ..shape(*gauge, d / 2.0, track)
            };
            shapes.push_shape(&ring);
            if full && glow > 0.0 {
                shapes.push_shape(&Shape {
                    outline: 0.0,
                    glow: stroke * 1.6,
                    blend: Blend::Additive,
                    colour: faded(lit, fade * glow * 0.7),
                    ..ring
                });
            }
            if level <= 0.0 {
                return;
            }
            // The fill: round dots close enough to read as one stroke with round ends,
            // opaque over the card so that their overlaps do not show.
            let [cx, cy] = gauge.centre();
            let radius = d / 2.0 - stroke / 2.0;
            let length = ARC_SWEEP * radius * level;
            let steps = ((length / (stroke * 0.3)).ceil() as usize).clamp(1, ARC_DOTS - 1);
            let solid = Color { a: 1.0, ..lit };
            for k in 0..=steps {
                let angle = ARC_START - ARC_SWEEP * level * k as f32 / steps as f32;
                let at = [
                    cx + radius * angle.cos() - stroke / 2.0,
                    cy - radius * angle.sin() - stroke / 2.0,
                ];
                shapes.push_shape(&Shape {
                    radius: stroke / 2.0,
                    ..Shape::new(
                        white.texture,
                        white.uv,
                        at,
                        [stroke, stroke],
                        faded(solid, fade),
                    )
                });
            }
        }
        MeterStyle::Column | MeterStyle::Bar => {
            let vertical = style == MeterStyle::Column;
            let round = gauge.width.min(gauge.height) / 2.0;
            shapes.push_shape(&shape(*gauge, round, track));
            if level <= 0.0 {
                return;
            }
            // Never thinner than its rounding, so the ends stay round.
            let fill = if vertical {
                let h = (gauge.height * level).max(gauge.width);
                Rect {
                    y: gauge.y + gauge.height - h,
                    height: h,
                    ..*gauge
                }
            } else {
                Rect {
                    width: (gauge.width * level).max(gauge.height),
                    ..*gauge
                }
            };
            let bright = Color {
                r: lit.r + (1.0 - lit.r) * 0.35,
                g: lit.g + (1.0 - lit.g) * 0.35,
                b: lit.b + (1.0 - lit.b) * 0.35,
                a: lit.a,
            };
            // Brighter towards the tip.
            let (from, to, gradient): (Color, Color, fn(Color) -> Gradient) = if vertical {
                (bright, lit, Gradient::Vertical)
            } else {
                (lit, bright, Gradient::Horizontal)
            };
            let filled = Shape {
                gradient: Some(gradient(faded(to, fade))),
                ..shape(fill, round, faded(from, fade))
            };
            if glow > 0.0 {
                shapes.push_shape(&Shape {
                    glow: round * 2.5,
                    gradient: None,
                    blend: Blend::Additive,
                    colour: faded(lit, fade * glow * if full { 1.0 } else { 0.6 }),
                    ..filled
                });
            }
            shapes.push_shape(&filled);
        }
    }
}

/// A well-mixed hash of `x` (the finaliser of MurmurHash3).
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// `x` as a number in `0 .. 1`.
fn unit(x: u32) -> f32 {
    (x >> 8) as f32 / (1u32 << 24) as f32
}

#[cfg(test)]
mod tests;
