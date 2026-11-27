//! The toolbar of the menus (ADR-0168): a thin strip across the top with the settings and
//! home buttons on the left, and the clock and the pinned settings on the right. A pinned
//! setting is an icon of its section; a click opens a popup under it with that one row.
//!
//! The update side ([`Toolbar`]) keeps the hover and the popup's widget and puts a
//! [`ToolbarFrame`] into the snapshot; the draw side ([`ToolbarView`]) draws it, taking the
//! rows' texts and widgets from the settings view. Neither allocates after it is made.

use std::fmt::Write as _;
use std::sync::Arc;

use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::{Affine, SpriteBatch};
use slam_text::{FontRegistry, Glyph, GlyphCache, GlyphKey};

use crate::anim::{ease_out_cubic, ease_out_quint, progress};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::settings_screen::icons;
use crate::settings_screen::panel::panel_width;
use crate::settings_screen::{
    RowValue, RowWidget, SettingsInput, SettingsLayout, SettingsView, route_value,
};
use crate::theme::{Theme, faded};
use crate::widgets::{Look, draw, push_left, push_right};

mod player;

pub use player::{NowPlayingFrame, NowPlayingText, PlayerAction, PlayerFrame, PlayerRects};
use player::{PlayerOpen, PlayerView};

/// Most settings that can be pinned.
pub const MAX_PINS: usize = 8;

/// Clock value for an unknown time: nothing is drawn.
pub const NO_CLOCK: u16 = u16::MAX;

/// Something on the toolbar a pointer can be over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Settings,
    Home,
    /// The music player's button (ADR-0178).
    Player,
    /// The pinned setting at this position.
    Pin(usize),
}

/// Where the toolbar's parts are: a pure function of the window size, theme and the number
/// of pins, shared by hit testing (update) and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarLayout {
    pub bar: Rect,
    pub settings: Rect,
    pub home: Rect,
    /// The music player's button, left of the pins.
    pub player: Rect,
    pub clock: Rect,
    pins: [Rect; MAX_PINS],
    pin_count: usize,
    /// Width of a popup's row: the settings panel's.
    row_width: f32,
    window: [f32; 2],
    theme: Theme,
    pub scale: f32,
}

impl ToolbarLayout {
    pub fn new(width: f32, height: f32, theme: &Theme, pin_count: usize) -> ToolbarLayout {
        let scale = theme.scale(height);
        let side = theme.toolbar_height * scale;
        let gap = theme.toolbar_gap * scale;
        let pin_count = pin_count.min(MAX_PINS);
        let bar = Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: side,
        };
        let button = |i: usize| Rect {
            x: i as f32 * (side + gap),
            y: 0.0,
            width: side,
            height: side,
        };
        let pad = theme.widget_pad * scale;
        let clock_width = theme.toolbar_clock_size * scale * 3.2 + 2.0 * pad;
        let clock = Rect {
            x: (width - clock_width - pad).max(0.0),
            y: 0.0,
            width: clock_width,
            height: side,
        };
        let mut pins = [Rect::default(); MAX_PINS];
        for (i, pin) in pins.iter_mut().enumerate().take(pin_count) {
            *pin = Rect {
                x: clock.x - (pin_count + 1 - i) as f32 * (side + gap),
                y: 0.0,
                width: side,
                height: side,
            };
        }
        let player = Rect {
            x: clock.x - (side + gap),
            y: 0.0,
            width: side,
            height: side,
        };
        let row_width =
            SettingsLayout::new(panel_width(width, height, theme), height, theme).content_width();
        ToolbarLayout {
            bar,
            settings: button(0),
            home: button(1),
            player,
            clock,
            pins,
            pin_count,
            row_width,
            window: [width, height],
            theme: *theme,
            scale,
        }
    }

    /// The bar's height in pixels: what the menus below it make room for.
    pub fn height(&self) -> f32 {
        self.bar.height
    }

    /// The pins' icon buttons, left to right.
    pub fn pins(&self) -> &[Rect] {
        &self.pins[..self.pin_count]
    }

    /// The button or pin under the point.
    pub fn target_at(&self, x: f32, y: f32) -> Option<Target> {
        if self.settings.contains(x, y) {
            return Some(Target::Settings);
        }
        if self.home.contains(x, y) {
            return Some(Target::Home);
        }
        if self.player.contains(x, y) {
            return Some(Target::Player);
        }
        self.pins()
            .iter()
            .position(|pin| pin.contains(x, y))
            .map(Target::Pin)
    }

    /// The popup of pin `pin`: under its icon, within the window.
    pub fn popup(&self, pin: usize) -> Rect {
        let icon = self.pins.get(pin).copied().unwrap_or_default();
        let pad = self.popup_pad();
        let width = self.row_width + 2.0 * pad;
        let height = self.theme.widget_height * self.scale + 2.0 * pad;
        let gap = self.theme.toolbar_gap * self.scale * 2.0;
        Rect {
            x: (icon.x + icon.width - width).clamp(0.0, (self.window[0] - width).max(0.0)),
            y: self.bar.height + gap,
            width,
            height,
        }
    }

    fn popup_pad(&self) -> f32 {
        self.theme.widget_pad * self.scale * 2.0
    }

    /// The layout of the one row in the popup of pin `pin`, moved `dy` down.
    pub fn row(&self, pin: usize, dy: f32) -> SettingsLayout {
        let popup = self.popup(pin);
        let pad = self.popup_pad();
        let row = Rect {
            x: popup.x + pad,
            y: popup.y + pad + dy,
            width: popup.width - 2.0 * pad,
            height: popup.height - 2.0 * pad,
        };
        SettingsLayout::single(row, self.window[1], &self.theme)
    }
}

