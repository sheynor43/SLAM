//! Theme tokens: the colours and sizes the screens draw with. The colours come from a
//! built-in colour scheme, the shape tokens are shared (ADR-0117); `theme.toml` loading
//! comes later (ARCHITECTURE §12).

use slam_render::Color;

/// A built-in set of interface colours (ADR-0117).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColourScheme {
    #[default]
    Pink,
    Blue,
    Green,
    Orange,
    Mono,
}

impl ColourScheme {
    pub const ALL: [ColourScheme; 5] = [
        ColourScheme::Pink,
        ColourScheme::Blue,
        ColourScheme::Green,
        ColourScheme::Orange,
        ColourScheme::Mono,
    ];

    /// The name in settings files.
    pub fn name(self) -> &'static str {
        match self {
            ColourScheme::Pink => "pink",
            ColourScheme::Blue => "blue",
            ColourScheme::Green => "green",
            ColourScheme::Orange => "orange",
            ColourScheme::Mono => "mono",
        }
    }

    pub fn from_name(name: &str) -> Option<ColourScheme> {
        ColourScheme::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The localization key of the scheme's name.
    pub fn label_key(self) -> &'static str {
        match self {
            ColourScheme::Pink => "settings-general-colour-scheme-pink",
            ColourScheme::Blue => "settings-general-colour-scheme-blue",
            ColourScheme::Green => "settings-general-colour-scheme-green",
            ColourScheme::Orange => "settings-general-colour-scheme-orange",
            ColourScheme::Mono => "settings-general-colour-scheme-mono",
        }
    }

    /// The scheme's colours: background, accent, the panels' base tint.
    fn palette(self) -> Palette {
        let rgb = Color::rgb;
        match self {
            ColourScheme::Pink => Palette {
                background: rgb(0.07, 0.07, 0.10),
                accent: rgb(1.0, 0.4, 0.7),
                panel: rgb(0.07, 0.07, 0.10),
                raised: rgb(0.12, 0.12, 0.16),
            },
            ColourScheme::Blue => Palette {
                background: rgb(0.05, 0.07, 0.11),
                accent: rgb(0.30, 0.62, 1.0),
                panel: rgb(0.06, 0.08, 0.13),
                raised: rgb(0.10, 0.13, 0.20),
            },
            ColourScheme::Green => Palette {
                background: rgb(0.05, 0.08, 0.07),
                accent: rgb(0.35, 0.85, 0.55),
                panel: rgb(0.06, 0.09, 0.08),
                raised: rgb(0.10, 0.15, 0.13),
            },
            ColourScheme::Orange => Palette {
                background: rgb(0.09, 0.07, 0.05),
                accent: rgb(1.0, 0.58, 0.22),
                panel: rgb(0.10, 0.08, 0.06),
                raised: rgb(0.16, 0.12, 0.09),
            },
            ColourScheme::Mono => Palette {
                background: rgb(0.06, 0.06, 0.06),
                accent: rgb(0.85, 0.85, 0.85),
                panel: rgb(0.07, 0.07, 0.07),
                raised: rgb(0.13, 0.13, 0.13),
            },
        }
    }
}

struct Palette {
    background: Color,
    accent: Color,
    /// Base of the translucent panels.
    panel: Color,
    /// Base of popups and the curtain: a step lighter than the panels.
    raised: Color,
}

