//! The screen stack (ADR-0044): navigation and transitions on the update thread.
//!
//! The stack holds the application's screens (`S`, usually an enum of screen states) and
//! never calls them: the application gives input to [`ScreenStack::top_mut`], turns what
//! the screen asks for into a [`Nav`] and writes [`ScreenStack::layers`] into its snapshot
//! for the draw thread.

use crate::anim::{ease_out_cubic, ease_out_quint, progress};
use crate::theme::{Theme, faded, transition as fancy};
use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};

/// Length of a screen transition.
pub const TRANSITION_NS: u64 = 200_000_000;
/// How far a screen slides during a transition, as a fraction of the window width.
pub const SLIDE: f32 = 0.1;
/// Deepest stack; the screens' vector is allocated for it up front.
pub const MAX_DEPTH: usize = 16;
/// Most layers visible at once: the top screen and the one it replaces.
pub const MAX_LAYERS: usize = 2;

/// What a screen asks the stack to do.
#[derive(Debug, PartialEq)]
pub enum Nav<S> {
    /// Opens a screen over the current one.
    Push(S),
    /// Closes the current screen; at the root, quits.
    Pop,
    /// Swaps the current screen for another.
    Replace(S),
    /// Quits the application.
    Exit,
}

/// Outcome of [`ScreenStack::navigate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigated {
    /// The stack changed and the application runs on.
    Stay,
    /// A push past [`MAX_DEPTH`]: the stack did not change. Not logged here, the update
    /// thread does no I/O.
    Full,
    /// The application should quit.
    Exit,
}

/// How screens change (ADR-0126, ADR-0129).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransitionStyle {
    /// At once; the leaving screen is not drawn.
    None,
    /// A short fade with a slide (ADR-0044).
    Simple,
    /// A longer one with scale and an accent band across the screen (ADR-0126).
    Band,
    /// Skewed bars grow over the screen one after another and give way to the new one.
    Slices,
    /// An accent circle opens from the pointer, covers the screen and opens onto the new one.
    Iris,
    /// A grid of skewed tiles closes over the screen and opens again.
    Shards,
    /// Two skewed panels close and open.
    Shutter,
    /// Each transition takes one of the fancy ones, never the same twice in a row.
    #[default]
    Random,
}

/// The fancy styles `Random` picks from.
const FANCY: [TransitionStyle; 5] = [
    TransitionStyle::Band,
    TransitionStyle::Slices,
    TransitionStyle::Iris,
    TransitionStyle::Shards,
    TransitionStyle::Shutter,
];