/// What the toolbar asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolbarEvent {
    /// The press or release was the toolbar's, and nothing is asked.
    Absorbed,
    OpenSettings,
    Home,
    /// The pin at this position was clicked: the application shows its row with
    /// [`Toolbar::show_popup`].
    OpenPopup {
        pin: usize,
    },
    /// The popup went; the application commits what it changed.
    ClosePopup,
    /// The player's button was clicked: the application shows its popup with
    /// [`Toolbar::show_player`].
    OpenPlayer,
    /// The player's popup went.
    ClosePlayer,
    /// A press or drag in the player's popup.
    Player(PlayerAction),
    /// The popup's widget changed row `row` to `value`.
    Changed {
        row: usize,
        value: RowValue,
    },
}

/// The popup under a pin, with its row's widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupFrame {
    pub pin: u8,
    /// The settings screen's row index.
    pub row: u16,
    pub widget: RowWidget,
    pub since_ns: u64,
}

/// What the draw side needs; `Copy`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarFrame {
    pub visible: bool,
    pub hover: Option<Target>,
    /// The pinned rows' indices on the settings screen, `pin_count` of them.
    pub pins: [u16; MAX_PINS],
    pub pin_count: u8,
    pub popup: Option<PopupFrame>,
    /// The local time as minutes into the day, or [`NO_CLOCK`].
    pub clock: u16,
    /// How far the bar is in, 0 (above the window) ..= 1 (in place): it slides in with the
    /// main menu after the intro and out and in around the results (ADR-0178).
    pub slide: f32,
    /// `slide` follows a running screen transition (ADR-0126): drawn as it is, not eased
    /// again, and the bar fades with it.
    pub follows: bool,
    /// The music player's popup, when open.
    pub player: Option<PlayerFrame>,
    /// What plays.
    pub now_playing: NowPlayingFrame,
}

impl Default for ToolbarFrame {
    fn default() -> ToolbarFrame {
        ToolbarFrame {
            visible: false,
            hover: None,
            pins: [0; MAX_PINS],
            pin_count: 0,
            popup: None,
            clock: NO_CLOCK,
            slide: 1.0,
            follows: false,
            player: None,
            now_playing: NowPlayingFrame::default(),
        }
    }
}

/// How far the bar is in, easing towards shown or hidden over the theme's
/// `toolbar_slide_ms` (ADR-0178). The value is linear progress, 0 hidden ..= 1 in place;
/// the draw side eases it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    from: f32,
    to: f32,
    since_ns: u64,
    now: f32,
}

impl Default for Slide {
    /// Out of sight.
    fn default() -> Slide {
        Slide {
            from: 0.0,
            to: 0.0,
            since_ns: 0,
            now: 0.0,
        }
    }
}

impl Slide {
    /// Takes `value` from a running screen transition, which drives the bar; `shown` is
    /// where the bar is headed, so that it carries on from there once the transition ends.
    pub fn follow(&mut self, value: f32, shown: bool) -> f32 {
        self.from = value;
        self.to = if shown { 1.0 } else { 0.0 };
        self.since_ns = 0;
        self.now = value;
        value
    }

