//! Live input: window events to steps of the gameplay rules (ADR-0032).
//!
//! Button presses become [`ActionEvent`]s through the key bindings, each at the game time of
//! its own timestamp; mouse motion moves the cursor in osu!pixels through the
//! [`PlayfieldLayout`]. One [`LiveInput::step`] per update tick yields the rules'
//! [`StepInput`].

use slam_audio::ClockSnapshot;
use slam_input::{InputEvent, InputKind, MouseButton};
use slam_osu::Vec2;
use slam_osu::replay::{ActionEvent, Actions, StepInput};

use crate::cursor::CursorPath;
use crate::playfield::PlayfieldLayout;
use crate::time::{Offsets, game_time};

/// SDL scancodes (USB HID usage ids) of the default keys.
const SCANCODE_Z: u16 = 29;
const SCANCODE_X: u16 = 27;
const SCANCODE_C: u16 = 6;

/// Most bindings a [`Bindings`] holds.
pub const MAX_BINDINGS: usize = 16;

/// Most inputs (keys and mouse buttons) tracked as held at once. Only bound inputs are
/// tracked, so every bound input fits.
pub const MAX_HELD_INPUTS: usize = MAX_BINDINGS;

/// Most presses and releases one step delivers; the rest wait for the next step.
pub const MAX_LIVE_STEP_EVENTS: usize = 32;

/// Most input events waiting for a later step; events beyond it are dropped and counted.
pub const MAX_PENDING_EVENTS: usize = 256;

/// A physical input a binding listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Input {
    /// A keyboard key by scancode (USB HID usage id, as `SDL_Scancode`).
    Key(u16),
    /// A mouse button.
    Mouse(MouseButton),
}

/// One input bound to one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The input.
    pub input: Input,
    /// The action: one of [`Actions::SINGLE`].
    pub action: Actions,
}

/// The key bindings of gameplay, with a fixed capacity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bindings {
    list: [Binding; MAX_BINDINGS],
    len: usize,
}

impl Bindings {
    /// Builds bindings from a list; bindings past [`MAX_BINDINGS`] are dropped. An input bound
    /// to several actions presses all of them.
    pub fn new(bindings: &[Binding]) -> Bindings {
        let mut list = [Binding {
            input: Input::Key(0),
            action: Actions::NONE,
        }; MAX_BINDINGS];
        let len = bindings.len().min(MAX_BINDINGS);
        list[..len].copy_from_slice(&bindings[..len]);
        Bindings { list, len }
    }

    /// The bindings.
    pub fn as_slice(&self) -> &[Binding] {
        &self.list[..self.len]
    }

    /// The actions bound to `input`.
    pub fn actions_of(&self, input: Input) -> Actions {
        self.as_slice()
            .iter()
            .filter(|b| b.input == input)
            .fold(Actions::NONE, |acc, b| acc | b.action)
    }
}

impl Default for Bindings {
    /// Lazer's default osu!standard bindings: Z and the left mouse button for the left
    /// button, X and the right mouse button for the right button, C for smoke.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/OsuRuleset.cs (GetDefaultKeyBindings)
    fn default() -> Bindings {
        Bindings::new(&[
            Binding {
                input: Input::Key(SCANCODE_Z),
                action: Actions::LEFT,
            },
            Binding {
                input: Input::Key(SCANCODE_X),
                action: Actions::RIGHT,
            },
            Binding {
                input: Input::Key(SCANCODE_C),
                action: Actions::SMOKE,
            },
            Binding {
                input: Input::Mouse(MouseButton::Left),
                action: Actions::LEFT,
            },
            Binding {
                input: Input::Mouse(MouseButton::Right),
                action: Actions::RIGHT,
            },
        ])
    }
}

/// Maps engine timestamps to game time for one step.
pub trait GameTime {
    /// Game time in milliseconds at engine time `time_ns`.
    fn game_time(&self, time_ns: u64) -> f64;
}

/// The game clock's snapshot for this tick with the user's offsets: the game time of live
/// play ([`game_time`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockTime {
    /// The game clock's snapshot for this tick.
    pub clock: ClockSnapshot,
    /// The user's offsets.
    pub offsets: Offsets,
}

impl GameTime for ClockTime {
    fn game_time(&self, time_ns: u64) -> f64 {
        game_time(&self.clock, time_ns, &self.offsets)
    }
}

/// An input held, with the actions it pressed.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Held {
    input: Input,
    actions: Actions,
}

/// Live input state between update ticks. Never allocates after construction.
#[derive(Debug, Clone)]
pub struct LiveInput {
    bindings: Bindings,
    layout: PlayfieldLayout,
    mouse_buttons_disabled: bool,
    /// Last cursor position in window units, if the cursor moved yet.
    window_position: Option<Vec2>,
    /// Cursor position in osu!pixels.
    position: Vec2,
    /// The positions gameplay took, for the cursor trail.
    path: CursorPath,
    /// Inputs held, in press order.
    held_inputs: [Held; MAX_HELD_INPUTS],
    held_len: usize,
    /// Actions held: the union of `held_inputs`.
    held: Actions,
    /// Time of the previous step: no event of a later step is earlier.
    last_time: f64,
    /// Events not delivered yet because a step was full, oldest first (a ring).
    pending: [InputEvent; MAX_PENDING_EVENTS],
    pending_start: usize,
    pending_len: usize,
    dropped: u64,
    events: [ActionEvent; MAX_LIVE_STEP_EVENTS],
    /// Gameplay does not take input (paused): the state below follows the window instead.
    suspended: bool,
    /// Window position of the cursor while suspended.
    parent_window: Option<Vec2>,
    /// The cursor positions followed while suspended, in osu!pixels.
    parent_path: CursorPath,
    /// Bound inputs held while suspended.
    parent_held: [Input; MAX_HELD_INPUTS],
    parent_len: usize,
    /// The press that ended the suspension, held by gameplay without a press.
    resume_press: Option<Input>,
    /// The next step syncs gameplay with the state followed while suspended.
    sync_pending: bool,
}