impl TransitionStyle {
    pub const ALL: [TransitionStyle; 8] = [
        TransitionStyle::None,
        TransitionStyle::Simple,
        TransitionStyle::Band,
        TransitionStyle::Slices,
        TransitionStyle::Iris,
        TransitionStyle::Shards,
        TransitionStyle::Shutter,
        TransitionStyle::Random,
    ];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            TransitionStyle::None => "none",
            TransitionStyle::Simple => "simple",
            TransitionStyle::Band => "band",
            TransitionStyle::Slices => "slices",
            TransitionStyle::Iris => "iris",
            TransitionStyle::Shards => "shards",
            TransitionStyle::Shutter => "shutter",
            TransitionStyle::Random => "random",
        }
    }

    /// The style called `name`; the old `fancy` is `band` (ADR-0129).
    pub fn from_name(name: &str) -> Option<TransitionStyle> {
        if name == "fancy" {
            return Some(TransitionStyle::Band);
        }
        TransitionStyle::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The Fluent key of the style's label.
    pub fn label_key(self) -> &'static str {
        match self {
            TransitionStyle::None => "settings-option-transitions-none",
            TransitionStyle::Simple => "settings-option-transitions-simple",
            TransitionStyle::Band => "settings-option-transitions-band",
            TransitionStyle::Slices => "settings-option-transitions-slices",
            TransitionStyle::Iris => "settings-option-transitions-iris",
            TransitionStyle::Shards => "settings-option-transitions-shards",
            TransitionStyle::Shutter => "settings-option-transitions-shutter",
            TransitionStyle::Random => "settings-option-transitions-random",
        }
    }

    /// Length of a transition in this style.
    pub fn duration_ns(self) -> u64 {
        match self {
            TransitionStyle::None => 0,
            TransitionStyle::Simple => TRANSITION_NS,
            TransitionStyle::Band | TransitionStyle::Random => fancy::FANCY_NS,
            TransitionStyle::Slices => fancy::SLICES_NS,
            TransitionStyle::Iris => fancy::IRIS_NS,
            TransitionStyle::Shards => fancy::SHARDS_NS,
            TransitionStyle::Shutter => fancy::SHUTTER_NS,
        }
    }

    /// Whether the style covers the screen with shapes while the screens swap behind them.
    fn covers(self) -> bool {
        matches!(
            self,
            TransitionStyle::Slices
                | TransitionStyle::Iris
                | TransitionStyle::Shards
                | TransitionStyle::Shutter
        )
    }

    /// Eased progress `0 ..= 1` of the screens' motion, from the linear one.
    fn shaped(self, linear: f32) -> f32 {
        match self {
            TransitionStyle::Band => ease_out_quint(linear),
            _ => ease_out_cubic(linear),
        }
    }

    /// Slide, as a fraction of the window width, and the scales an incoming screen starts
    /// at and an outgoing one ends at.
    fn motion(self) -> (f32, f32, f32) {
        match self {
            TransitionStyle::Band | TransitionStyle::Random => (
                fancy::FANCY_SLIDE,
                fancy::FANCY_SCALE_IN,
                fancy::FANCY_SCALE_OUT,
            ),
            TransitionStyle::Slices => (fancy::SLICES_SLIDE, 1.0, 1.0),
            TransitionStyle::Iris => (0.0, fancy::IRIS_SCALE_IN, fancy::IRIS_SCALE_OUT),
            TransitionStyle::Shards => (0.0, fancy::SHARDS_SCALE_IN, fancy::SHARDS_SCALE_OUT),
            TransitionStyle::Shutter => (0.0, 1.0, fancy::SHUTTER_SCALE_OUT),
            _ => (SLIDE, 1.0, 1.0),
        }
    }

    /// Poses of the screen coming in and the one going out, at the eased progress `p` and
    /// the linear one `linear`, when the incoming one comes from `side`: +1 right, -1 left.
    /// Covering styles swap the screens' opacity while the shapes hide it.
    fn poses(self, p: f32, linear: f32, side: f32) -> (Pose, Pose) {
        let (slide, scale_in, scale_out) = self.motion();
        let swap = if self.covers() {
            smooth((linear - 0.4) / 0.2)
        } else {
            p
        };
        let incoming = Pose {
            opacity: swap,
            offset: side * slide * (1.0 - p),
            scale: 1.0 + (scale_in - 1.0) * (1.0 - p),
        };
        let outgoing = Pose {
            opacity: 1.0 - swap,
            offset: -side * slide * p,
            scale: 1.0 + (scale_out - 1.0) * p,
        };
        (incoming, outgoing)
    }
}

/// `x` clamped to `0 ..= 1`, eased in and out.
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// How covered one shape is `t` into a covering transition: it grows from its `delay` on,
/// is whole by the middle, and shrinks away from `delay` after it. With `delay` up to
/// [`fancy::MAX_DELAY`] everything is covered at the middle and clear at the end.
fn pulse(t: f32, delay: f32) -> f32 {
    smooth((t - delay) / fancy::GROW) * (1.0 - smooth((t - 0.5 - delay) / fancy::GROW))
}

/// The accent band of a `band` transition (ADR-0126), as a function of time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    /// Eased progress across the screen, `0 ..= 1`.
    pub progress: f32,
    /// Push or replace: the band crosses right to left; on a pop, left to right.
    pub leftward: bool,
    /// 0 at both ends, 1 in the middle; multiplied by the token's peak opacity to draw.
    pub strength: f32,
}

impl Band {
    /// The band `linear` of the way through a transition.
    fn at(linear: f32, leftward: bool) -> Band {
        Band {
            progress: ease_out_quint(linear),
            leftward,
            strength: (linear * std::f32::consts::PI).sin().max(0.0),
        }
    }

    /// The band as a shape in a `width` × `height` window: a skewed, accent-coloured
    /// strip the full height, coming in past one edge and leaving past the other. `None`
    /// while it is invisible.
    pub fn shape(
        &self,
        theme: &Theme,
        white: AtlasRegion,
        width: f32,
        height: f32,
    ) -> Option<Shape> {
        let alpha = fancy::BAND_PEAK_OPACITY * self.strength;
        if alpha <= 0.0 || width <= 0.0 || height <= 0.0 {
            return None;
        }
        let skew = theme.button_skew();
        let band_width = width * fancy::BAND_WIDTH;
        // The top edge leans by this much; the strip starts and ends clear of the window.
        let lean = height * skew.tan();
        let reach = 0.5 * (band_width + lean.abs());
        let (from, to) = if self.leftward {
            (width + reach, -reach)
        } else {
            (-reach, width + reach)
        };
        let centre = from + (to - from) * self.progress;
        Some(Shape {
            skew,
            ..Shape::new(
                white.texture,
                white.uv,
                [centre - band_width * 0.5, 0.0],
                [band_width, height],
                faded(theme.accent, alpha),
            )
        })
    }

