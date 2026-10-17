//! A key binding box: shows the current binding, captures the next key when asked.

use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::sprite::SpriteBatch;
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use super::{fill, push_left};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::theme::{Theme, faded};

/// What a key binding box understands. `K` is the application's key type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyBindInput<K> {
    /// Keyboard activation: starts listening.
    Activate,
    /// Escape: stops listening without binding.
    Back,
    /// A key the application captured while the box listens.
    Captured(K),
    /// Primary button pressed at `x`, `y`: inside starts listening, outside stops it.
    PointerPress { x: f32, y: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyBindAction<K> {
    StartedListening,
    Bound(K),
    Cancelled,
}

/// Where a key binding box is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyBindLayout {
    pub bounds: Rect,
    pub scale: f32,
}

impl KeyBindLayout {
    pub fn new(bounds: Rect, scale: f32) -> KeyBindLayout {
        KeyBindLayout { bounds, scale }
    }
}

/// A key binding box's state on the update thread. The bound key itself belongs to the
/// application; this only tracks whether the box waits for one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyBind {
    listening: bool,
}

impl KeyBind {
    pub fn new() -> KeyBind {
        KeyBind::default()
    }

    pub fn is_listening(&self) -> bool {
        self.listening
    }

    /// Applies `input`. Does not allocate.
    pub fn input<K: Copy>(
        &mut self,
        input: KeyBindInput<K>,
        layout: &KeyBindLayout,
    ) -> Option<KeyBindAction<K>> {
        match input {
            KeyBindInput::Activate => self.start(),
            KeyBindInput::PointerPress { x, y } => {
                if layout.bounds.contains(x, y) {
                    self.start()
                } else {
                    self.stop(KeyBindAction::Cancelled)
                }
            }
            KeyBindInput::Back => self.stop(KeyBindAction::Cancelled),
            KeyBindInput::Captured(key) => self.stop(KeyBindAction::Bound(key)),
        }
    }

    fn start<K>(&mut self) -> Option<KeyBindAction<K>> {
        if self.listening {
            return None;
        }
        self.listening = true;
        Some(KeyBindAction::StartedListening)
    }

    fn stop<K>(&mut self, action: KeyBindAction<K>) -> Option<KeyBindAction<K>> {
        if !self.listening {
            return None;
        }
        self.listening = false;
        Some(action)
    }
}

/// The draw side of [`KeyBind`].
pub struct KeyBindView {
    theme: Theme,
    type_setter: Typesetter,
    scale: f32,
    label: Line,
    prompt: Line,
}

impl std::fmt::Debug for KeyBindView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyBindView")
            .field("label", &self.label.text)
            .finish_non_exhaustive()
    }
}

impl KeyBindView {
    /// `prompt` is the localised "press a key" text shown while listening, `label` the
    /// current binding. Lays the texts out once: call it off the hot paths.
    pub fn new(
        registry: Arc<FontRegistry>,
        theme: Theme,
        prompt: &str,
        label: &str,
    ) -> KeyBindView {
        let mut view = KeyBindView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            scale: 1.0,
            label: Line::new(label),
            prompt: Line::new(prompt),
            theme,
        };
        view.lay_out();
        view
    }

    /// Shows `label` as the current binding. Lays it out again, so call it when the
    /// binding changes, not every frame.
    pub fn set_label(&mut self, label: &str) {
        self.label.text.clear();
        self.label.text.push_str(label);
        let size = self.theme.widget_text_size * self.scale;
        self.type_setter.lay_out(&mut self.label, size);
    }

    fn lay_out(&mut self) {
        let size = self.theme.widget_text_size * self.scale;
        self.type_setter.lay_out(&mut self.label, size);
        self.type_setter.lay_out(&mut self.prompt, size);
    }

    /// Asks `glyphs` for every glyph the box shows. Returns how many are not ready yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        [&self.label, &self.prompt]
            .into_iter()
            .flat_map(|line| line.layout.glyphs())
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
            .count()
    }

    /// Writes the box into `shapes` and its text into `text`. Allocation-free once warm.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut SpriteBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        state: &KeyBind,
        layout: &KeyBindLayout,
        opacity: f32,
        white: AtlasRegion,
    ) {
        if layout.scale != self.scale {
            self.scale = layout.scale;
            self.lay_out();
        }
        let theme = &self.theme;
        let (box_fill, line) = if state.listening {
            (theme.button_hover_fill, &self.prompt)
        } else {
            (theme.button_fill, &self.label)
        };
        fill(shapes, white, &layout.bounds, faded(box_fill, opacity));
        let pad = theme.widget_pad * layout.scale;
        push_left(
            text,
            line,
            &layout.bounds,
            pad,
            faded(theme.text, opacity),
            glyphs,
        );
    }
}