impl LiveInput {
    /// Live input with these bindings and layout. The cursor is at osu!pixel (0, 0) until
    /// the first mouse motion.
    pub fn new(bindings: Bindings, layout: PlayfieldLayout) -> LiveInput {
        let no_event = InputEvent {
            time_ns: 0,
            kind: InputKind::MouseMove { x: 0.0, y: 0.0 },
        };
        let no_action = ActionEvent {
            time: 0.0,
            position: Vec2::ZERO,
            action: Actions::NONE,
            pressed: false,
        };
        LiveInput {
            bindings,
            layout,
            mouse_buttons_disabled: false,
            window_position: None,
            position: Vec2::ZERO,
            path: CursorPath::default(),
            held_inputs: [Held {
                input: Input::Key(0),
                actions: Actions::NONE,
            }; MAX_HELD_INPUTS],
            held_len: 0,
            held: Actions::NONE,
            last_time: f64::NEG_INFINITY,
            pending: [no_event; MAX_PENDING_EVENTS],
            pending_start: 0,
            pending_len: 0,
            dropped: 0,
            events: [no_action; MAX_LIVE_STEP_EVENTS],
            suspended: false,
            parent_window: None,
            parent_path: CursorPath::default(),
            parent_held: [Input::Key(0); MAX_HELD_INPUTS],
            parent_len: 0,
            resume_press: None,
            sync_pending: false,
        }
    }

    /// Where the playfield sits in the window.
    pub fn layout(&self) -> PlayfieldLayout {
        self.layout
    }

    /// Changes the bindings. Inputs already held keep the actions they pressed until released.
    pub fn set_bindings(&mut self, bindings: Bindings) {
        self.bindings = bindings;
    }

    /// Changes the playfield layout (the window was resized). The cursor keeps its window
    /// position, so its osu!pixel position changes, as in lazer.
    pub fn set_layout(&mut self, layout: PlayfieldLayout) {
        self.layout = layout;
        if let Some(window) = self.window_position {
            self.position = layout.to_playfield(window);
        }
    }

    /// Ignores mouse button presses (lazer's "disable mouse buttons" setting). A button held
    /// when this is turned on still releases its actions.
    pub fn set_mouse_buttons_disabled(&mut self, disabled: bool) {
        self.mouse_buttons_disabled = disabled;
    }

    /// Whether mouse button presses are ignored ([`Self::set_mouse_buttons_disabled`]).
    pub fn mouse_buttons_disabled(&self) -> bool {
        self.mouse_buttons_disabled
    }

    /// Cursor position in osu!pixels.
    pub fn position(&self) -> Vec2 {
        self.position
    }

    /// The cursor positions gameplay took, newest last.
    pub fn path(&self) -> &CursorPath {
        &self.path
    }

    /// Actions held.
    pub fn held(&self) -> Actions {
        self.held
    }

