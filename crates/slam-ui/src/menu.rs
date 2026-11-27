//! The main menu (ADR-0186): the beating logo alone, then, on a click, the buttons in the
//! lazer style (a bar across the screen) or the stable one (a column out of the logo).
//! Update-side state and layout, input-agnostic (the application maps its events to
//! [`MenuInput`]). The draw side is [`view::MenuView`].

use crate::anim::{ease_out_elastic, ease_out_expo, ease_out_quint};
use crate::theme::Theme;

pub mod logo;
pub mod view;

pub use logo::{LogoAudio, LogoMotion, LogoSnapshot};
pub use view::{LogoArt, MenuLabels, MenuView};

/// What the menu understands; the application turns keys and mouse events into these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuInput {
    Up,
    Down,
    Activate,
    Back,
    /// Pointer moved to `x`, `y` in window pixels.
    PointerMove {
        x: f32,
        y: f32,
    },
    /// Primary button pressed at `x`, `y`.
    PointerPress {
        x: f32,
        y: f32,
    },
    /// A button's key, as lazer's (P play, O or S settings, Q exit): play opens the
    /// buttons first, the others act only while the buttons are out.
    Hotkey(MenuItem),
}

/// What the menu asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Play,
    Settings,
    Exit,
}

/// A sound the menu asks for: the buttons opened or closed, or the logo was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCue {
    Open,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Play,
    Settings,
    Exit,
}

impl MenuItem {
    /// The items in keyboard order (lazer's bar shows settings first, left of the logo).
    pub const ALL: [MenuItem; 3] = [MenuItem::Play, MenuItem::Settings, MenuItem::Exit];

    /// Whether the item can be selected and activated.
    pub const fn enabled(self) -> bool {
        true
    }

    fn action(self) -> MenuAction {
        match self {
            MenuItem::Play => MenuAction::Play,
            MenuItem::Settings => MenuAction::Settings,
            MenuItem::Exit => MenuAction::Exit,
        }
    }
}

/// How the buttons come out of the logo (`general.menu_style`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MenuStyle {
    /// A bar across the screen, settings left of the logo (lazer's `ButtonSystem`).
    #[default]
    Lazer,
    /// A column sliding out of the logo to the right (stable).
    Stable,
}

/// An axis-aligned rectangle: top-left corner and size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Whether the rectangle sheared by `skew` radians about its centre line (top to the
    /// right, as interface shapes are drawn, ADR-0116) contains the point.
    pub fn contains_skewed(&self, x: f32, y: f32, skew: f32) -> bool {
        let shift = (self.y + self.height / 2.0 - y) * skew.tan();
        self.contains(x - shift, y)
    }

    pub fn centre(&self) -> [f32; 2] {
        [self.x + self.width / 2.0, self.y + self.height / 2.0]
    }
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Menu/ButtonSystem.cs, ButtonArea.cs,
// MainMenuButton.cs (sizes in reference pixels of a 768-high window).
const BUTTON_WIDTH: f32 = 140.0;
const BAR_HEIGHT: f32 = 100.0;
const HOVER_WIDTH: f32 = 1.5;
const HOVER_MS: f64 = 500.0;
const EXPAND_MS: f64 = 500.0;
const EXPAND_DELAY_MS: f64 = 150.0;
const BAR_FADE_MS: f64 = 300.0;
/// The logo's size alone, and in the bar as a part of it (lazer: half, then the flow's).
const LOGO_SIZE: f32 = 400.0;
/// Stable's logo alone is larger: two thirds of the window's height (the user's stable
/// recording, ADR-0186).
const STABLE_LOGO_SIZE: f32 = 515.0;
const LOGO_TOP_SCALE: f32 = 0.45;
const LOGO_OPEN_MS: f64 = 300.0;
const LOGO_CLOSE_MS: f64 = 800.0;