    fn push_with(
        &self,
        theme: &Theme,
        white: AtlasRegion,
        width: f32,
        height: f32,
        emit: &mut impl FnMut(Shape),
    ) {
        if let Some(shape) = self.shape(theme, white, width, height) {
            emit(shape);
        }
    }

    /// Draws the band into `shapes`, over everything recorded so far.
    pub fn push(
        &self,
        shapes: &mut ShapeBatch,
        theme: &Theme,
        white: AtlasRegion,
        width: f32,
        height: f32,
    ) {
        if let Some(shape) = self.shape(theme, white, width, height) {
            shapes.push_shape(&shape);
        }
    }
}

/// The shapes a transition draws over the layers (ADR-0129): a pure function of the style,
/// the direction and the time, copied into the snapshot for the draw thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionFx {
    /// The style, never `None` or `Random`.
    pub style: TransitionStyle,
    /// Push or replace; on a pop, the sweeps go the other way.
    pub forward: bool,
    /// Linear progress, `0 ..= 1`.
    pub linear: f32,
    /// Window pixels where the iris opens; the centre if unknown.
    pub origin: Option<[f32; 2]>,
}

/// Most shapes a transition draws at once.
pub const MAX_FX_SHAPES: usize = 64;

impl TransitionFx {
    /// Hands each shape of the transition in a `width` × `height` window to `emit`, bottom
    /// first: at most [`MAX_FX_SHAPES`], none at the very start and end. Colours are the
    /// theme's accent and background, the skew its button skew. Allocates nothing.
    pub fn emit(
        &self,
        theme: &Theme,
        white: AtlasRegion,
        width: f32,
        height: f32,
        mut emit: impl FnMut(Shape),
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }
        let skew = theme.button_skew();
        let (accent, background) = (theme.accent, theme.background);
        let t = self.linear;
        let shape = |position: [f32; 2], size: [f32; 2], colour, skew: f32| Shape {
            skew,
            ..Shape::new(white.texture, white.uv, position, size, colour)
        };
        match self.style {
            TransitionStyle::Band => {
                Band::at(t, self.forward).push_with(theme, white, width, height, &mut emit);
            }
            TransitionStyle::Slices => {
                let n = fancy::SLICE_COUNT;
                let lean = (height * skew.tan()).abs();
                let bar = (width + 2.0 * lean) / n as f32;
                for i in 0..n {
                    // The sweep starts at the edge the new screen comes from.
                    let order = if self.forward { n - 1 - i } else { i };
                    let delay = fancy::MAX_DELAY * order as f32 / (n - 1) as f32;
                    let c = pulse(t, delay);
                    if c <= 0.0 {
                        continue;
                    }
                    let even = i % 2 == 0;
                    let tall = height * c;
                    let y = if even { 0.0 } else { height - tall };
                    let x = -lean + bar * i as f32 - 1.0;
                    let colour = if even { accent } else { background };
                    emit(shape([x, y], [bar + 2.0, tall], colour, skew));
                }
            }
            TransitionStyle::Iris => {
                let [ox, oy] = self.origin.unwrap_or([width * 0.5, height * 0.5]);
                let reach = (ox.max(width - ox)).hypot(oy.max(height - oy)) + 1.0;
                let (radius, outline) = if t < 0.5 {
                    (reach * ease_out_cubic(t * 2.0), 0.0)
                } else {
                    // The disc becomes a ring whose hole opens from the pointer.
                    let hole = reach * smooth((t - 0.5) * 2.0);
                    (reach, reach - hole)
                };
                if radius >= 0.5 && (t < 0.5 || outline >= 0.5) {
                    emit(Shape {
                        radius,
                        outline,
                        ..shape(
                            [ox - radius, oy - radius],
                            [radius * 2.0, radius * 2.0],
                            accent,
                            0.0,
                        )
                    });
                }
            }
            TransitionStyle::Shards => {
                let (cols, rows) = (fancy::SHARD_COLS, fancy::SHARD_ROWS);
                let cell_h = height / rows as f32;
                let lean = (cell_h * skew.tan()).abs();
                let cell_w = (width + 2.0 * lean) / cols as f32;
                let diagonals = (cols + rows - 2) as f32;
                for row in 0..rows {
                    for col in 0..cols {
                        let along = if self.forward { cols - 1 - col } else { col };
                        let delay = fancy::MAX_DELAY * (along + row) as f32 / diagonals;
                        let c = pulse(t, delay);
                        if c <= 0.0 {
                            continue;
                        }
                        let size = [(cell_w + 2.0) * c, (cell_h + 2.0) * c];
                        let centre = [
                            -lean + cell_w * (col as f32 + 0.5),
                            cell_h * (row as f32 + 0.5),
                        ];
                        let colour = if (col + row) % 2 == 0 {
                            accent
                        } else {
                            background
                        };
                        emit(shape(
                            [centre[0] - size[0] * 0.5, centre[1] - size[1] * 0.5],
                            size,
                            colour,
                            skew,
                        ));
                    }
                }
            }
            TransitionStyle::Shutter => {
                let c = pulse(t, 0.0);
                if c <= 0.0 {
                    return;
                }
                let lean = (height * skew.tan()).abs();
                let panel = (width * 0.5 + lean + 1.0) * c;
                emit(shape([-lean, 0.0], [panel, height], accent, skew));
                emit(shape(
                    [width + lean - panel, 0.0],
                    [panel, height],
                    accent,
                    skew,
                ));
            }
            TransitionStyle::None | TransitionStyle::Simple | TransitionStyle::Random => {}
        }
    }

    /// Draws the transition's shapes into `shapes`, over everything recorded so far.
    pub fn push(
        &self,
        shapes: &mut ShapeBatch,
        theme: &Theme,
        white: AtlasRegion,
        width: f32,
        height: f32,
    ) {
        self.emit(theme, white, width, height, |shape| {
            shapes.push_shape(&shape)
        });
    }
}