/// `colour` at alpha `a`.
const fn alpha(colour: Color, a: f32) -> Color {
    Color { a, ..colour }
}

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
    /// Backing of the song select panel, so its texts read over the beatmap background.
    pub song_panel_fill: Color,
    /// Fill of the song select search line.
    pub song_search_fill: Color,
    /// Text of the song select key hints.
    pub song_hint_text: Color,
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
    /// Width over height of a song row's thumbnail (ADR-0127).
    pub song_thumbnail_aspect: f32,
    /// Gap between a row's edge and its thumbnail.
    pub song_thumbnail_inset: f32,
    /// How long a thumbnail fades in, in milliseconds.
    pub song_thumbnail_fade_ms: f32,
    /// Size of the artist and title, and of version texts.
    pub song_title_size: f32,
    /// Size of the creator and of the hint.
    pub song_detail_size: f32,
    /// Height of the status line above the list, and its text size.
    pub song_header_height: f32,
    pub song_status_size: f32,
    /// The info panel's difficulty bars: height, and the empty part's colour.
    pub song_bar_height: f32,
    pub song_bar_track_fill: Color,
    /// Pitch of the info panel's score rows.
    pub song_score_row_height: f32,
    /// Width of a mod tile of the mod select panel, and the gap between its columns.
    pub mod_tile_width: f32,
    pub mod_column_gap: f32,
    /// Fill over the screen under the mod select panel.
    pub mod_backdrop_fill: Color,
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
    /// Height of a settings widget row (toggle, slider, dropdown box, key binding box).
    pub widget_height: f32,
    /// Size of the texts inside widgets.
    pub widget_text_size: f32,
    /// Padding between a widget's edge and its text.
    pub widget_pad: f32,
    /// Size of a toggle's track.
    pub widget_toggle_width: f32,
    pub widget_toggle_height: f32,
    /// Gap between a toggle's track and its knob.
    pub widget_knob_pad: f32,
    /// Thickness of a slider's track.
    pub widget_track_height: f32,
    /// Size of a slider's knob.
    pub widget_knob_width: f32,
    pub widget_knob_height: f32,
    /// Width reserved for a slider's value text, and the gap before it.
    pub widget_value_width: f32,
    pub widget_value_gap: f32,
    /// Fill of a switched-on toggle and of the filled part of a slider.
    pub widget_accent_fill: Color,
    /// Fill of an open dropdown list.
    pub widget_popup_fill: Color,
    /// Margin around the settings screen, and the gap below its title.
    pub settings_margin: f32,
    /// Size of the settings screen's title.
    pub settings_title_size: f32,
    /// Width of the column of section tabs.
    pub settings_tab_width: f32,
    /// Pitch of the settings rows and of the tabs.
    pub settings_row_pitch: f32,
    /// Fraction of the content width the row labels take.
    pub settings_label_ratio: f32,
    /// Size of the "after restart" marker.
    pub settings_marker_size: f32,
    /// Width of the settings panel that slides in from the left (ADR-0095).
    pub settings_panel_width: f32,
    /// Fill of the settings panel.
    pub settings_panel_fill: Color,
    /// Fill over the screen under the open settings panel, at full opacity once it is in.
    pub settings_dim_fill: Color,
    /// How long the settings panel slides in or out, in milliseconds.
    pub settings_panel_ms: f32,
    /// Fill of the curtain's slats while the language switches (ADR-0091).
    pub curtain_fill: Color,
    /// Height of the accent edge leading each slat.
    pub curtain_edge: f32,
    /// The black behind the startup intro (ADR-0114).
    pub intro_backdrop: Color,
    /// The colour scheme the colours above come from (ADR-0117).
    pub scheme: ColourScheme,
    /// The scheme's accent, opaque: outlines of selected elements.
    pub accent: Color,
    /// Multiplies [`Theme::scale`]: the interface scale setting (ADR-0118).
    pub ui_scale: f32,
    /// Corner radius of buttons, rows and widgets (ADR-0117).
    pub button_radius: f32,
    /// Corner radius of panels.
    pub panel_radius: f32,
    /// Skew of buttons and rows in degrees, leaning right (`/`); adjustable, 0 to
    /// [`MAX_SKEW_DEGREES`] (ADR-0117).
    pub button_skew_degrees: f32,
    /// Width of the outline of a hovered or selected element.
    pub outline_width: f32,
    /// Whether the effects are on (ADR-0123): glow, sheen, the background's blur and motion.
    pub effects: bool,
    /// Strength of the sheen sweeping a selected button (0 to 1).
    pub button_sheen: f32,
    /// Sweeps of the sheen, and pulses of the glow, per second.
    pub effect_speed: f32,
    /// Width of the glow around a selected button, its alpha and how deep it pulses.
    pub glow_width: f32,
    pub glow_alpha: f32,
    pub glow_pulse: f32,
    /// How far the main menu's background zooms in and back, as a part of its size, and
    /// the seconds of one such drift (ADR-0123).
    pub background_motion: f32,
    pub background_motion_seconds: f32,
    /// Blur of the menus' background, in pixels of the window (ADR-0123).
    pub background_blur: f32,
    /// Dim over the main menu's background (ADR-0119), 0 to 1.
    pub menu_background_dim: f32,
    /// Backing of the main menu's title and buttons, so they read over the background.
    pub menu_panel_fill: Color,
    /// Margin of that backing around them.
    pub menu_panel_pad: f32,
    /// SLAM's menu cursor (ADR-0124, ADR-0128): its height in reference pixels, the fill,
    /// the outline's colour and width (a part of the height), and the tilt of its silhouette.
    pub cursor_size: f32,
    pub cursor_fill: Color,
    pub cursor_outline_colour: Color,
    pub cursor_outline: f32,
    pub cursor_tilt_degrees: f32,
    /// Press, as the lazer menu cursor: the scale it shrinks to, the milliseconds the shrink
    /// and the way back take, and the fade of the accent glow that comes with it.
    pub cursor_press_scale: f32,
    pub cursor_press_in_ms: f32,
    pub cursor_press_out_ms: f32,
    pub cursor_glow_in_ms: f32,
    pub cursor_glow_out_ms: f32,
    /// Drag rotation (right button): milliseconds of a turn towards the drag, the base of
    /// the way back (it grows with the angle), the distance the drag must reach before it
    /// turns the cursor, and the one from which its centre starts to follow (reference
    /// pixels).
    pub cursor_rotate_ms: f32,
    pub cursor_rotate_back_ms: f32,
    pub cursor_drag_start: f32,
    pub cursor_drag_float: f32,
    /// SLAM's gameplay cursor (ADR-0128, lazer's `OsuCursor`): its diameter in osu!pixels,
    /// the widths of the outer and the inner ring (parts of the diameter), the opacity of
    /// the accent fill and of the inner ring, the centre dot's size (a part of the diameter)
    /// and the width of the glow around it in osu!pixels.
    pub cursor_gameplay_size: f32,
    pub cursor_ring: f32,
    pub cursor_inner_ring: f32,
    pub cursor_ring_fill_alpha: f32,
    pub cursor_inner_ring_alpha: f32,
    pub cursor_dot: f32,
    pub cursor_gameplay_glow: f32,
}