    /// Moves towards `shown` at `now_ns`; a change of direction turns from where it is.
    /// Returns how far in the bar is.
    pub fn update(&mut self, shown: bool, now_ns: u64, theme: &Theme) -> f32 {
        let to = if shown { 1.0 } else { 0.0 };
        if to != self.to {
            self.from = self.now;
            self.to = to;
            self.since_ns = now_ns;
        }
        let ms = (f64::from(theme.toolbar_slide_ms.max(0.0)) * 1e6) as u64;
        let t = if theme.effects {
            progress(self.since_ns, now_ns, ms)
        } else {
            1.0
        };
        self.now = self.from + (self.to - self.from) * t;
        self.now
    }
}

/// The toolbar on the update thread.
#[derive(Clone, Copy, Debug)]
pub struct Toolbar {
    pins: [u16; MAX_PINS],
    pin_count: usize,
    hover: Option<Target>,
    popup: Option<PopupFrame>,
    player: Option<PlayerOpen>,
}

impl Default for Toolbar {
    fn default() -> Toolbar {
        Toolbar::new()
    }
}

impl Toolbar {
    pub fn new() -> Toolbar {
        Toolbar {
            pins: [0; MAX_PINS],
            pin_count: 0,
            hover: None,
            popup: None,
            player: None,
        }
    }

    /// The pinned rows' indices on the settings screen.
    pub fn pins(&self) -> &[u16] {
        &self.pins[..self.pin_count]
    }

    /// Sets the pinned rows, the first [`MAX_PINS`] of them. A popup of a pin that is no
    /// longer the same row goes; returns whether one did.
    pub fn set_pins(&mut self, rows: &[u16]) -> bool {
        let count = rows.len().min(MAX_PINS);
        self.pins[..count].copy_from_slice(&rows[..count]);
        self.pin_count = count;
        self.hover = None;
        let stale = self
            .popup
            .is_some_and(|p| self.pins().get(usize::from(p.pin)) != Some(&p.row));
        if stale {
            self.popup = None;
        }
        stale
    }

    /// The row whose popup is open.
    pub fn popup_row(&self) -> Option<usize> {
        self.popup.map(|p| usize::from(p.row))
    }

    pub fn popup_open(&self) -> bool {
        self.popup.is_some()
    }

    /// Opens the popup of pin `pin` with the widget of its row.
    pub fn show_popup(&mut self, pin: usize, widget: RowWidget, now_ns: u64) {
        if let Some(row) = self.pins().get(pin).copied() {
            self.popup = Some(PopupFrame {
                pin: pin as u8,
                row,
                widget,
                since_ns: now_ns,
            });
        }
    }

    /// Closes the popup, if open.
    pub fn close_popup(&mut self) {
        self.popup = None;
    }

    /// The popup's widget, for the application to refresh.
    pub fn popup_widget_mut(&mut self) -> Option<&mut RowWidget> {
        self.popup.as_mut().map(|p| &mut p.widget)
    }

    fn layout(&self, layout: &ToolbarLayout) -> Option<(PopupFrame, SettingsLayout)> {
        self.popup.map(|p| (p, layout.row(usize::from(p.pin), 0.0)))
    }

    /// Whether the point is on the popup, including its open dropdown list.
    fn on_popup(&self, x: f32, y: f32, layout: &ToolbarLayout) -> bool {
        let Some((popup, row)) = self.layout(layout) else {
            return false;
        };
        if layout.popup(usize::from(popup.pin)).contains(x, y) {
            return true;
        }
        matches!(popup.widget, RowWidget::Dropdown(d) if d.is_open()
            && row.dropdown_at(0, d.count()).list().contains(x, y))
    }

    /// Follows the pointer at `x`, `y`: the hover, and a slider's drag or a dropdown's
    /// highlight in the popup.
    pub fn pointer(&mut self, x: f32, y: f32, layout: &ToolbarLayout) -> Option<ToolbarEvent> {
        self.hover = layout.target_at(x, y);
        if let Some(event) = self.player_pointer(x, y, layout) {
            return Some(event);
        }
        self.route(SettingsInput::PointerMove { x, y }, layout)
    }