// Ported from McOsu db2add20: src/App/Osu/OsuMainMenu.cpp (setMenuElementsVisible): the
// logo slides aside in 0.35 s; the column closes after 6 s untouched. How far it slides
// and the column's sizes follow the user's stable recording (ADR-0186).
const STABLE_OPEN_MS: f64 = 350.0;
const STABLE_CLOSE_MS: f64 = 500.0;
const STABLE_IDLE_MS: f64 = 6000.0;
/// The column's buttons: height and gap as parts of the logo, how far they reach past it,
/// and how much further the hovered one slides.
const STABLE_SHIFT: f32 = 0.38;
const STABLE_BUTTON: f32 = 0.165;
const STABLE_GAP: f32 = 0.035;
const STABLE_REACH: f32 = 0.68;
const STABLE_HOVER: f32 = 0.06;
const STABLE_HOVER_MS: f64 = 200.0;

/// The menu's fixed geometry for a window: a pure function of its size and the theme,
/// shared by input and drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuLayout {
    pub width: f32,
    pub height: f32,
    /// Pixels per reference unit.
    pub scale: f32,
    /// Skew of the buttons in radians (ADR-0117): hit tests follow it.
    pub skew: f32,
    /// The toolbar's height: lazer's bar centres in the room under it.
    pub toolbar: f32,
}

impl MenuLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> MenuLayout {
        MenuLayout {
            width,
            height,
            scale: theme.scale(height),
            skew: theme.button_skew(),
            toolbar: theme.toolbar_height * theme.scale(height),
        }
    }

    /// The logo alone in lazer's style (as the intro leaves it): its centre and size.
    pub fn logo_alone(&self) -> ([f32; 2], f32) {
        self.logo_alone_in(MenuStyle::Lazer)
    }

    /// The logo alone in `style`.
    pub fn logo_alone_in(&self, style: MenuStyle) -> ([f32; 2], f32) {
        let size = match style {
            MenuStyle::Lazer => LOGO_SIZE,
            MenuStyle::Stable => STABLE_LOGO_SIZE,
        };
        ([self.width / 2.0, self.height / 2.0], size * self.scale)
    }
}

/// One button as drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MenuButton {
    pub rect: Rect,
    /// 0 hidden, 1 fully out.
    pub alpha: f32,
    pub hovered: bool,
    /// How lit by the pointer, 0 to 1 (lazer's hover layer).
    pub lit: f32,
}

/// What the draw side needs of the menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuSnapshot {
    pub style: MenuStyle,
    /// Index into [`MenuItem::ALL`] of the selected or hovered item.
    pub selected: usize,
    pub logo: LogoSnapshot,
    pub logo_centre: [f32; 2],
    /// Diameter before the logo's own scale.
    pub logo_size: f32,
    /// In [`MenuItem::ALL`] order.
    pub buttons: [MenuButton; 3],
    /// The lazer bar's opacity.
    pub bar_alpha: f32,
}

impl Default for MenuSnapshot {
    fn default() -> Self {
        MenuSnapshot {
            style: MenuStyle::Lazer,
            selected: 0,
            logo: LogoSnapshot::default(),
            logo_centre: [0.0; 2],
            logo_size: 0.0,
            buttons: [MenuButton::default(); 3],
            bar_alpha: 0.0,
        }
    }
}

/// A value going between two ends over time.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Swing {
    from: f32,
    to: f32,
    at_ns: u64,
    ms: f64,
}

impl Swing {
    const fn rest(value: f32) -> Swing {
        Swing {
            from: value,
            to: value,
            at_ns: 0,
            ms: 0.0,
        }
    }

    /// Linear progress 0..1.
    fn t(&self, now_ns: u64) -> f32 {
        if self.ms <= 0.0 {
            return 1.0;
        }
        (now_ns.saturating_sub(self.at_ns) as f64 / 1e6 / self.ms).clamp(0.0, 1.0) as f32
    }

    fn value(&self, now_ns: u64, ease: fn(f32) -> f32) -> f32 {
        self.from + (self.to - self.from) * ease(self.t(now_ns))
    }

    fn go(&mut self, to: f32, ms: f64, now_ns: u64, ease: fn(f32) -> f32) {
        *self = Swing {
            from: self.value(now_ns, ease),
            to,
            at_ns: now_ns,
            ms,
        };
    }
}

fn ease_in_out_quad(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
    }
}

fn linear(t: f32) -> f32 {
    t
}

