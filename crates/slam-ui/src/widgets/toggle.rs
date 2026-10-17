//! A boolean switch.

use slam_render::atlas::AtlasRegion;
use slam_render::sprite::SpriteBatch;

use super::fill;
use crate::menu::Rect;
use crate::theme::{Theme, faded};

/// What a toggle understands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToggleInput {
    /// Keyboard activation (Enter, Space).
    Activate,
    /// Primary button pressed at `x`, `y` in layout units.
    PointerPress { x: f32, y: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleAction {
    /// The value changed to this.
    Changed(bool),
}

/// Where a toggle is: its whole `bounds` are clickable, the switch is at the right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToggleLayout {
    pub bounds: Rect,
    pub track: Rect,
    pub scale: f32,
}

impl ToggleLayout {
    /// `scale` is pixels per reference unit (see [`Theme::scale`]).
    pub fn new(bounds: Rect, theme: &Theme, scale: f32) -> ToggleLayout {
        let (width, height) = (
            theme.widget_toggle_width * scale,
            theme.widget_toggle_height * scale,
        );
        ToggleLayout {
            bounds,
            track: Rect {
                x: bounds.x + bounds.width - width,
                y: bounds.y + (bounds.height - height) / 2.0,
                width,
                height,
            },
            scale,
        }
    }
}

/// A toggle's state on the update thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Toggle {
    value: bool,
}

impl Toggle {
    pub fn new(value: bool) -> Toggle {
        Toggle { value }
    }

    pub fn value(&self) -> bool {
        self.value
    }

    /// Sets the value without reporting an action.
    pub fn set(&mut self, value: bool) {
        self.value = value;
    }

    /// Applies `input`. Does not allocate.
    pub fn input(&mut self, input: ToggleInput, layout: &ToggleLayout) -> Option<ToggleAction> {
        let hit = match input {
            ToggleInput::Activate => true,
            ToggleInput::PointerPress { x, y } => layout.bounds.contains(x, y),
        };
        if !hit {
            return None;
        }
        self.value = !self.value;
        Some(ToggleAction::Changed(self.value))
    }
}

/// The draw side of [`Toggle`]: shapes only.
#[derive(Clone, Copy, Debug)]
pub struct ToggleView {
    theme: Theme,
}

impl ToggleView {
    pub fn new(theme: Theme) -> ToggleView {
        ToggleView { theme }
    }

    /// Writes the switch into `shapes`. Does not allocate once the batch has room.
    pub fn push(
        &self,
        shapes: &mut SpriteBatch,
        toggle: &Toggle,
        layout: &ToggleLayout,
        opacity: f32,
        white: AtlasRegion,
    ) {
        let theme = &self.theme;
        let track_fill = if toggle.value {
            theme.widget_accent_fill
        } else {
            theme.button_fill
        };
        fill(shapes, white, &layout.track, faded(track_fill, opacity));
        let pad = theme.widget_knob_pad * layout.scale;
        let size = layout.track.height - 2.0 * pad;
        let travel = layout.track.width - 2.0 * pad - size;
        let knob = Rect {
            x: layout.track.x + pad + if toggle.value { travel } else { 0.0 },
            y: layout.track.y + pad,
            width: size,
            height: size,
        };
        fill(shapes, white, &knob, faded(theme.text, opacity));
    }
}