    fn route(&mut self, input: SettingsInput, layout: &ToolbarLayout) -> Option<ToolbarEvent> {
        let popup = self.popup.as_mut()?;
        let row = layout.row(usize::from(popup.pin), 0.0);
        let value = route_value(&mut popup.widget, input, &row, 0)?;
        Some(ToolbarEvent::Changed {
            row: usize::from(popup.row),
            value,
        })
    }

    /// Whether the popup's slider is being typed into (ADR-0173).
    pub fn is_editing(&self) -> bool {
        matches!(&self.popup, Some(p) if matches!(&p.widget, RowWidget::Slider(s) if s.is_editing()))
    }

    /// A key for the popup's slider: Enter starts typing its value or commits it, typed
    /// characters, Backspace and Ctrl+Backspace (`Type`, `Erase`, `EraseWord`) edit it while
    /// typing. `None` when the toolbar does not take the key; a taken key that changed
    /// nothing is [`ToolbarEvent::Absorbed`].
    pub fn key(&mut self, input: SettingsInput, layout: &ToolbarLayout) -> Option<ToolbarEvent> {
        let slider = matches!(&self.popup, Some(p) if matches!(p.widget, RowWidget::Slider(_)));
        let taken = match input {
            SettingsInput::Activate => slider,
            SettingsInput::Type(_) | SettingsInput::Erase | SettingsInput::EraseWord => {
                self.is_editing()
            }
            _ => false,
        };
        taken.then(|| self.route(input, layout).unwrap_or(ToolbarEvent::Absorbed))
    }

    /// A press of the primary button at `x`, `y`: `None` when it is not the toolbar's.
    /// With a popup open every press is: inside it goes to the widget, outside closes it. A
    /// press outside ends the typing of a value first; when that changed the value it is
    /// all the press does.
    pub fn press(&mut self, x: f32, y: f32, layout: &ToolbarLayout) -> Option<ToolbarEvent> {
        if self.is_editing()
            && !self.on_popup(x, y, layout)
            && let Some(event) = self.route(SettingsInput::Activate, layout)
        {
            return Some(event);
        }
        let target = layout.target_at(x, y);
        if self.player.is_some() {
            return Some(
                self.player_press(x, y, layout)
                    .unwrap_or_else(|| self.player_outside(target)),
            );
        }
        if self.popup.is_some() {
            if self.on_popup(x, y, layout) {
                return Some(
                    self.route(SettingsInput::PointerPress { x, y }, layout)
                        .unwrap_or(ToolbarEvent::Absorbed),
                );
            }
            let open = self.popup.map(|p| usize::from(p.pin));
            self.popup = None;
            return Some(match target {
                // The pin of the open popup closes it; another one opens its own.
                Some(Target::Pin(pin)) if Some(pin) != open => ToolbarEvent::OpenPopup { pin },
                Some(Target::Settings) => ToolbarEvent::OpenSettings,
                Some(Target::Home) => ToolbarEvent::Home,
                Some(Target::Player) => ToolbarEvent::OpenPlayer,
                _ => ToolbarEvent::ClosePopup,
            });
        }
        match target {
            Some(Target::Settings) => Some(ToolbarEvent::OpenSettings),
            Some(Target::Home) => Some(ToolbarEvent::Home),
            Some(Target::Pin(pin)) => Some(ToolbarEvent::OpenPopup { pin }),
            Some(Target::Player) => Some(ToolbarEvent::OpenPlayer),
            // The bar takes the press: nothing under it is clicked through.
            None if layout.bar.contains(x, y) => Some(ToolbarEvent::Absorbed),
            None => None,
        }
    }

    /// The primary button released: ends a slider's drag.
    pub fn release(&mut self, layout: &ToolbarLayout) {
        self.player_release();
        self.route(SettingsInput::PointerRelease, layout);
    }

    /// The wheel over an open dropdown list of the popup: whether it took it.
    pub fn scroll(&mut self, lines: f32, x: f32, y: f32, layout: &ToolbarLayout) -> bool {
        if !self.on_popup(x, y, layout) {
            return false;
        }
        self.route(SettingsInput::Scroll { lines }, layout);
        true
    }