/// The menu's state on the update thread.
#[derive(Clone, Debug, PartialEq)]
pub struct MainMenu {
    style: MenuStyle,
    open: bool,
    selected: usize,
    /// The item under the pointer, if any.
    hovered: Option<usize>,
    logo: LogoMotion,
    /// The logo between alone (0) and with the buttons (1).
    logo_move: Swing,
    /// The buttons' width share (lazer) or reach (stable), 0 to 1.
    expand: Swing,
    bar: Swing,
    /// Per item: lazer's hover widening or stable's hover slide, 0 to 1.
    hover: [Swing; 3],
    /// Last time the player touched the menu (stable closes after a while).
    touched_ns: u64,
    cue: Option<MenuCue>,
}

impl Default for MainMenu {
    fn default() -> Self {
        MainMenu {
            style: MenuStyle::Lazer,
            open: false,
            selected: 0,
            hovered: None,
            logo: LogoMotion::new(),
            logo_move: Swing::rest(0.0),
            expand: Swing::rest(0.0),
            bar: Swing::rest(0.0),
            hover: [Swing::rest(0.0); 3],
            touched_ns: 0,
            cue: None,
        }
    }
}

impl MainMenu {
    pub fn new() -> MainMenu {
        MainMenu::default()
    }

    pub fn selected(&self) -> MenuItem {
        MenuItem::ALL[self.selected]
    }

    /// Whether the buttons are out.
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn style(&self) -> MenuStyle {
        self.style
    }

    /// Follows `general.menu_style`.
    pub fn set_style(&mut self, style: MenuStyle) {
        self.style = style;
    }

    /// The sound asked for since the last call.
    pub fn take_cue(&mut self) -> Option<MenuCue> {
        self.cue.take()
    }

    /// Advances the logo to `now_ns` on `audio`, and closes the stable column after it
    /// has been left alone a while.
    pub fn tick(&mut self, now_ns: u64, audio: &LogoAudio<'_>, effects: bool) {
        self.logo.tick(now_ns, audio, effects);
        if self.open
            && self.style == MenuStyle::Stable
            && self.hovered.is_none()
            && !self.logo.hovered()
            && now_ns.saturating_sub(self.touched_ns) as f64 / 1e6 > STABLE_IDLE_MS
        {
            self.close(now_ns);
        }
    }

    fn open(&mut self, now_ns: u64) {
        if self.open {
            return;
        }
        self.open = true;
        self.cue = Some(MenuCue::Open);
        self.touched_ns = now_ns;
        match self.style {
            MenuStyle::Lazer => {
                self.logo_move.go(1.0, LOGO_OPEN_MS, now_ns, ease_out_quint);
                self.expand = Swing {
                    from: 0.0,
                    to: 1.0,
                    at_ns: now_ns + (EXPAND_DELAY_MS * 1e6) as u64,
                    ms: EXPAND_MS,
                };
                self.bar.go(1.0, BAR_FADE_MS, now_ns, linear);
            }
            MenuStyle::Stable => {
                self.logo_move
                    .go(1.0, STABLE_OPEN_MS, now_ns, ease_in_out_quad);
                self.expand
                    .go(1.0, STABLE_OPEN_MS, now_ns, ease_in_out_quad);
            }
        }
    }

    fn close(&mut self, now_ns: u64) {
        if !self.open {
            return;
        }
        self.open = false;
        self.cue = Some(MenuCue::Close);
        self.hovered = None;
        match self.style {
            MenuStyle::Lazer => {
                self.logo_move.go(0.0, LOGO_CLOSE_MS, now_ns, ease_out_expo);
                self.expand.go(0.0, EXPAND_MS, now_ns, ease_out_expo);
                self.bar.go(0.0, BAR_FADE_MS, now_ns, linear);
            }
            MenuStyle::Stable => {
                let ms =
                    STABLE_CLOSE_MS * f64::from(self.logo_move.value(now_ns, ease_in_out_quad));
                self.logo_move.go(0.0, ms, now_ns, ease_in_out_quad);
                self.expand.go(0.0, ms, now_ns, ease_in_out_quad);
            }
        }
    }