/// Where a visible layer is in its transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// 0 is invisible, 1 opaque; multiplies the screen's colours.
    pub opacity: f32,
    /// Horizontal shift as a fraction of the window width; positive is to the right.
    pub offset: f32,
    /// Scale about the window's centre; 1 is none.
    pub scale: f32,
}

impl Pose {
    /// A screen at rest.
    pub const SHOWN: Pose = Pose {
        opacity: 1.0,
        offset: 0.0,
        scale: 1.0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    /// Push or replace: the new screen comes in from the right.
    Forward,
    /// Pop: the top screen leaves to the right.
    Back,
}

#[derive(Clone, Copy, Debug)]
struct Transition {
    start_ns: u64,
    direction: Direction,
    /// The style the transition started in (never `Random`: that is resolved at the
    /// start); a setting changed meanwhile waits.
    style: TransitionStyle,
    /// Where the pointer was, in window pixels, if known: an iris opens from it.
    origin: Option<[f32; 2]>,
}

/// The screens of the application, bottom first.
#[derive(Debug)]
pub struct ScreenStack<S> {
    screens: Vec<S>,
    /// A popped or replaced screen still animating out; it gets no input.
    leaving: Option<S>,
    transition: Option<Transition>,
    /// The style the next transition starts in.
    style: TransitionStyle,
    /// Where the next transition's iris opens, in window pixels.
    origin: Option<[f32; 2]>,
    /// Random transitions so far, and the fancy style the last one took.
    random_count: u32,
    last_random: Option<TransitionStyle>,
}

impl<S> ScreenStack<S> {
    /// A stack with `root` at rest.
    pub fn new(root: S) -> ScreenStack<S> {
        let mut screens = Vec::with_capacity(MAX_DEPTH);
        screens.push(root);
        ScreenStack {
            screens,
            leaving: None,
            transition: None,
            // The application sets the player's style before the first navigation.
            style: TransitionStyle::Simple,
            origin: None,
            random_count: 0,
            last_random: None,
        }
    }

    /// Sets the style of the transitions that start from now on; one running keeps its own.
    pub fn set_style(&mut self, style: TransitionStyle) {
        self.style = style;
    }

    /// Where an iris opens in the transitions that start from now on, in window pixels; the
    /// window's centre when `None`.
    pub fn set_origin(&mut self, origin: Option<[f32; 2]>) {
        self.origin = origin;
    }

    /// The fancy style the next `Random` transition takes: a fixed walk over [`FANCY`]
    /// that never repeats the previous pick.
    fn pick_random(&mut self) -> TransitionStyle {
        let n = FANCY.len();
        let mut index = (self.random_count as usize * 2) % n;
        self.random_count = self.random_count.wrapping_add(1);
        if Some(FANCY[index]) == self.last_random {
            index = (index + 1) % n;
        }
        self.last_random = Some(FANCY[index]);
        FANCY[index]
    }

