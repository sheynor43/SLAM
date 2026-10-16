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
    /// Fill of a hovered song select row.
    pub row_hover_fill: Color,
    /// Fraction of the window width the song select list takes (on the right).
    pub song_panel_ratio: f32,
    /// Margin around the song select panel.
    pub song_margin: f32,
    /// Pitch of a song select row.
    pub song_row_height: f32,
    /// Gap between two drawn song select rows (inside the pitch).
    pub song_row_gap: f32,
    /// How far difficulty rows are indented.
    pub song_indent: f32,
    /// Padding between a row's edge and its text.
    pub song_text_pad: f32,
    /// Size of the artist and title, and of version texts.
    pub song_title_size: f32,
    /// Size of the creator and of the hint.
    pub song_detail_size: f32,
    /// Height of the status line above the list, and its text size.
    pub song_header_height: f32,
    pub song_status_size: f32,
    /// Margin around the results screen.
    pub results_margin: f32,
    /// Size of the beatmap line on top of the results.
    pub results_title_size: f32,
    /// Size of the rank letter.
    pub results_rank_size: f32,
    /// Size of the texts of the results' rows.
    pub results_text_size: f32,
    /// Pitch of the results' rows.
    pub results_row_height: f32,
    /// Width of each of the two columns of the results.
    pub results_panel_width: f32,
    /// Gap between the two columns.
    pub results_column_gap: f32,
    /// Colour of the rank letter of a silver rank (Hidden, Flashlight).
    pub rank_silver: Color,
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
            row_hover_fill: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.18,
            },
            song_panel_ratio: 0.55,
            song_margin: 24.0,
            song_row_height: 52.0,
            song_row_gap: 4.0,
            song_indent: 32.0,
            song_text_pad: 16.0,
            song_title_size: 20.0,
            song_detail_size: 14.0,
            song_header_height: 36.0,
            song_status_size: 22.0,
            results_margin: 24.0,
            results_title_size: 24.0,
            results_rank_size: 160.0,
            results_text_size: 22.0,
            results_row_height: 34.0,
            results_panel_width: 400.0,
            results_column_gap: 48.0,
            rank_silver: Color::rgb(0.78, 0.82, 0.88),
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