    /// Events dropped because too many waited for a later step (consecutive waiting mouse
    /// motions count as one). A dropped release leaves its actions held until the input is
    /// pressed and released again; this takes hundreds of button events in one tick.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Whether input is suspended ([`suspend`](Self::suspend)).
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Whether the cursor has moved yet: until then lazer's gameplay cursor is hidden.
    pub fn cursor_moved(&self) -> bool {
        self.window_position.is_some()
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/DrawableRuleset.cs (IsPaused.ValueChanged)
    /// Stops delivering input to gameplay, as lazer's ruleset input manager stops using its
    /// parent's input while gameplay is paused. Gameplay keeps its held actions and cursor;
    /// [`follow_suspended`](Self::follow_suspended) follows the window meanwhile. Events
    /// still waiting for a step are taken as happening after the suspension.
    pub fn suspend(&mut self) {
        if self.suspended {
            return;
        }
        self.suspended = true;
        self.sync_pending = false;
        self.resume_press = None;
        self.parent_window = self.window_position;
        self.parent_path.push_jump(self.position);
        self.parent_len = self.held_len;
        for (parent, held) in self.parent_held.iter_mut().zip(&self.held_inputs) {
            *parent = held.input;
        }
        while self.pending_len > 0 {
            let event = self.pending[self.pending_start];
            self.pending_start = (self.pending_start + 1) % MAX_PENDING_EVENTS;
            self.pending_len -= 1;
            self.follow(event);
        }
    }

    /// While suspended, follows `events` in order without delivering them: the cursor and
    /// the bound inputs held. Each press of a bound input is offered to `stop` with its
    /// actions and the cursor position in osu!pixels; if `stop` returns `true`, following
    /// stops after that event and its index is returned. The rest of `events` is not taken.
    pub fn follow_suspended(
        &mut self,
        events: &[InputEvent],
        mut stop: impl FnMut(Actions, Vec2) -> bool,
    ) -> Option<usize> {
        debug_assert!(self.suspended, "input is not suspended");
        for (index, &event) in events.iter().enumerate() {
            if let Some((input, actions)) = self.follow(event)
                && stop(actions, self.parent_position())
            {
                self.resume_press = Some(input);
                return Some(index);
            }
        }
        None
    }

    /// The cursor the window has while suspended, in osu!pixels: lazer's resume overlay
    /// shows a cursor of its own there. Gameplay's cursor otherwise.
    pub fn window_cursor(&self) -> Vec2 {
        self.parent_position()
    }

    /// The positions of [`window_cursor`](Self::window_cursor) while suspended, newest last:
    /// each suspension starts with a jump to gameplay's cursor.
    pub fn window_path(&self) -> &CursorPath {
        &self.parent_path
    }

    /// The cursor position the window has, in osu!pixels.
    fn parent_position(&self) -> Vec2 {
        if self.suspended {
            self.parent_window
                .map_or(self.position, |w| self.layout.to_playfield(w))
        } else {
            self.position
        }
    }

    /// Follows one event while suspended. Returns the input and its actions if it is a
    /// press of a bound input.
    fn follow(&mut self, event: InputEvent) -> Option<(Input, Actions)> {
        let (input, down) = match event.kind {
            InputKind::MouseMove { x, y } => {
                let window = Vec2::new(x, y);
                self.parent_window = Some(window);
                self.parent_path.push(self.layout.to_playfield(window));
                return None;
            }
            InputKind::MouseWheel { .. }
            | InputKind::KeyRepeat { .. }
            | InputKind::Text(_)
            | InputKind::WindowMode(_) => {
                return None;
            }
            InputKind::Key { scancode, down } => (Input::Key(scancode), down),
            InputKind::MouseButton { button, down } => (Input::Mouse(button), down),
        };
        let index = self.parent_held[..self.parent_len]
            .iter()
            .position(|&held| held == input);
        if !down {
            if let Some(index) = index {
                self.parent_held
                    .copy_within(index + 1..self.parent_len, index);
                self.parent_len -= 1;
            }
            return None;
        }
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/RulesetInputManager.cs (Handle)
        // The resume overlay has a ruleset input manager of its own, which drops mouse
        // presses as gameplay's does.
        if self.mouse_buttons_disabled && matches!(input, Input::Mouse(_)) {
            return None;
        }
        let actions = self.bindings.actions_of(input);
        if actions.is_empty() || index.is_some() {
            return None;
        }
        if self.parent_len < MAX_HELD_INPUTS {
            self.parent_held[self.parent_len] = input;
            self.parent_len += 1;
        }
        Some((input, actions))
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Input/PassThroughInputManager.cs (syncWithParent)
    /// Delivers input to gameplay again. The next step first releases what was released
    /// while suspended, then moves the cursor where it went; presses made while suspended
    /// are not delivered. The press that resumed (see
    /// [`follow_suspended`](Self::follow_suspended)) is held from then on without being
    /// delivered as a press: lazer's `OsuResumeOverlay` blocks it from reaching hit objects.
    pub fn resume(&mut self) {
        if self.suspended {
            self.suspended = false;
            self.sync_pending = true;
        }
    }

    /// Forgets the held inputs and the time of the last step, for a play that starts over.
    /// Inputs held now are not held by the new play (their releases are dropped); the cursor
    /// stays.
    pub fn reset(&mut self) {
        if let Some(window) = self.parent_window.filter(|_| self.suspended) {
            self.window_position = Some(window);
            self.position = self.layout.to_playfield(window);
            self.path.push_jump(self.position);
        }
        self.held_len = 0;
        self.held = Actions::NONE;
        self.last_time = f64::NEG_INFINITY;
        self.pending_start = 0;
        self.pending_len = 0;
        self.suspended = false;
        self.parent_len = 0;
        self.resume_press = None;
        self.sync_pending = false;
    }

    /// Keeps `events` for the next step: gameplay does not step this tick.
    pub fn hold(&mut self, events: &[InputEvent]) {
        for &event in events {
            self.defer(event);
        }
    }

    /// Takes the input events of one update tick and returns the step for the rules.
    ///
    /// `events` must be in non-decreasing `time_ns` order and not newer than `now_ns`, as the
    /// engine's update loop delivers them. The step's time is the game time at `now_ns`, never
    /// before the previous step's. Each press and release gets the game time of its own
    /// timestamp, clamped between the previous step's time and this one's, and the cursor
    /// position as of that event. Events that do not fit into this step, with every event
    /// after them, are delivered by the next step.
    pub fn step(
        &mut self,
        events: &[InputEvent],
        now_ns: u64,
        time: &impl GameTime,
    ) -> StepInput<'_> {
        debug_assert!(!self.suspended, "input is suspended");
        let step_time = time.game_time(now_ns).max(self.last_time);
        let floor = self.last_time;
        let mut len = 0;
        if std::mem::take(&mut self.sync_pending) {
            // Lazer syncs when gameplay resumes: at the time it stopped.
            let at = if floor.is_finite() { floor } else { step_time };
            self.sync(at, &mut len);
        }

        // Events left over from earlier steps go first.
        while self.pending_len > 0 {
            let event = self.pending[self.pending_start];
            if !self.apply(event, floor, step_time, time, &mut len) {
                break;
            }
            self.pending_start = (self.pending_start + 1) % MAX_PENDING_EVENTS;
            self.pending_len -= 1;
        }
        for &event in events {
            if self.pending_len > 0 || !self.apply(event, floor, step_time, time, &mut len) {
                self.defer(event);
            }
        }

        self.last_time = step_time;
        StepInput {
            time: step_time,
            position: self.position,
            held: self.held,
            events: &self.events[..len],
        }
    }

    /// Syncs gameplay with the state followed while suspended, at game time `time`: releases
    /// the inputs no longer held, moves the cursor, then holds the press that resumed.
    fn sync(&mut self, time: f64, len: &mut usize) {
        let mut k = 0;
        while k < self.held_len {
            let input = self.held_inputs[k].input;
            if self.parent_held[..self.parent_len].contains(&input) {
                k += 1;
                continue;
            }
            let (released, still_held) = self.release_changes(k);
            self.emit(released, time, false, len);
            self.held_inputs.copy_within(k + 1..self.held_len, k);
            self.held_len -= 1;
            self.held = still_held;
        }
        if let Some(window) = self.parent_window {
            self.window_position = Some(window);
            self.position = self.layout.to_playfield(window);
            // Where the cursor went while suspended: a jump, not a move to draw a trail along.
            self.path.push_jump(self.position);
        }
        if let Some(input) = self.resume_press.take() {
            let bound = self.bindings.actions_of(input);
            let held = self.held_inputs[..self.held_len]
                .iter()
                .any(|h| h.input == input);
            if !held && !bound.is_empty() && self.held_len < MAX_HELD_INPUTS {
                self.held_inputs[self.held_len] = Held {
                    input,
                    actions: bound,
                };
                self.held_len += 1;
                self.held = self.held | bound;
            }
        }
    }

    /// What releasing `held_inputs[index]` changes: the actions released and those still held.
    fn release_changes(&self, index: usize) -> (Actions, Actions) {
        let released = self.held_inputs[index].actions;
        let still_held = self.held_inputs[..self.held_len]
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != index)
            .fold(Actions::NONE, |acc, (_, h)| acc | h.actions);
        (released.difference(still_held), still_held)
    }