    /// The logo's centre and size at `now_ns`.
    fn logo_at(&self, layout: &MenuLayout, now_ns: u64) -> ([f32; 2], f32) {
        let ([cx, cy], size) = layout.logo_alone_in(self.style);
        match self.style {
            MenuStyle::Lazer => {
                let ease = if self.open {
                    ease_out_quint
                } else {
                    ease_out_expo
                };
                let t = self.logo_move.value(now_ns, ease);
                // In the bar the logo sits left of the centre by half the extra width of
                // the buttons on its right (two) over the one on its left.
                let top_x = cx - BUTTON_WIDTH / 2.0 * layout.scale;
                let top_y = cy + layout.toolbar / 2.0;
                let top_size = size * LOGO_TOP_SCALE;
                (
                    [cx + (top_x - cx) * t, cy + (top_y - cy) * t],
                    size + (top_size - size) * t,
                )
            }
            MenuStyle::Stable => {
                let t = self.logo_move.value(now_ns, ease_in_out_quad);
                ([cx - size * STABLE_SHIFT * t, cy], size)
            }
        }
    }

    /// The buttons and the bar at `now_ns`.
    fn buttons_at(&self, layout: &MenuLayout, now_ns: u64) -> ([MenuButton; 3], f32) {
        let ([cx, cy], size) = self.logo_at(layout, now_ns);
        let mut buttons = [MenuButton::default(); 3];
        match self.style {
            MenuStyle::Lazer => {
                let s = layout.scale;
                let expand = self.expand.value(now_ns, ease_out_expo).clamp(0.0, 1.0);
                let height = BAR_HEIGHT * s;
                let y = cy - height / 2.0;
                let width = |i: usize| {
                    let hover = self.hover[i].value(now_ns, ease_out_elastic);
                    BUTTON_WIDTH * s * (1.0 + (HOVER_WIDTH - 1.0) * hover) * expand
                };
                // Settings ends at the logo's left, the others follow from its right.
                let inset = size * 0.45;
                let settings = 1;
                let w = width(settings);
                buttons[settings].rect = Rect {
                    x: cx - inset - w,
                    y,
                    width: w,
                    height,
                };
                let mut x = cx + inset;
                for i in [0, 2] {
                    let w = width(i);
                    buttons[i].rect = Rect {
                        x,
                        y,
                        width: w,
                        height,
                    };
                    x += w;
                }
                for button in &mut buttons {
                    // lazer fades the content in from half the width on.
                    button.alpha = ((expand - 0.5) / 0.3).clamp(0.0, 1.0);
                }
                (buttons, self.bar.value(now_ns, linear))
            }
            MenuStyle::Stable => {
                let reach = self.expand.value(now_ns, ease_in_out_quad).clamp(0.0, 1.0);
                let height = size * STABLE_BUTTON;
                let gap = size * STABLE_GAP;
                let total = 3.0 * height + 2.0 * gap;
                for (i, button) in buttons.iter_mut().enumerate() {
                    let slide = self.hover[i].value(now_ns, ease_out_quint);
                    let right =
                        cx + size / 2.0 + size * (STABLE_REACH * reach + STABLE_HOVER * slide);
                    button.rect = Rect {
                        x: cx,
                        y: cy - total / 2.0 + i as f32 * (height + gap),
                        width: (right - cx).max(0.0),
                        height,
                    };
                    button.alpha = reach;
                }
                (buttons, 0.0)
            }
        }
    }

    /// The item whose button is under `x`, `y` at `now_ns`, while the buttons are out.
    fn item_at(&self, layout: &MenuLayout, now_ns: u64, x: f32, y: f32) -> Option<usize> {
        if !self.open {
            return None;
        }
        let (centre, size) = self.logo_at(layout, now_ns);
        if logo_contains(centre, size, x, y) {
            return None;
        }
        let (buttons, _) = self.buttons_at(layout, now_ns);
        buttons
            .iter()
            .position(|b| b.alpha > 0.5 && b.rect.contains_skewed(x, y, layout.skew))
            .filter(|&i| MenuItem::ALL[i].enabled())
    }