/// Tokens of the `fancy` screen transition (ADR-0126). The `simple` one keeps the
/// constants in `screen`.
pub mod transition {
    /// Length of the transition.
    pub const FANCY_NS: u64 = 450_000_000;
    /// How far a screen slides, as a fraction of the window width.
    pub const FANCY_SLIDE: f32 = 0.15;
    /// Scale an incoming screen starts at.
    pub const FANCY_SCALE_IN: f32 = 1.06;
    /// Scale an outgoing screen ends at.
    pub const FANCY_SCALE_OUT: f32 = 0.94;
    /// Width of the accent band, as a fraction of the window width.
    pub const BAND_WIDTH: f32 = 0.22;
    /// Opacity of the band at the middle of the transition.
    pub const BAND_PEAK_OPACITY: f32 = 0.55;
}

/// Largest skew [`Theme::button_skew_degrees`] takes.
pub const MAX_SKEW_DEGREES: f32 = 20.0;

/// Range of the interface scale settings (ADR-0118).
pub const UI_SCALE_RANGE: (f32, f32) = (0.75, 1.5);

impl Default for Theme {
    fn default() -> Theme {
        Theme::new(ColourScheme::default(), 1.0)
    }
}

impl Theme {
    /// The theme in `scheme`'s colours at interface scale `ui_scale` (clamped to
    /// [`UI_SCALE_RANGE`]).
    pub fn new(scheme: ColourScheme, ui_scale: f32) -> Theme {
        let p = scheme.palette();
        let ui_scale = if ui_scale.is_finite() {
            ui_scale.clamp(UI_SCALE_RANGE.0, UI_SCALE_RANGE.1)
        } else {
            1.0
        };
        Theme {
            background: p.background,
            // Dense enough to read over a beatmap background (ADR-0119).
            button_fill: alpha(p.raised, 0.78),
            button_hover_fill: alpha(p.accent, 0.55),
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
            song_panel_fill: alpha(p.panel, 0.78),
            song_search_fill: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.16,
            },
            song_hint_text: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.7,
            },
            song_margin: 24.0,
            song_row_height: 52.0,
            song_row_gap: 4.0,
            song_indent: 32.0,
            song_text_pad: 16.0,
            song_thumbnail_aspect: 1.6,
            song_thumbnail_inset: 3.0,
            song_thumbnail_fade_ms: 200.0,
            song_title_size: 20.0,
            song_detail_size: 14.0,
            song_header_height: 36.0,
            song_status_size: 22.0,
            song_bar_height: 6.0,
            song_bar_track_fill: Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.16,
            },
            song_score_row_height: 38.0,
            mod_tile_width: 150.0,
            mod_column_gap: 32.0,
            mod_backdrop_fill: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.8,
            },
            results_margin: 24.0,
            results_title_size: 24.0,
            results_rank_size: 160.0,
            results_text_size: 22.0,
            results_row_height: 34.0,
            results_panel_width: 400.0,
            results_column_gap: 48.0,
            rank_silver: Color::rgb(0.78, 0.82, 0.88),
            widget_height: 40.0,
            widget_text_size: 20.0,
            widget_pad: 12.0,
            widget_toggle_width: 52.0,
            widget_toggle_height: 28.0,
            widget_knob_pad: 3.0,
            widget_track_height: 6.0,
            widget_knob_width: 14.0,
            widget_knob_height: 26.0,
            widget_value_width: 84.0,
            widget_value_gap: 16.0,
            widget_accent_fill: alpha(p.accent, 0.85),
            widget_popup_fill: alpha(p.raised, 0.97),
            settings_margin: 32.0,
            settings_title_size: 40.0,
            settings_tab_width: 170.0,
            settings_row_pitch: 52.0,
            settings_label_ratio: 0.45,
            settings_marker_size: 14.0,
            settings_panel_width: 880.0,
            settings_panel_fill: alpha(p.panel, 0.9),
            settings_dim_fill: Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.5,
            },
            settings_panel_ms: 250.0,
            curtain_fill: p.raised,
            curtain_edge: 4.0,
            intro_backdrop: Color::rgb(0.0, 0.0, 0.0),
            scheme,
            accent: p.accent,
            ui_scale,
            button_radius: 8.0,
            panel_radius: 14.0,
            button_skew_degrees: 10.0,
            outline_width: 2.0,
            effects: true,
            button_sheen: 0.35,
            effect_speed: 0.3,
            glow_width: 14.0,
            glow_alpha: 0.55,
            glow_pulse: 0.35,
            background_motion: 0.06,
            background_motion_seconds: 40.0,
            background_blur: 12.0,
            menu_background_dim: 0.5,
            menu_panel_fill: alpha(p.panel, 0.62),
            menu_panel_pad: 28.0,
            cursor_size: 30.0,
            cursor_fill: Color::rgb(0.96, 0.97, 1.0),
            cursor_outline_colour: Color {
                r: 0.04,
                g: 0.05,
                b: 0.09,
                a: 0.9,
            },
            cursor_outline: 0.055,
            cursor_tilt_degrees: 24.3,
            cursor_press_scale: 0.9,
            cursor_press_in_ms: 800.0,
            cursor_press_out_ms: 500.0,
            cursor_glow_in_ms: 800.0,
            cursor_glow_out_ms: 500.0,
            cursor_rotate_ms: 120.0,
            cursor_rotate_back_ms: 400.0,
            cursor_drag_start: 80.0,
            cursor_drag_float: 60.0,
            cursor_gameplay_size: 28.0,
            cursor_ring: 1.0 / 6.0,
            cursor_inner_ring: 1.0 / 3.0,
            cursor_ring_fill_alpha: 0.55,
            cursor_inner_ring_alpha: 0.5,
            cursor_dot: 0.14,
            cursor_gameplay_glow: 5.0,
        }
    }

    /// The same theme with the effects on or off (ADR-0123).
    pub fn with_effects(self, effects: bool) -> Theme {
        Theme { effects, ..self }
    }

    /// The same theme at another interface scale: the in-game overlays take the game's.
    pub fn with_ui_scale(self, ui_scale: f32) -> Theme {
        Theme {
            ui_scale: Theme::new(self.scheme, ui_scale).ui_scale,
            ..self
        }
    }

    /// Factor from reference sizes to pixels on a window `height` pixels high.
    pub fn scale(&self, height: f32) -> f32 {
        height / self.reference_height * self.ui_scale
    }

    /// The skew of buttons and rows in radians, within `[0, MAX_SKEW_DEGREES]`.
    pub fn button_skew(&self) -> f32 {
        self.button_skew_degrees
            .clamp(0.0, MAX_SKEW_DEGREES)
            .to_radians()
    }
}