    /// Escape: closes an open dropdown list, else the popup. `None` without a popup.
    pub fn back(&mut self, layout: &ToolbarLayout) -> Option<ToolbarEvent> {
        if self.player.take().is_some() {
            return Some(ToolbarEvent::ClosePlayer);
        }
        let popup = self.popup.as_mut()?;
        if let RowWidget::Slider(slider) = &mut popup.widget
            && slider.is_editing()
        {
            // Escape drops the typed value first.
            slider.cancel_edit();
            return Some(ToolbarEvent::Absorbed);
        }
        if let RowWidget::Dropdown(d) = &popup.widget
            && d.is_open()
        {
            let row = layout.row(usize::from(popup.pin), 0.0);
            route_value(&mut popup.widget, SettingsInput::Back, &row, 0);
            return Some(ToolbarEvent::Absorbed);
        }
        self.popup = None;
        Some(ToolbarEvent::ClosePopup)
    }

    /// The frame to draw, with the local time as minutes into the day.
    pub fn frame(&self, visible: bool, clock: u16) -> ToolbarFrame {
        ToolbarFrame {
            visible,
            hover: self.hover,
            pins: self.pins,
            pin_count: self.pin_count as u8,
            popup: self.popup,
            clock,
            slide: 1.0,
            follows: false,
            player: self.player_frame(),
            now_playing: NowPlayingFrame::default(),
        }
    }
}

/// The toolbar's own texts, already localised.
#[derive(Clone, Debug, Default)]
pub struct ToolbarLabels {
    pub settings: String,
    pub home: String,
    /// The player's button tooltip.
    pub player: String,
    /// The player's popup when nothing plays.
    pub nothing_playing: String,
}

/// The toolbar on the draw thread.
pub struct ToolbarView {
    theme: Theme,
    type_setter: Typesetter,
    scale: f32,
    settings: Line,
    home: Line,
    player_tip: Line,
    player: PlayerView,
    clock: Line,
    clock_seen: u16,
    /// All the digits, so their glyphs are asked for up front.
    warm: Line,
}

impl std::fmt::Debug for ToolbarView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolbarView").finish_non_exhaustive()
    }
}

impl ToolbarView {
    pub fn new(registry: Arc<FontRegistry>, theme: Theme, labels: &ToolbarLabels) -> ToolbarView {
        let mut clock = Line::new("");
        clock.text.reserve(8);
        let mut view = ToolbarView {
            type_setter: Typesetter::new(registry, theme.font_weight),
            scale: 0.0,
            settings: Line::new(&labels.settings),
            home: Line::new(&labels.home),
            player_tip: Line::new(&labels.player),
            player: PlayerView::new(&labels.nothing_playing),
            clock,
            clock_seen: NO_CLOCK,
            warm: Line::new("0123456789:"),
            theme,
        };
        view.lay_out(1.0);
        view
    }

    /// Lays the texts out for a `width` × `height` window ahead of the first showing there.
    /// Allocates: call it off the hot paths.
    pub fn lay_out_for(&mut self, width: f32, height: f32) {
        let scale = ToolbarLayout::new(width, height, &self.theme, 0).scale;
        if scale != self.scale {
            self.lay_out(scale);
        }
    }