    fn set_hovered(&mut self, hovered: Option<usize>, now_ns: u64) {
        if hovered == self.hovered {
            return;
        }
        let (ms, ease): (f64, fn(f32) -> f32) = match self.style {
            MenuStyle::Lazer => (HOVER_MS, ease_out_elastic),
            MenuStyle::Stable => (STABLE_HOVER_MS, ease_out_quint),
        };
        if let Some(old) = self.hovered {
            self.hover[old].go(0.0, ms, now_ns, ease);
        }
        if let Some(new) = hovered {
            self.hover[new].go(1.0, ms, now_ns, ease);
            self.selected = new;
        }
        self.hovered = hovered;
    }

    pub fn snapshot(&self, layout: &MenuLayout, now_ns: u64) -> MenuSnapshot {
        let (logo_centre, logo_size) = self.logo_at(layout, now_ns);
        let (mut buttons, bar_alpha) = self.buttons_at(layout, now_ns);
        for (i, button) in buttons.iter_mut().enumerate() {
            button.hovered = self.open && i == self.selected;
            button.lit = self.hover[i].value(now_ns, ease_out_quint).clamp(0.0, 1.0);
        }
        MenuSnapshot {
            style: self.style,
            selected: self.selected,
            logo: self.logo.snapshot(now_ns),
            logo_centre,
            logo_size,
            buttons,
            bar_alpha,
        }
    }

    /// Moves the selection one item, wrapping, opening the buttons first if needed. The
    /// keys walk play, settings, exit in either style, as the menu always has.
    fn step(&mut self, forward: bool, now_ns: u64) {
        self.open(now_ns);
        let next = if forward {
            (self.selected + 1) % 3
        } else {
            (self.selected + 2) % 3
        };
        if self.hovered.is_some() {
            self.set_hovered(Some(next), now_ns);
        }
        self.selected = next;
    }

    /// Applies `input` at `now_ns`; `layout` places the logo and buttons for pointer
    /// input. Does not allocate.
    pub fn input(
        &mut self,
        input: MenuInput,
        layout: &MenuLayout,
        now_ns: u64,
    ) -> Option<MenuAction> {
        self.touched_ns = now_ns;
        match input {
            MenuInput::Up => self.step(false, now_ns),
            MenuInput::Down => self.step(true, now_ns),
            MenuInput::Activate => {
                if self.open {
                    return Some(self.selected().action());
                }
                // Enter on the logo alone plays at once: the keyboard's quick way in.
                self.logo.click(now_ns);
                return Some(MenuAction::Play);
            }
            MenuInput::Back => {
                if !self.open {
                    return Some(MenuAction::Exit);
                }
                self.close(now_ns);
            }
            MenuInput::Hotkey(item) => {
                if self.open {
                    self.selected = MenuItem::ALL.iter().position(|i| *i == item).unwrap_or(0);
                    return Some(item.action());
                }
                if item == MenuItem::Play {
                    self.logo.click(now_ns);
                    self.open(now_ns);
                }
            }
            MenuInput::PointerMove { x, y } => {
                let (centre, size) = self.logo_at(layout, now_ns);
                self.logo
                    .set_hovered(logo_contains(centre, size, x, y), now_ns);
                let item = self.item_at(layout, now_ns, x, y);
                self.set_hovered(item, now_ns);
            }
            MenuInput::PointerPress { x, y } => {
                let (centre, size) = self.logo_at(layout, now_ns);
                if logo_contains(centre, size, x, y) {
                    self.logo.click(now_ns);
                    if self.open {
                        return Some(MenuAction::Play);
                    }
                    self.open(now_ns);
                    return None;
                }
                if let Some(index) = self.item_at(layout, now_ns, x, y) {
                    self.selected = index;
                    return Some(MenuItem::ALL[index].action());
                }
            }
        }
        None
    }
}

/// Whether the logo of `size` at `centre` contains the point (a circle).
fn logo_contains(centre: [f32; 2], size: f32, x: f32, y: f32) -> bool {
    let (dx, dy) = (x - centre[0], y - centre[1]);
    dx * dx + dy * dy <= size * size / 4.0
}

#[cfg(test)]
mod tests;
