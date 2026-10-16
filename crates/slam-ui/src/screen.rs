//! The screen stack (ADR-0044): navigation and transitions on the update thread.
//!
//! The stack holds the application's screens (`S`, usually an enum of screen states) and
//! never calls them: the application gives input to [`ScreenStack::top_mut`], turns what
//! the screen asks for into a [`Nav`] and writes [`ScreenStack::layers`] into its snapshot
//! for the draw thread.

use crate::anim::{ease_out_cubic, progress};

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

/// Where a visible layer is in its transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// 0 is invisible, 1 opaque; multiplies the screen's colours.
    pub opacity: f32,
    /// Horizontal shift as a fraction of the window width; positive is to the right.
    pub offset: f32,
}

impl Pose {
    /// A screen at rest.
    pub const SHOWN: Pose = Pose {
        opacity: 1.0,
        offset: 0.0,
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
}

/// The screens of the application, bottom first.
#[derive(Debug)]
pub struct ScreenStack<S> {
    screens: Vec<S>,
    /// A popped or replaced screen still animating out; it gets no input.
    leaving: Option<S>,
    transition: Option<Transition>,
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
        }
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
            .is_some_and(|t| now_ns < t.start_ns.saturating_add(TRANSITION_NS))
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
        self.transition = Some(Transition {
            start_ns: now_ns,
            direction,
        });
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
        let p = ease_out_cubic(progress(transition.start_ns, now_ns, TRANSITION_NS));
        match transition.direction {
            Direction::Forward => {
                let incoming = Pose {
                    opacity: p,
                    offset: SLIDE * (1.0 - p),
                };
                let outgoing = Pose {
                    opacity: 1.0 - p,
                    offset: -SLIDE * p,
                };
                // A replaced screen is leaving; a pushed one covers the screen below it.
                let below = self
                    .leaving
                    .as_ref()
                    .or_else(|| self.screens.len().checked_sub(2).map(|i| &self.screens[i]));
                [below.map(|s| (s, outgoing)), Some((top, incoming))]
            }
            Direction::Back => {
                let returning = Pose {
                    opacity: p,
                    offset: -SLIDE * (1.0 - p),
                };
                let outgoing = Pose {
                    opacity: 1.0 - p,
                    offset: SLIDE * p,
                };
                [
                    Some((top, returning)),
                    self.leaving.as_ref().map(|s| (s, outgoing)),
                ]
            }
        }
    }
}

#[cfg(test)]
mod tests;