/// `colour` with its alpha multiplied by `opacity`.
pub fn faded(colour: Color, opacity: f32) -> Color {
    Color {
        a: colour.a * opacity,
        ..colour
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_names_round_trip() {
        for scheme in ColourScheme::ALL {
            assert_eq!(ColourScheme::from_name(scheme.name()), Some(scheme));
        }
        assert_eq!(ColourScheme::from_name("purple"), None);
    }

    #[test]
    fn the_default_keeps_the_pink_colours() {
        let theme = Theme::default();
        assert_eq!(theme.scheme, ColourScheme::Pink);
        assert_eq!(
            theme.button_hover_fill,
            Color {
                a: 0.55,
                ..Color::rgb(1.0, 0.4, 0.7)
            }
        );
        assert_eq!(theme.background, Color::rgb(0.07, 0.07, 0.10));
    }

    #[test]
    fn schemes_differ_in_their_accent() {
        let accents: Vec<_> = ColourScheme::ALL.map(|s| Theme::new(s, 1.0).accent).into();
        for (i, a) in accents.iter().enumerate() {
            assert!(accents[i + 1..].iter().all(|b| b != a));
        }
    }

    #[test]
    fn the_interface_scale_multiplies_the_scale_within_its_range() {
        assert_eq!(Theme::new(ColourScheme::Pink, 1.25).scale(768.0), 1.25);
        assert_eq!(
            Theme::new(ColourScheme::Pink, 9.0).ui_scale,
            UI_SCALE_RANGE.1
        );
        assert_eq!(Theme::new(ColourScheme::Pink, f32::NAN).ui_scale, 1.0);
        let game = Theme::default().with_ui_scale(0.8);
        assert_eq!(game.scale(768.0), 0.8);
        assert_eq!(game.scheme, ColourScheme::Pink);
    }

    #[test]
    fn the_skew_is_held_to_its_range() {
        let mut theme = Theme::default();
        assert!((theme.button_skew() - 10f32.to_radians()).abs() < 1e-6);
        theme.button_skew_degrees = 45.0;
        assert!((theme.button_skew() - MAX_SKEW_DEGREES.to_radians()).abs() < 1e-6);
        theme.button_skew_degrees = -5.0;
        assert_eq!(theme.button_skew(), 0.0);
    }
}
