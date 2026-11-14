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

/// How screens change (ADR-0126).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransitionStyle {
    /// At once; the leaving screen is not drawn.
    None,
    /// A short fade with a slide (ADR-0044).
    #[default]
    Simple,
    /// A longer one with scale and an accent band across the screen.
    Fancy,
}

impl TransitionStyle {
    pub const ALL: [TransitionStyle; 3] = [
        TransitionStyle::None,
        TransitionStyle::Simple,
        TransitionStyle::Fancy,
    ];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            TransitionStyle::None => "none",
            TransitionStyle::Simple => "simple",
            TransitionStyle::Fancy => "fancy",
        }
    }

    /// The style called `name`.
    pub fn from_name(name: &str) -> Option<TransitionStyle> {
        TransitionStyle::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The Fluent key of the style's label.
    pub fn label_key(self) -> &'static str {
        match self {
            TransitionStyle::None => "settings-option-transitions-none",
            TransitionStyle::Simple => "settings-option-transitions-simple",
            TransitionStyle::Fancy => "settings-option-transitions-fancy",
        }
    }

    /// Length of a transition in this style.
    pub fn duration_ns(self) -> u64 {
        match self {
            TransitionStyle::None => 0,
            TransitionStyle::Simple => TRANSITION_NS,
            TransitionStyle::Fancy => fancy::FANCY_NS,
        }
    }

    /// Eased progress `0 ..= 1` of a transition that started at `start_ns`.
    fn eased(self, start_ns: u64, now_ns: u64) -> f32 {
        let linear = progress(start_ns, now_ns, self.duration_ns());
        match self {
            TransitionStyle::Fancy => ease_out_quint(linear),
            _ => ease_out_cubic(linear),
        }
    }

    /// Slide, as a fraction of the window width, and the scales an incoming screen starts
    /// at and an outgoing one ends at.
    fn motion(self) -> (f32, f32, f32) {
        match self {
            TransitionStyle::Fancy => (
                fancy::FANCY_SLIDE,
                fancy::FANCY_SCALE_IN,
                fancy::FANCY_SCALE_OUT,
            ),
            _ => (SLIDE, 1.0, 1.0),
        }
    }

    /// Poses of the screen coming in and the one going out, `p` into the transition
    /// (eased), when the incoming one comes from `side`: +1 right, -1 left.
    fn poses(self, p: f32, side: f32) -> (Pose, Pose) {
        let (slide, scale_in, scale_out) = self.motion();
        let incoming = Pose {
            opacity: p,
            offset: side * slide * (1.0 - p),
            scale: 1.0 + (scale_in - 1.0) * (1.0 - p),
        };
        let outgoing = Pose {
            opacity: 1.0 - p,
            offset: -side * slide * p,
            scale: 1.0 + (scale_out - 1.0) * p,
        };
        (incoming, outgoing)
    }
}

/// The accent band of a `fancy` transition (ADR-0126), as a function of time.
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
    /// The style the transition started in; a setting changed meanwhile waits.
    style: TransitionStyle,
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
            style: TransitionStyle::default(),
        }
    }

    /// Sets the style of the transitions that start from now on; one running keeps its own.
    pub fn set_style(&mut self, style: TransitionStyle) {
        self.style = style;
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
            self.transition = Some(Transition {
                start_ns: now_ns,
                direction,
                style: self.style,
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
        let p = transition.style.eased(transition.start_ns, now_ns);
        match transition.direction {
            Direction::Forward => {
                let (incoming, outgoing) = transition.style.poses(p, 1.0);
                // A replaced screen is leaving; a pushed one covers the screen below it.
                let below = self
                    .leaving
                    .as_ref()
                    .or_else(|| self.screens.len().checked_sub(2).map(|i| &self.screens[i]));
                [below.map(|s| (s, outgoing)), Some((top, incoming))]
            }
            Direction::Back => {
                let (returning, outgoing) = transition.style.poses(p, -1.0);
                [
                    Some((top, returning)),
                    self.leaving.as_ref().map(|s| (s, outgoing)),
                ]
            }
        }
    }

    /// The accent band at `now_ns`: only during a `fancy` transition.
    pub fn band(&self, now_ns: u64) -> Option<Band> {
        let transition = self.transition?;
        if transition.style != TransitionStyle::Fancy || !self.in_transition(now_ns) {
            return None;
        }
        let linear = progress(transition.start_ns, now_ns, transition.style.duration_ns());
        Some(Band {
            progress: transition.style.eased(transition.start_ns, now_ns),
            leftward: transition.direction == Direction::Forward,
            strength: (linear * std::f32::consts::PI).sin().max(0.0),
        })
    }
}

#[cfg(test)]
mod tests;
