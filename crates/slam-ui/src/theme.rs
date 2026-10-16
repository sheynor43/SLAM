//! Theme tokens: the colours and sizes the screens draw with. Only the built-in defaults
//! for now; `theme.toml` loading comes with the settings (ARCHITECTURE §12).

use slam_render::Color;

/// The tokens menus read. Sizes are given at [`Theme::reference_height`] and scale with the
/// window height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    /// Window clear colour.
    pub background: Color,
    pub button_fill: Color,
    /// Fill of the selected or hovered button.
    pub button_hover_fill: Color,
    pub text: Color,
    /// Text of a button that cannot be used.
    pub disabled_text: Color,
    /// Window height the sizes below are given at.
    pub reference_height: f32,
    /// Font weight of every text.
    pub font_weight: u16,
    pub title_size: f32,
    pub button_text_size: f32,
    /// Text of a screen that is only a message.
    pub message_size: f32,
    pub button_width: f32,
    pub button_height: f32,
    /// Gap between two buttons.
    pub button_spacing: f32,
    /// Gap between the title and the first button.
    pub title_gap: f32,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme {
            background: Color::rgb(0.07, 0.07, 0.10),
            button_fill: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.08,
            },
            button_hover_fill: Color {
                r: 1.0,
                g: 0.4,
                b: 0.7,
                a: 0.55,
            },
            text: Color::WHITE,
            disabled_text: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.35,
            },
            reference_height: 768.0,
            font_weight: 700,
            title_size: 96.0,
            button_text_size: 28.0,
            message_size: 28.0,
            button_width: 320.0,
            button_height: 56.0,
            button_spacing: 12.0,
            title_gap: 48.0,
        }
    }
}

impl Theme {
    /// Factor from reference sizes to pixels on a window `height` pixels high.
    pub fn scale(&self, height: f32) -> f32 {
        height / self.reference_height
    }
}

/// `colour` with its alpha multiplied by `opacity`.
pub fn faded(colour: Color, opacity: f32) -> Color {
    Color {
        a: colour.a * opacity,
        ..colour
    }
}
