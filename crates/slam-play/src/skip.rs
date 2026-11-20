//! The skip of an intro or break (ADR-0160): a short dip of the picture and the music
//! around the seek, and the skip button's styles.

/// How long the picture and the music fade out before the seek.
pub const DIP_OUT_NS: u64 = 100_000_000;
/// How long they fade back in after it.
pub const DIP_IN_NS: u64 = 250_000_000;
/// A skip whose target is nearer than this, in game milliseconds, seeks at once: the dip
/// would end past the target even at double speed.
pub const DIP_MIN_LEAD_MS: f64 = 300.0;

/// How dark the picture is `since_ns` after the skip was pressed, 0..=1: OutQuad down to
/// black until the seek, then back linearly.
pub fn dip_alpha(since_ns: u64) -> f32 {
    if since_ns < DIP_OUT_NS {
        let t = since_ns as f64 / DIP_OUT_NS as f64;
        (t * (2.0 - t)) as f32
    } else if since_ns < DIP_OUT_NS + DIP_IN_NS {
        let t = (since_ns - DIP_OUT_NS) as f64 / DIP_IN_NS as f64;
        (1.0 - t) as f32
    } else {
        0.0
    }
}

/// How the skip button looks (`gameplay.skip_button`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkipStyle {
    /// The skin's `play-skip` where the HUD layout puts it, as stable has it; without the
    /// image, [`SkipStyle::Lazer`].
    #[default]
    Skin,
    /// Lazer's `SkipOverlay`: a bar across the lower middle of the screen.
    Lazer,
    /// A plate in SLAM's interface style where the HUD layout puts it.
    Slam,
}

/// A skip under way on the update side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Dip {
    /// Engine time the skip was pressed.
    pub started_ns: u64,
    /// Where the seek goes, in game time.
    pub target: f64,
    /// The seek has been asked for.
    pub seeked: bool,
}

impl Dip {
    /// Whether the seek is due at `now_ns` and not asked for yet.
    pub fn seek_due(&self, now_ns: u64) -> bool {
        !self.seeked && now_ns.saturating_sub(self.started_ns) >= DIP_OUT_NS
    }

    /// Whether the dip is over at `now_ns`.
    pub fn is_over(&self, now_ns: u64) -> bool {
        self.seeked && now_ns.saturating_sub(self.started_ns) >= DIP_OUT_NS + DIP_IN_NS
    }
}

// ---- The button's look ----

use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_ui::menu::Rect;
use slam_ui::theme::Theme;
use slam_ui::widgets::{Look, draw};

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SkipOverlay.cs (fade_time)
/// How long the button fades in and out, ms.
pub const FADE_MS: f64 = 300.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SkipOverlay.cs (OnHover, OnClick)
const HOVER_MS: f64 = 500.0;
const FLASH_MS: f64 = 500.0;
const CLICK_SCALE_MS: f64 = 2000.0;
const CLICK_SCALE: f32 = 1.2;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs (Orange3)
/// `#cca633`.
const ORANGE3: Color = Color::rgb(0.8, 166.0 / 255.0, 0.2);

/// A value moving to a target over a duration, by HUD time (ms).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fade {
    from: f32,
    to: f32,
    since: f64,
    duration: f64,
    out_quint: bool,
}

impl Fade {
    fn at_rest(value: f32, duration: f64, out_quint: bool) -> Self {
        Fade {
            from: value,
            to: value,
            since: f64::NEG_INFINITY,
            duration,
            out_quint,
        }
    }

    /// The value at `time`.
    pub fn value(&self, time: f64) -> f32 {
        let t = ((time - self.since) / self.duration).clamp(0.0, 1.0);
        let t = if self.out_quint {
            let u = t - 1.0;
            u * u * u * u * u + 1.0
        } else {
            t
        };
        self.from + (self.to - self.from) * t as f32
    }

    /// Heads for `to` from where it is at `time`.
    pub fn set(&mut self, to: f32, time: f64) {
        if to != self.to {
            self.from = self.value(time);
            self.to = to;
            self.since = time;
        }
    }
}

/// The skip button's animations on the draw side: shown or not, hovered or not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkipAnimation {
    shown: Fade,
    hover: Fade,
}

impl Default for SkipAnimation {
    fn default() -> Self {
        SkipAnimation {
            shown: Fade::at_rest(0.0, FADE_MS, false),
            hover: Fade::at_rest(0.0, HOVER_MS, true),
        }
    }
}

/// How the skip button looks at one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkipPose {
    /// 0..=1: the button fades in and out (lazer's `ButtonContainer`).
    pub alpha: f32,
    /// 0..=1: how far the hover has gone.
    pub hover: f32,
    /// 0..=1 after a click: the white flash, 1 at the click.
    pub flash: f32,
    /// The click's growth, 1 at rest.
    pub scale: f32,
}

impl SkipAnimation {
    /// Takes the frame's state at HUD time `time` (ms).
    pub fn update(&mut self, available: bool, hovered: bool, time: f64) {
        self.shown.set(if available { 1.0 } else { 0.0 }, time);
        self.hover.set(if hovered { 1.0 } else { 0.0 }, time);
    }