    /// The screen that takes input.
    pub fn top(&self) -> &S {
        self.screens.last().expect("the stack is never empty")
    }

    /// The screen that takes input.
    pub fn top_mut(&mut self) -> &mut S {
        self.screens.last_mut().expect("the stack is never empty")
    }

    /// Number of screens, not counting one that is leaving.
    pub fn depth(&self) -> usize {
        self.screens.len()
    }

    /// Whether a transition is still running at `now_ns`.
    pub fn in_transition(&self, now_ns: u64) -> bool {
        self.transition
            .is_some_and(|t| now_ns < t.start_ns.saturating_add(t.style.duration_ns()))
    }

    /// Applies `nav` at `now_ns`. A transition already running ends at once: its leaving
    /// screen is dropped and the new transition starts from rest. Allocates only what
    /// dropping or building screens does.
    pub fn navigate(&mut self, nav: Nav<S>, now_ns: u64) -> Navigated {
        let direction = match nav {
            Nav::Exit => return Navigated::Exit,
            Nav::Pop if self.screens.len() == 1 => return Navigated::Exit,
            Nav::Push(_) if self.screens.len() == MAX_DEPTH => return Navigated::Full,
            Nav::Push(screen) => {
                self.leaving = None;
                self.screens.push(screen);
                Direction::Forward
            }
            Nav::Replace(screen) => {
                let old = std::mem::replace(self.top_mut(), screen);
                self.leaving = Some(old);
                Direction::Forward
            }
            Nav::Pop => {
                self.leaving = self.screens.pop();
                Direction::Back
            }
        };
        if self.style == TransitionStyle::None {
            // At once: nothing leaves, nothing animates.
            self.leaving = None;
            self.transition = None;
        } else {
            let style = match self.style {
                TransitionStyle::Random => self.pick_random(),
                style => style,
            };
            self.transition = Some(Transition {
                start_ns: now_ns,
                direction,
                style,
                origin: self.origin,
            });
        }
        Navigated::Stay
    }

    /// Ends a finished transition: drops the leaving screen. Call once per tick.
    pub fn tick(&mut self, now_ns: u64) {
        if self.transition.is_some() && !self.in_transition(now_ns) {
            self.transition = None;
            self.leaving = None;
        }
    }

    /// The visible screens at `now_ns`, bottom first: the top one alone at rest, or the top
    /// one and the one it replaces during a transition.
    pub fn layers(&self, now_ns: u64) -> [Option<(&S, Pose)>; MAX_LAYERS] {
        let top = self.top();
        let Some(transition) = self.transition else {
            return [Some((top, Pose::SHOWN)), None];
        };
        let linear = progress(transition.start_ns, now_ns, transition.style.duration_ns());
        let p = transition.style.shaped(linear);
        match transition.direction {
            Direction::Forward => {
                let (incoming, outgoing) = transition.style.poses(p, linear, 1.0);
                // A replaced screen is leaving; a pushed one covers the screen below it.
                let below = self
                    .leaving
                    .as_ref()
                    .or_else(|| self.screens.len().checked_sub(2).map(|i| &self.screens[i]));
                [below.map(|s| (s, outgoing)), Some((top, incoming))]
            }
            Direction::Back => {
                let (returning, outgoing) = transition.style.poses(p, linear, -1.0);
                [
                    Some((top, returning)),
                    self.leaving.as_ref().map(|s| (s, outgoing)),
                ]
            }
        }
    }

    /// The accent band at `now_ns`: only during a `band` transition.
    pub fn band(&self, now_ns: u64) -> Option<Band> {
        let transition = self.transition?;
        if transition.style != TransitionStyle::Band || !self.in_transition(now_ns) {
            return None;
        }
        let linear = progress(transition.start_ns, now_ns, transition.style.duration_ns());
        Some(Band::at(linear, transition.direction == Direction::Forward))
    }

    /// What the running transition draws over the layers at `now_ns`, if it draws anything.
    pub fn fx(&self, now_ns: u64) -> Option<TransitionFx> {
        let transition = self.transition?;
        if transition.style == TransitionStyle::None || !self.in_transition(now_ns) {
            return None;
        }
        Some(TransitionFx {
            style: transition.style,
            forward: transition.direction == Direction::Forward,
            linear: progress(transition.start_ns, now_ns, transition.style.duration_ns()),
            origin: transition.origin,
        })
    }
}

#[cfg(test)]
mod tests;