    /// How many glyphs of its texts are not ready yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        [
            &self.settings,
            &self.home,
            &self.player_tip,
            &self.clock,
            &self.warm,
        ]
        .into_iter()
        .flat_map(|l| l.layout.glyphs())
        .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), Glyph::Pending))
        .count()
    }

    fn lay_out(&mut self, scale: f32) {
        self.scale = scale;
        let text = self.theme.widget_text_size * scale;
        // Toolbar buttons are Bold (ADR-0174).
        let bold = self.theme.heading_face(text);
        self.type_setter.lay_out_face(&mut self.settings, bold);
        self.type_setter.lay_out_face(&mut self.home, bold);
        self.type_setter.lay_out_face(&mut self.player_tip, bold);
        self.player
            .lay_out(&mut self.type_setter, &self.theme, scale);
        self.lay_out_clock();
        self.type_setter
            .lay_out(&mut self.warm, self.theme.toolbar_clock_size * scale);
    }

    fn lay_out_clock(&mut self) {
        self.type_setter
            .lay_out(&mut self.clock, self.theme.toolbar_clock_size * self.scale);
    }

    /// Draws the toolbar of `frame` on a `width` × `height` window at `now_ns`; the pinned
    /// rows' names and the popup's row come from `rows`. Returns where the tooltip's shapes
    /// and texts begin when there is one: the caller draws them on a layer of their own,
    /// over the rest. Does not allocate once laid out at the window's scale.
    ///
    /// The bar slides in from above as `frame.slide` goes from 0 to 1 (ADR-0178); `words`
    /// and `cover` (the playing beatmap's background, when ready) are for the player's
    /// popup.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        rows: &mut SettingsView,
        frame: &ToolbarFrame,
        words: &NowPlayingText,
        cover: Option<AtlasRegion>,
        width: f32,
        height: f32,
        now_ns: u64,
        white: AtlasRegion,
    ) -> Option<(u32, u32)> {
        if !frame.visible || frame.slide <= 0.0 {
            return None;
        }
        let first = (shapes.len(), text.len());
        let tip = self.push_bar(
            shapes, text, glyphs, rows, frame, words, cover, width, height, now_ns, white,
        );
        let slide = frame.slide.clamp(0.0, 1.0);
        let (away, fade) = if frame.follows {
            (1.0 - slide, slide)
        } else {
            (1.0 - ease_out_quint(slide), 1.0)
        };
        if away > 0.0 || fade < 1.0 {
            let dy = -away * ToolbarLayout::new(width, height, &self.theme, 0).height();
            let map = Affine::about([0.0, 0.0], 1.0, 0.0, [0.0, dy]);
            let tint = [1.0, 1.0, 1.0, fade];
            shapes.transform_from(first.0, &map, tint);
            text.transform_from(first.1, &map, tint);
        }
        tip
    }

    #[allow(clippy::too_many_arguments)]
    fn push_bar(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        rows: &mut SettingsView,
        frame: &ToolbarFrame,
        words: &NowPlayingText,
        cover: Option<AtlasRegion>,
        width: f32,
        height: f32,
        now_ns: u64,
        white: AtlasRegion,
    ) -> Option<(u32, u32)> {
        let pins = usize::from(frame.pin_count).min(MAX_PINS);
        let layout = ToolbarLayout::new(width, height, &self.theme, pins);
        if layout.scale != self.scale {
            self.lay_out(layout.scale);
        }
        if frame.clock != self.clock_seen {
            self.clock_seen = frame.clock;
            self.clock.text.clear();
            if frame.clock != NO_CLOCK {
                let _ = write!(
                    self.clock.text,
                    "{:02}:{:02}",
                    frame.clock / 60 % 24,
                    frame.clock % 60
                );
            }
            self.lay_out_clock();
        }
        let theme = self.theme;
        let scale = layout.scale;
        draw(
            shapes,
            white,
            &layout.bar,
            theme.toolbar_fill,
            Look::Plain,
            &theme,
            scale,
        );
        let popup_pin = frame.popup.map(|p| usize::from(p.pin));
        let button = |shapes: &mut ShapeBatch, rect: &Rect, lit: bool, parts: &[icons::Part]| {
            if lit {
                let inset = theme.toolbar_gap * scale;
                let plate = Rect {
                    x: rect.x + inset,
                    y: rect.y + inset,
                    width: rect.width - 2.0 * inset,
                    height: rect.height - 2.0 * inset,
                };
                draw(
                    shapes,
                    white,
                    &plate,
                    theme.button_hover_fill,
                    Look::Round,
                    &theme,
                    scale,
                );
            }
            icons::push_icon(
                shapes,
                white,
                rect,
                theme.toolbar_icon_share,
                theme.text,
                parts,
            );
        };
        button(
            shapes,
            &layout.settings,
            frame.hover == Some(Target::Settings),
            icons::GEAR,
        );
        button(
            shapes,
            &layout.home,
            frame.hover == Some(Target::Home),
            icons::HOME,
        );
        button(
            shapes,
            &layout.player,
            frame.hover == Some(Target::Player) || frame.player.is_some(),
            icons::NOTE,
        );
        for (i, rect) in layout.pins().iter().enumerate() {
            let row = usize::from(frame.pins[i]);
            let lit = frame.hover == Some(Target::Pin(i)) || popup_pin == Some(i);
            let parts = icons::icon(rows.section_of(row).unwrap_or(0));
            button(shapes, rect, lit, parts);
        }
        if frame.clock != NO_CLOCK {
            push_right(
                text,
                &self.clock,
                &layout.clock,
                theme.widget_pad * scale,
                theme.text,
                glyphs,
            );
        }
        if let (Some(popup), true) = (frame.popup, popup_pin.is_some()) {
            self.push_popup(shapes, text, glyphs, rows, &layout, &popup, now_ns, white);
        }
        if let Some(player) = &frame.player {
            self.player.push(
                &mut self.type_setter,
                shapes,
                text,
                glyphs,
                &layout,
                player,
                &frame.now_playing,
                words,
                cover,
                now_ns,
                white,
            );
        }
        // The tooltip of the hovered button, over everything; a pin whose popup is open
        // (or the player's) shows its row instead.
        let tip = frame
            .hover
            .filter(|t| !matches!(t, Target::Pin(i) if popup_pin == Some(*i)))
            .filter(|t| !(*t == Target::Player && frame.player.is_some()))?;
        let (rect, label) = match tip {
            Target::Settings => (layout.settings, &self.settings),
            Target::Home => (layout.home, &self.home),
            Target::Player => (layout.player, &self.player_tip),
            Target::Pin(i) => {
                let rect = *layout.pins().get(i)?;
                let width = rows.shown_label(usize::from(frame.pins[i]))?.1;
                return Some(self.push_tip(
                    shapes,
                    text,
                    glyphs,
                    &layout,
                    &rect,
                    width,
                    |text, glyphs, plate, pad| {
                        rows.push_label(
                            text,
                            glyphs,
                            usize::from(frame.pins[i]),
                            plate,
                            pad,
                            theme.text,
                        );
                    },
                    white,
                ));
            }
        };
        let (width, line) = (label.layout.width(), label);
        Some(self.push_tip(
            shapes,
            text,
            glyphs,
            &layout,
            &rect,
            width,
            |text, glyphs, plate, pad| push_left(text, line, plate, pad, theme.text, glyphs),
            white,
        ))
    }

    /// The tooltip plate under `icon`, `label_width` wide; `label` draws the text on it.
    /// Returns the split before it.
    #[allow(clippy::too_many_arguments)]
    fn push_tip(
        &self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        layout: &ToolbarLayout,
        icon: &Rect,
        label_width: f32,
        label: impl FnOnce(&mut SpriteBatch, &mut GlyphCache, &Rect, f32),
        white: AtlasRegion,
    ) -> (u32, u32) {
        let split = (shapes.len() as u32, text.len() as u32);
        let theme = &self.theme;
        let pad = theme.widget_pad * layout.scale;
        let width = label_width + 2.0 * pad;
        let plate = Rect {
            x: (icon.x + icon.width / 2.0 - width / 2.0)
                .clamp(0.0, (layout.bar.width - width).max(0.0)),
            y: layout.bar.height + theme.toolbar_gap * layout.scale * 2.0,
            width,
            height: theme.widget_height * layout.scale * 0.8,
        };
        draw(
            shapes,
            white,
            &plate,
            theme.widget_popup_fill,
            Look::Round,
            theme,
            layout.scale,
        );
        label(text, glyphs, &plate, pad);
        split
    }

    #[allow(clippy::too_many_arguments)]
    fn push_popup(
        &mut self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        rows: &mut SettingsView,
        layout: &ToolbarLayout,
        popup: &PopupFrame,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let theme = self.theme;
        let ms = (f64::from(theme.toolbar_popup_ms.max(0.0)) * 1e6) as u64;
        // Fades in and slides down onto its place; without effects it is simply there.
        let t = if theme.effects {
            ease_out_cubic(progress(popup.since_ns, now_ns, ms))
        } else {
            1.0
        };
        let dy = (t - 1.0) * theme.toolbar_popup_slide * layout.scale;
        let pin = usize::from(popup.pin);
        let rect = {
            let r = layout.popup(pin);
            Rect { y: r.y + dy, ..r }
        };
        draw(
            shapes,
            white,
            &rect,
            faded(theme.widget_popup_fill, t),
            Look::Round,
            &theme,
            layout.scale,
        );
        let row = layout.row(pin, dy);
        rows.push_row(
            shapes,
            text,
            glyphs,
            &row,
            panel_width(layout.window[0], layout.window[1], &theme),
            layout.window[1],
            usize::from(popup.row),
            &popup.widget,
            t,
            white,
            now_ns,
        );
    }
}

#[cfg(test)]
mod tests;