    /// The pose at `time`, `clicked` ms after the click if there was one.
    pub fn pose(&self, time: f64, clicked: Option<f64>) -> SkipPose {
        let (flash, scale) = match clicked {
            Some(since) => {
                let out_quint = |d: f64| {
                    let u = (since / d).clamp(0.0, 1.0) - 1.0;
                    (u * u * u * u * u + 1.0) as f32
                };
                (
                    1.0 - out_quint(FLASH_MS),
                    1.0 + (CLICK_SCALE - 1.0) * out_quint(CLICK_SCALE_MS),
                )
            }
            None => (0.0, 1.0),
        };
        SkipPose {
            alpha: self.shown.value(time),
            hover: self.hover.value(time),
            flash,
            scale,
        }
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SkipOverlay.cs (SkipOverlay)
/// Height of lazer's skip band, lazer pixels; it is centred at 0.7 of the window.
const LAZER_BAND: f32 = 100.0;
const LAZER_BAND_Y: f32 = 0.7;
const LAZER_BAR: f32 = 5.0;
const LAZER_BUTTON: f32 = 0.6 * LAZER_BAND;
const LAZER_CORNER: f32 = 15.0;
const LAZER_CHEVRON: f32 = 15.0;
const LAZER_CHEVRON_SPACING: f32 = 5.0;
/// Where the chevrons and the text are, fractions of the button's height.
pub const LAZER_CHEVRONS_Y: f32 = 0.4;
pub const LAZER_TEXT_Y: f32 = 0.7;
/// The text's size, lazer pixels.
pub const LAZER_TEXT_SIZE: f32 = 12.0;

/// The window box lazer's skip band takes on a window of `width` × `height`, where a
/// click skips: `[left, top, right, bottom]`. `unit` is window pixels per lazer pixel.
pub fn lazer_area(width: f32, height: f32, unit: f32) -> [f32; 4] {
    let centre = height * LAZER_BAND_Y;
    let half = LAZER_BAND * unit / 2.0;
    [0.0, centre - half, width, centre + half]
}

/// Lazer's button: its centre and side in window pixels at the click's `scale`.
pub fn lazer_button(width: f32, height: f32, unit: f32, scale: f32) -> ([f32; 2], f32) {
    (
        [width / 2.0, height * LAZER_BAND_Y],
        LAZER_BUTTON * unit * scale,
    )
}

fn lighten(c: Color, amount: f32) -> Color {
    // Ported from osu-framework 2026.921.1: osu.Framework/Extensions/Color4Extensions/Color4Extensions.cs (Lighten)
    let k = 1.0 + amount;
    Color {
        r: (c.r * k).min(1.0),
        g: (c.g * k).min(1.0),
        b: (c.b * k).min(1.0),
        a: c.a,
    }
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

fn with_alpha(c: Color, alpha: f32) -> Color {
    Color {
        a: c.a * alpha,
        ..c
    }
}

/// Writes lazer's skip overlay: the band, the button with its chevrons and the time left
/// (`progress`, 1 to 0). The text goes with [`lazer_button`]. Allocation-free once `shapes`
/// has room for [`MAX_SKIP_SHAPES`].
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SkipOverlay.cs (load, Button)
pub fn push_lazer(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    window: [f32; 2],
    unit: f32,
    pose: &SkipPose,
    progress: f32,
) {
    if pose.alpha <= 0.0 {
        return;
    }
    let [width, height] = window;
    let [_, top, _, bottom] = lazer_area(width, height, unit);
    let rect = |position, size, colour| Shape::new(white.texture, white.uv, position, size, colour);
    // The band's dark box, darker on hover.
    let shade = 0.2 + 0.2 * pose.hover;
    shapes.push_shape(&rect(
        [0.0, top],
        [width, bottom - top],
        with_alpha(Color::BLACK, shade * pose.alpha),
    ));
    // The button.
    let ([cx, cy], side) = lazer_button(width, height, unit, pose.scale);
    let colour = mix(ORANGE3, lighten(ORANGE3, 0.2), pose.hover);
    let colour = mix(colour, Color::WHITE, pose.flash);
    let mut button = rect(
        [cx - side / 2.0, cy - side / 2.0],
        [side, side],
        with_alpha(colour, pose.alpha),
    );
    button.radius = LAZER_CORNER * unit * pose.scale;
    shapes.push_shape(&button);
    // Three chevrons, spread on hover.
    let chevron = LAZER_CHEVRON * unit * pose.scale;
    let spacing = LAZER_CHEVRON_SPACING * unit * pose.scale * pose.hover;
    let row = 3.0 * chevron + 2.0 * spacing;
    let y = cy - side / 2.0 + side * LAZER_CHEVRONS_Y;
    for i in 0..3 {
        let x = cx - row / 2.0 + i as f32 * (chevron + spacing);
        push_chevron(
            shapes,
            white,
            [x, y],
            chevron,
            with_alpha(Color::WHITE, pose.alpha),
        );
    }
    // The time left, a bar along the band's bottom from the middle.
    let bar = LAZER_BAR * unit;
    let bar_width = width * progress.clamp(0.0, 1.0);
    if bar_width > 0.0 {
        let mut time_left = rect(
            [(width - bar_width) / 2.0, bottom - bar],
            [bar_width, bar],
            with_alpha(ORANGE3, pose.alpha),
        );
        time_left.radius = bar / 2.0;
        shapes.push_shape(&time_left);
    }
}

/// A `›` of side `size` whose box starts at `x` and is centred on `y`: two strokes leaning
/// opposite ways.
fn push_chevron(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    [x, y]: [f32; 2],
    size: f32,
    colour: Color,
) {
    let stroke = size * 0.22;
    let half = size * 0.45;
    let lean = (size * 0.4 / half).atan();
    for (top, skew) in [(y - half, -lean), (y, lean)] {
        let mut arm = Shape::new(
            white.texture,
            white.uv,
            [x + size * 0.3 - stroke / 2.0, top],
            [stroke, half],
            colour,
        );
        arm.skew = skew;
        shapes.push_shape(&arm);
    }
}

/// Shapes [`push_lazer`] and [`push_slam`] write at most: generous for the theme's layers.
pub const MAX_SKIP_SHAPES: usize = 32;

/// Writes the skip button in SLAM's style in `rect` (window pixels) with `theme` at `scale`
/// pixels per reference unit: a button plate, lit on hover, flashing on the click. Its text
/// is drawn over it with the HUD's text.
pub fn push_slam(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    theme: &Theme,
    rect: &Rect,
    scale: f32,
    pose: &SkipPose,
) {
    if pose.alpha <= 0.0 {
        return;
    }
    let fill = mix(
        theme.button_fill,
        theme.button_hover_fill,
        pose.hover.max(pose.flash),
    );
    let grown = Rect {
        x: rect.x + rect.width * (1.0 - pose.scale) / 2.0,
        y: rect.y + rect.height * (1.0 - pose.scale) / 2.0,
        width: rect.width * pose.scale,
        height: rect.height * pose.scale,
    };
    let look = Look::Button {
        highlighted: pose.hover > 0.5 || pose.flash > 0.0,
        seed: 0.61,
        fade: pose.alpha,
    };
    draw(
        shapes,
        white,
        &grown,
        with_alpha(fill, pose.alpha),
        look,
        theme,
        scale,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dip_goes_dark_by_the_seek_and_back() {
        assert_eq!(dip_alpha(0), 0.0);
        assert!((dip_alpha(DIP_OUT_NS / 2) - 0.75).abs() < 1e-6);
        assert_eq!(dip_alpha(DIP_OUT_NS), 1.0);
        assert!((dip_alpha(DIP_OUT_NS + DIP_IN_NS / 2) - 0.5).abs() < 1e-6);
        assert_eq!(dip_alpha(DIP_OUT_NS + DIP_IN_NS), 0.0);
    }

    #[test]
    fn the_button_fades_and_flashes_like_lazers() {
        let mut anim = SkipAnimation::default();
        anim.update(true, false, 0.0);
        assert!((anim.pose(150.0, None).alpha - 0.5).abs() < 1e-6);
        assert_eq!(anim.pose(300.0, None).alpha, 1.0);
        anim.update(true, true, 300.0);
        assert_eq!(anim.pose(800.0, None).hover, 1.0);
        // The click: white at once, growing towards 1.2.
        let clicked = anim.pose(800.0, Some(0.0));
        assert_eq!((clicked.flash, clicked.scale), (1.0, 1.0));
        let later = anim.pose(800.0, Some(500.0));
        assert_eq!(later.flash, 0.0);
        assert!(later.scale > 1.0 && later.scale < 1.2);
        // Gone: fades out from where it was.
        anim.update(false, true, 1000.0);
        assert!((anim.pose(1150.0, None).alpha - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lazers_band_is_100_pixels_at_seven_tenths() {
        let area = lazer_area(1024.0, 768.0, 1.0);
        let expected = [0.0, 487.6, 1024.0, 587.6];
        assert!(
            area.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-3),
            "{area:?}"
        );
        let ([x, y], side) = lazer_button(1024.0, 768.0, 1.0, 1.0);
        assert_eq!(x, 512.0);
        assert!((y - 537.6).abs() < 1e-3 && (side - 60.0).abs() < 1e-3);
    }

    #[test]
    fn the_seek_waits_for_the_dark() {
        let dip = Dip {
            started_ns: 1_000,
            target: 5000.0,
            seeked: false,
        };
        assert!(!dip.seek_due(1_000 + DIP_OUT_NS - 1));
        assert!(dip.seek_due(1_000 + DIP_OUT_NS));
        assert!(!dip.is_over(1_000 + DIP_OUT_NS + DIP_IN_NS));
        let seeked = Dip {
            seeked: true,
            ..dip
        };
        assert!(seeked.is_over(1_000 + DIP_OUT_NS + DIP_IN_NS));
    }
}