    /// Writes a press or release of each action in `changed` at `len`. The caller checks
    /// the room.
    fn emit(&mut self, changed: Actions, time: f64, pressed: bool, len: &mut usize) {
        for action in Actions::SINGLE {
            if changed.contains(action) && *len < MAX_LIVE_STEP_EVENTS {
                self.events[*len] = ActionEvent {
                    time,
                    position: self.position,
                    action,
                    pressed,
                };
                *len += 1;
            }
        }
    }

    /// Applies one event to the state, writing its presses and releases at `len`. Returns
    /// `false`, changing nothing, if they do not fit.
    fn apply(
        &mut self,
        event: InputEvent,
        floor: f64,
        step_time: f64,
        time: &impl GameTime,
        len: &mut usize,
    ) -> bool {
        let (input, down) = match event.kind {
            InputKind::MouseMove { x, y } => {
                let window = Vec2::new(x, y);
                self.window_position = Some(window);
                self.position = self.layout.to_playfield(window);
                self.path.push(self.position);
                return true;
            }
            // Gameplay does not use the wheel, key repeats, text or window modes.
            InputKind::MouseWheel { .. }
            | InputKind::KeyRepeat { .. }
            | InputKind::Text(_)
            | InputKind::WindowMode(_) => {
                return true;
            }
            InputKind::Key { scancode, down } => (Input::Key(scancode), down),
            InputKind::MouseButton { button, down } => (Input::Mouse(button), down),
        };

        // What changes: lazer's `KeyBindingContainer` in `SimultaneousBindingMode.Unique`
        // presses an action on its first held binding and releases it with the last one.
        // Ported from osu-framework 2026.921.1: osu.Framework/Input/Bindings/KeyBindingContainer.cs (PropagatePressed, PropagateReleased)
        let held_index = self.held_inputs[..self.held_len]
            .iter()
            .position(|h| h.input == input);
        let mut bound = Actions::NONE;
        let (changed, held_after) = if down {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/RulesetInputManager.cs (Handle)
            // With mouse buttons disabled, a press never reaches the bindings.
            if self.mouse_buttons_disabled && matches!(input, Input::Mouse(_)) {
                return true;
            }
            // Unbound inputs (and presses past the capacity) are not tracked: unlike lazer, an
            // unbound key held while it gets bound does not count as held.
            bound = self.bindings.actions_of(input);
            if held_index.is_some() || self.held_len == MAX_HELD_INPUTS || bound.is_empty() {
                return true;
            }
            (bound.difference(self.held), self.held | bound)
        } else {
            let Some(index) = held_index else {
                // Lazer drops the release of a button it never saw pressed.
                return true;
            };
            self.release_changes(index)
        };

        let count = Actions::SINGLE
            .iter()
            .filter(|&&a| changed.contains(a))
            .count();
        if *len + count > MAX_LIVE_STEP_EVENTS {
            return false;
        }

        let event_time = time.game_time(event.time_ns).clamp(floor, step_time);
        self.emit(changed, event_time, down, len);

        if down {
            self.held_inputs[self.held_len] = Held {
                input,
                actions: bound,
            };
            self.held_len += 1;
        } else if let Some(index) = held_index {
            self.held_inputs
                .copy_within(index + 1..self.held_len, index);
            self.held_len -= 1;
        }
        self.held = held_after;
        true
    }

    /// Keeps an event for the next step, or drops it if too many wait. A motion right after a
    /// waiting motion replaces it: only the last position matters.
    fn defer(&mut self, event: InputEvent) {
        // Gameplay does not use the wheel, key repeats or text: they never take a slot a
        // key may need.
        if matches!(
            event.kind,
            InputKind::MouseWheel { .. } | InputKind::KeyRepeat { .. } | InputKind::Text(_)
        ) {
            return;
        }
        if self.pending_len > 0 && matches!(event.kind, InputKind::MouseMove { .. }) {
            let last = (self.pending_start + self.pending_len - 1) % MAX_PENDING_EVENTS;
            if matches!(self.pending[last].kind, InputKind::MouseMove { .. }) {
                self.pending[last] = event;
                return;
            }
        }
        if self.pending_len == MAX_PENDING_EVENTS {
            self.dropped += 1;
            return;
        }
        self.pending[(self.pending_start + self.pending_len) % MAX_PENDING_EVENTS] = event;
        self.pending_len += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Engine nanoseconds are game microseconds × 1000: 1 ms of game time per 1 ms.
    struct Linear;

    impl GameTime for Linear {
        fn game_time(&self, time_ns: u64) -> f64 {
            time_ns as f64 / 1_000_000.0
        }
    }

    fn ms(t: f64) -> u64 {
        (t * 1_000_000.0) as u64
    }

    fn key(t: f64, scancode: u16, down: bool) -> InputEvent {
        InputEvent {
            time_ns: ms(t),
            kind: InputKind::Key { scancode, down },
        }
    }

    fn mouse(t: f64, button: MouseButton, down: bool) -> InputEvent {
        InputEvent {
            time_ns: ms(t),
            kind: InputKind::MouseButton { button, down },
        }
    }

    fn moved(t: f64, x: f32, y: f32) -> InputEvent {
        InputEvent {
            time_ns: ms(t),
            kind: InputKind::MouseMove { x, y },
        }
    }

    fn live() -> LiveInput {
        // 640×480: one window point per osu!pixel, playfield origin at (64, 56).
        LiveInput::new(Bindings::default(), PlayfieldLayout::new(640.0, 480.0))
    }

    fn summary(step: &StepInput<'_>) -> Vec<(f64, Actions, bool)> {
        step.events
            .iter()
            .map(|e| (e.time, e.action, e.pressed))
            .collect()
    }

    #[test]
    fn events_keep_their_own_times_and_positions() {
        let mut input = live();
        let step = input.step(
            &[
                moved(10.2, 164.0, 156.0),
                key(10.4, SCANCODE_Z, true),
                moved(10.6, 264.0, 256.0),
                key(10.8, SCANCODE_X, true),
            ],
            ms(11.0),
            &Linear,
        );
        assert_eq!(step.time, 11.0);
        assert_eq!(step.position, Vec2::new(200.0, 200.0));
        assert_eq!(step.held, Actions::LEFT | Actions::RIGHT);
        assert_eq!(
            summary(&step),
            [(10.4, Actions::LEFT, true), (10.8, Actions::RIGHT, true)]
        );
        assert_eq!(step.events[0].position, Vec2::new(100.0, 100.0));
        assert_eq!(step.events[1].position, Vec2::new(200.0, 200.0));
    }

    #[test]
    fn an_action_is_held_until_its_last_input_is_released() {
        let mut input = live();
        let step = input.step(
            &[
                key(1.0, SCANCODE_Z, true),
                mouse(2.0, MouseButton::Left, true),
                key(3.0, SCANCODE_Z, false),
            ],
            ms(4.0),
            &Linear,
        );
        assert_eq!(summary(&step), [(1.0, Actions::LEFT, true)]);
        assert_eq!(step.held, Actions::LEFT);

        let step = input.step(&[mouse(5.0, MouseButton::Left, false)], ms(6.0), &Linear);
        assert_eq!(summary(&step), [(5.0, Actions::LEFT, false)]);
        assert_eq!(step.held, Actions::NONE);
    }

    #[test]
    fn releases_of_unpressed_or_unbound_inputs_are_ignored() {
        let mut input = live();
        let step = input.step(
            &[
                key(1.0, SCANCODE_Z, false),
                mouse(1.0, MouseButton::Middle, true),
                key(2.0, 4, true),
                key(3.0, 4, false),
                mouse(3.0, MouseButton::Middle, false),
            ],
            ms(4.0),
            &Linear,
        );
        assert!(step.events.is_empty());
        assert_eq!(step.held, Actions::NONE);
    }

    #[test]
    fn smoke_is_an_action_too() {
        let mut input = live();
        let step = input.step(
            &[key(1.0, SCANCODE_C, true), key(2.0, SCANCODE_C, false)],
            ms(3.0),
            &Linear,
        );
        assert_eq!(
            summary(&step),
            [(1.0, Actions::SMOKE, true), (2.0, Actions::SMOKE, false)]
        );
    }

    #[test]
    fn disabled_mouse_buttons_do_not_press_but_still_release() {
        let mut input = live();
        input.step(&[mouse(1.0, MouseButton::Right, true)], ms(2.0), &Linear);
        input.set_mouse_buttons_disabled(true);
        let step = input.step(
            &[
                mouse(3.0, MouseButton::Left, true),
                mouse(4.0, MouseButton::Right, false),
                key(5.0, SCANCODE_Z, true),
            ],
            ms(6.0),
            &Linear,
        );
        assert_eq!(
            summary(&step),
            [(4.0, Actions::RIGHT, false), (5.0, Actions::LEFT, true)]
        );
        // The left button was never pressed as far as the bindings know.
        let step = input.step(&[mouse(7.0, MouseButton::Left, false)], ms(8.0), &Linear);
        assert!(step.events.is_empty());
        assert_eq!(step.held, Actions::LEFT);
    }

    #[test]
    fn custom_bindings_can_press_several_actions() {
        let mut input = LiveInput::new(
            Bindings::new(&[
                Binding {
                    input: Input::Key(SCANCODE_Z),
                    action: Actions::LEFT,
                },
                Binding {
                    input: Input::Key(SCANCODE_Z),
                    action: Actions::RIGHT,
                },
            ]),
            PlayfieldLayout::new(640.0, 480.0),
        );
        let step = input.step(&[key(1.0, SCANCODE_Z, true)], ms(2.0), &Linear);
        assert_eq!(
            summary(&step),
            [(1.0, Actions::LEFT, true), (1.0, Actions::RIGHT, true)]
        );
        // The mouse is not bound any more.
        let step = input.step(&[mouse(3.0, MouseButton::Left, true)], ms(4.0), &Linear);
        assert!(step.events.is_empty());
    }

    #[test]
    fn event_times_stay_between_steps() {
        let mut input = live();
        input.step(&[], ms(10.0), &Linear);
        // A late event from before the previous step, and one the clock places after now.
        let step = input.step(
            &[key(9.0, SCANCODE_Z, true), key(10.5, SCANCODE_X, true)],
            ms(10.2),
            &Linear,
        );
        assert_eq!(
            summary(&step),
            [(10.0, Actions::LEFT, true), (10.2, Actions::RIGHT, true)]
        );
        // The clock going back does not move the step back.
        let step = input.step(&[], ms(9.5), &Linear);
        assert_eq!(step.time, 10.2);
    }

    #[test]
    fn events_that_do_not_fit_wait_for_the_next_step() {
        let mut input = live();
        let mut events = Vec::new();
        for i in 0..MAX_LIVE_STEP_EVENTS {
            let t = i as f64;
            events.push(key(t, SCANCODE_Z, i % 2 == 0));
        }
        // Over the limit: a move, then a press that must see the move.
        let last = MAX_LIVE_STEP_EVENTS as f64;
        events.push(moved(last, 100.0, 100.0));
        events.push(key(last, SCANCODE_X, true));

        let step = input.step(&events, ms(last + 1.0), &Linear);
        assert_eq!(step.events.len(), MAX_LIVE_STEP_EVENTS);
        assert_eq!(step.held, Actions::NONE);
        // Motion needs no room in the step: the move is applied, the press waits.
        assert_eq!(step.position, Vec2::new(36.0, 44.0));

        let step = input.step(
            &[key(last + 2.0, SCANCODE_C, true)],
            ms(last + 3.0),
            &Linear,
        );
        assert_eq!(
            summary(&step),
            [
                // Late now: clamped to the previous step.
                (last + 1.0, Actions::RIGHT, true),
                (last + 2.0, Actions::SMOKE, true),
            ]
        );
        assert_eq!(step.events[0].position, Vec2::new(36.0, 44.0));
        assert_eq!(input.dropped(), 0);
    }

    #[test]
    fn too_many_waiting_events_are_dropped() {
        let mut input = live();
        let events: Vec<_> = (0..MAX_LIVE_STEP_EVENTS + MAX_PENDING_EVENTS + 3)
            .map(|i| key(i as f64, SCANCODE_Z, i % 2 == 0))
            .collect();
        input.step(&events, ms(1e6), &Linear);
        assert_eq!(input.dropped(), 3);
    }

    #[test]
    fn clock_time_applies_rate_and_offsets() {
        use slam_audio::{
            GameClock, MusicAnchor, MusicSnapshot, PlayError, PositionSnapshot, TrackAdjustment,
            TrackControl,
        };

        struct NoTrack;
        impl TrackControl for NoTrack {
            fn play(&mut self) -> Result<(), PlayError> {
                Ok(())
            }
            fn play_at(&mut self, _: u64) -> Result<(), PlayError> {
                Ok(())
            }
            fn pause(&mut self) -> Result<(), PlayError> {
                Ok(())
            }
            fn seek(&mut self, _: u64) -> Result<(), PlayError> {
                Ok(())
            }
            fn set_rate(&mut self, _: f64) -> Result<(), PlayError> {
                Ok(())
            }

            fn adjust(&mut self, _: &[TrackAdjustment]) -> Result<(), PlayError> {
                Ok(())
            }
            fn generation(&self) -> u32 {
                0
            }
        }

        let anchor = MusicAnchor {
            device_frame: 0,
            track_frame: 0.0,
            speed: 1.5,
            accel: 0.0,
        };
        let mut clock = GameClock::new(&NoTrack);
        clock.update(
            0,
            Some(PositionSnapshot {
                frame: 0,
                timestamp_ns: 0,
                latency_ns: 0,
                sample_rate: 48_000,
            }),
            Some(MusicSnapshot {
                anchor,
                previous: anchor,
                generation: 0,
                ended: false,
            }),
        );
        clock.set_rate(0, 1.5, 1.5, &mut NoTrack).unwrap();
        let clock = clock.update(0, None, None).unwrap();
        let offsets = Offsets {
            global_ms: 20.0,
            beatmap_ms: -5.0,
        };
        let time = ClockTime { clock, offsets };

        // Track at 1.5x: 100 ms of output is 150 ms of track; the global offset is real time.
        let mut input = live();
        let step = input.step(&[key(100.0, SCANCODE_Z, true)], ms(200.0), &time);
        assert_eq!(step.time, 300.0 + 30.0 - 5.0);
        assert_eq!(summary(&step), [(150.0 + 30.0 - 5.0, Actions::LEFT, true)]);
    }

    #[test]
    fn bindings_changed_while_held_release_what_was_pressed() {
        let mut input = live();
        input.step(&[key(1.0, SCANCODE_Z, true)], ms(2.0), &Linear);
        // Z becomes the right button; C the left button.
        input.set_bindings(Bindings::new(&[
            Binding {
                input: Input::Key(SCANCODE_Z),
                action: Actions::RIGHT,
            },
            Binding {
                input: Input::Key(SCANCODE_C),
                action: Actions::LEFT,
            },
        ]));
        let step = input.step(
            &[key(3.0, SCANCODE_C, true), key(4.0, SCANCODE_Z, false)],
            ms(5.0),
            &Linear,
        );
        // Left is already held by Z: C presses nothing new; Z releases nothing C still holds.
        assert!(step.events.is_empty());
        assert_eq!(step.held, Actions::LEFT);
        let step = input.step(&[key(6.0, SCANCODE_C, false)], ms(7.0), &Linear);
        assert_eq!(summary(&step), [(6.0, Actions::LEFT, false)]);
    }

    #[test]
    fn resizing_keeps_the_cursor_where_it_is_in_the_window() {
        let mut input = live();
        assert_eq!(input.position(), Vec2::ZERO);
        // No motion yet: nothing to keep.
        input.set_layout(PlayfieldLayout::new(1280.0, 960.0));
        assert_eq!(input.position(), Vec2::ZERO);

        input.set_layout(PlayfieldLayout::new(640.0, 480.0));
        input.step(&[moved(1.0, 320.0, 248.0)], ms(2.0), &Linear);
        assert_eq!(input.position(), Vec2::new(256.0, 192.0));
        // Twice the size: the same window point is nearer the playfield's corner.
        input.set_layout(PlayfieldLayout::new(1280.0, 960.0));
        assert_eq!(input.position(), Vec2::new(96.0, 68.0));
    }

    #[test]
    fn waiting_events_come_out_in_order_across_the_ring() {
        // Each event presses or releases one action: Z, X, C down, then up, repeated.
        let keys = [SCANCODE_Z, SCANCODE_X, SCANCODE_C];
        let actions = [Actions::LEFT, Actions::RIGHT, Actions::SMOKE];
        let event = |i: usize| key(i as f64, keys[i % 3], (i / 3).is_multiple_of(2));
        let expected = |i: usize| (actions[i % 3], (i / 3).is_multiple_of(2));

        let mut input = live();
        let mut sent = MAX_LIVE_STEP_EVENTS + MAX_PENDING_EVENTS;
        let first: Vec<_> = (0..sent).map(event).collect();
        let mut delivered = Vec::new();
        let mut now = 1e4;
        let step = input.step(&first, ms(now), &Linear);
        delivered.extend(step.events.iter().map(|e| (e.action, e.pressed)));
        // Keep the queue full while it wraps around, then drain it.
        for round in 0..20 {
            now += 1.0;
            let more: Vec<_> = if round < 12 {
                (sent..sent + MAX_LIVE_STEP_EVENTS).map(event).collect()
            } else {
                Vec::new()
            };
            sent += more.len();
            let step = input.step(&more, ms(now), &Linear);
            delivered.extend(step.events.iter().map(|e| (e.action, e.pressed)));
        }
        assert_eq!(input.dropped(), 0);
        assert_eq!(delivered, (0..sent).map(expected).collect::<Vec<_>>());
    }

    #[test]
    fn waiting_motions_collapse() {
        let mut input = live();
        let mut events: Vec<_> = (0..MAX_LIVE_STEP_EVENTS + 1)
            .map(|i| key(i as f64, SCANCODE_Z, i % 2 == 0))
            .collect();
        events.extend((0..1000).map(|i| moved(40.0, 64.0 + i as f32, 56.0)));
        events.push(key(41.0, SCANCODE_X, true));
        input.step(&events, ms(50.0), &Linear);
        assert_eq!(input.dropped(), 0);
        let step = input.step(&[], ms(51.0), &Linear);
        assert_eq!(step.position, Vec2::new(999.0, 0.0));
        assert_eq!(step.held, Actions::LEFT | Actions::RIGHT);
    }

    #[test]
    fn suspended_input_reaches_gameplay_only_as_releases_on_resume() {
        let mut input = live();
        input.step(&[key(1.0, SCANCODE_Z, true)], ms(2.0), &Linear);
        input.suspend();
        assert!(input.is_suspended());
        let followed = input.follow_suspended(
            &[
                key(3.0, SCANCODE_Z, false),
                key(4.0, SCANCODE_X, true),
                moved(5.0, 164.0, 156.0),
            ],
            |_, _| false,
        );
        assert_eq!(followed, None);
        // Gameplay still holds Z and has the cursor where it was.
        assert_eq!(input.held(), Actions::LEFT);
        assert_eq!(input.position(), Vec2::ZERO);
        input.resume();
        let step = input.step(&[], ms(10.0), &Linear);
        // Z's release comes at the time gameplay stopped; X, pressed while paused, does not.
        assert_eq!(summary(&step), [(2.0, Actions::LEFT, false)]);
        assert_eq!(step.held, Actions::NONE);
        assert_eq!(step.position, Vec2::new(100.0, 100.0));
        // X's release later is dropped.
        let step = input.step(&[key(11.0, SCANCODE_X, false)], ms(12.0), &Linear);
        assert!(step.events.is_empty());
    }

    #[test]
    fn the_press_that_resumes_is_held_without_a_press() {
        let mut input = live();
        input.step(&[moved(1.0, 164.0, 156.0)], ms(1.0), &Linear);
        input.suspend();
        let events = [
            key(3.0, SCANCODE_C, true),
            mouse(4.0, MouseButton::Left, true),
            key(5.0, SCANCODE_X, true),
        ];
        let mut offered = Vec::new();
        let at = input.follow_suspended(&events, |actions, position| {
            offered.push((actions, position));
            actions.contains(Actions::LEFT)
        });
        assert_eq!(at, Some(1));
        assert_eq!(
            offered,
            [
                (Actions::SMOKE, Vec2::new(100.0, 100.0)),
                (Actions::LEFT, Vec2::new(100.0, 100.0))
            ]
        );
        input.resume();
        let step = input.step(&events[2..], ms(6.0), &Linear);
        assert_eq!(summary(&step), [(5.0, Actions::RIGHT, true)]);
        assert_eq!(step.held, Actions::LEFT | Actions::RIGHT);
        // Releasing the mouse button releases the left button as usual.
        let step = input.step(&[mouse(7.0, MouseButton::Left, false)], ms(8.0), &Linear);
        assert_eq!(summary(&step), [(7.0, Actions::LEFT, false)]);
    }

    #[test]
    fn events_waiting_at_a_suspension_happen_while_suspended() {
        let mut input = live();
        // More presses than a step takes: the rest wait.
        let mut events = Vec::new();
        for k in 0..20 {
            let t = 1.0 + k as f64 * 0.01;
            events.push(key(t, SCANCODE_Z, true));
            events.push(key(t, SCANCODE_X, true));
            events.push(key(t + 0.005, SCANCODE_Z, false));
            events.push(key(t + 0.005, SCANCODE_X, false));
        }
        input.step(&events, ms(2.0), &Linear);
        input.suspend();
        input.resume();
        let step = input.step(&[], ms(3.0), &Linear);
        // Everything ended released; the waiting events were not delivered.
        assert_eq!(step.held, Actions::NONE);
        assert!(step.events.iter().all(|e| !e.pressed));
    }

    fn wheel(t: f64) -> InputEvent {
        InputEvent {
            time_ns: ms(t),
            kind: InputKind::MouseWheel { y: -1.0 },
        }
    }

    /// The presses and releases of a few steps over `events`, the last steps empty.
    fn steps(events: &[InputEvent]) -> Vec<(f64, Actions, bool)> {
        let mut input = live();
        let mut all = summary(&input.step(events, ms(2.0), &Linear));
        for t in 3..8 {
            all.extend(summary(&input.step(&[], ms(t as f64), &Linear)));
        }
        all
    }

    #[test]
    fn the_wheel_changes_nothing_even_when_events_wait() {
        // More presses than a step takes: the rest wait for later steps.
        let mut plain = Vec::new();
        let mut scrolled = Vec::new();
        for k in 0..40 {
            let t = 1.0 + k as f64 * 0.01;
            let presses = [
                key(t, SCANCODE_Z, true),
                key(t, SCANCODE_X, true),
                key(t + 0.005, SCANCODE_Z, false),
                key(t + 0.005, SCANCODE_X, false),
            ];
            plain.extend(presses);
            for press in presses {
                scrolled.push(wheel(t));
                scrolled.push(press);
            }
        }
        // Enough scrolls to fill the waiting queue many times over if they took slots.
        scrolled.extend((0..2 * MAX_PENDING_EVENTS).map(|_| wheel(1.9)));
        scrolled.push(key(1.95, SCANCODE_Z, true));
        plain.push(key(1.95, SCANCODE_Z, true));
        let expected = steps(&plain);
        assert!(
            expected.last().is_some_and(|e| e.2),
            "the last press arrives"
        );
        assert_eq!(steps(&scrolled), expected);
    }

    #[test]
    fn the_wheel_is_not_offered_while_suspended() {
        let mut input = live();
        input.suspend();
        let mut offered = 0;
        let stopped = input.follow_suspended(&[wheel(1.0), wheel(2.0)], |_, _| {
            offered += 1;
            true
        });
        assert_eq!((stopped, offered), (None, 0));
    }

    #[test]
    fn mouse_presses_while_suspended_stay_disabled() {
        let mut input = live();
        input.set_mouse_buttons_disabled(true);
        input.suspend();
        let mut offered = 0;
        input.follow_suspended(&[mouse(1.0, MouseButton::Left, true)], |_, _| {
            offered += 1;
            true
        });
        assert_eq!(offered, 0);
    }

    #[test]
    fn a_reset_forgets_held_inputs_and_time() {
        let mut input = live();
        input.step(&[key(1.0, SCANCODE_Z, true)], ms(500.0), &Linear);
        input.reset();
        assert_eq!(input.held(), Actions::NONE);
        // Time may go back, and Z's release is dropped.
        let step = input.step(&[key(10.0, SCANCODE_Z, false)], ms(20.0), &Linear);
        assert_eq!(step.time, 20.0);
        assert!(step.events.is_empty());
    }

    #[test]
    fn held_events_wait_for_the_next_step() {
        let mut input = live();
        input.hold(&[key(1.0, SCANCODE_Z, true)]);
        let step = input.step(&[], ms(2.0), &Linear);
        assert_eq!(summary(&step), [(1.0, Actions::LEFT, true)]);
    }

    #[test]
    fn held_repeats_and_text_take_no_waiting_slots() {
        let mut input = live();
        let mut events = vec![key(1.0, SCANCODE_Z, true)];
        for _ in 0..2 * MAX_PENDING_EVENTS {
            events.push(InputEvent {
                time_ns: ms(1.5),
                kind: InputKind::KeyRepeat {
                    scancode: SCANCODE_Z,
                },
            });
            events.push(InputEvent {
                time_ns: ms(1.5),
                kind: InputKind::Text(slam_input::TextChunk::new("z").unwrap()),
            });
        }
        events.push(key(2.0, SCANCODE_Z, false));
        input.hold(&events);
        assert_eq!(input.dropped(), 0);
        let step = input.step(&[], ms(3.0), &Linear);
        assert_eq!(
            summary(&step),
            [(1.0, Actions::LEFT, true), (2.0, Actions::LEFT, false)]
        );
    }

    #[test]
    fn the_path_keeps_every_position_gameplay_takes() {
        let mut input = live();
        input.step(
            &[moved(1.0, 164.0, 156.0), moved(1.5, 174.0, 156.0)],
            ms(2.0),
            &Linear,
        );
        let path: Vec<Vec2> = input.path().since(0).collect();
        assert_eq!(path, [Vec2::new(100.0, 100.0), Vec2::new(110.0, 100.0)]);
        // While suspended the window's cursor moves, gameplay's does not.
        input.suspend();
        input.follow_suspended(&[moved(3.0, 264.0, 256.0)], |_, _| false);
        assert_eq!(input.path().written(), 2);
        // The window's path starts with a jump to gameplay's cursor, then follows.
        assert_eq!(input.window_cursor(), Vec2::new(200.0, 200.0));
        assert_eq!(
            input.window_path().moves_since(0).collect::<Vec<_>>(),
            [
                (Vec2::new(110.0, 100.0), true),
                (Vec2::new(200.0, 200.0), false)
            ]
        );
        // Resuming moves gameplay's cursor where the window's went: a jump.
        input.resume();
        input.step(&[], ms(4.0), &Linear);
        assert_eq!(
            input.path().moves_since(2).collect::<Vec<_>>(),
            [(Vec2::new(200.0, 200.0), true)]
        );
        assert_eq!(input.window_cursor(), input.position());
    }
}
