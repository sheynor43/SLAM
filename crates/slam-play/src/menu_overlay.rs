//! The pause and fail menus: lazer's `GameplayMenuOverlay` with its `PauseOverlay` and
//! `FailOverlay` (ADR-0052).
//!
//! The update side ([`GameplayMenu`]) belongs to the play session: which menu shows, which
//! button is selected, and what the keys and the mouse do to them. It goes to the draw side
//! as a fixed-size [`MenuSnapshot`]; [`MenuView`] draws it over the play.
//!
//! The buttons are plain rectangles: no shear, glow, flash on click or text scale of lazer's
//! `DialogButton`, only its width on selection. No pause loop sound and no button to save
//! the failed score yet.

use std::sync::Arc;

use slam_input::{InputEvent, InputKind, MouseButton};
use slam_osu::Vec2;
use slam_render::Color;
use slam_render::atlas::AtlasRegion;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::{
    FamilyId, FontRegistry, GlyphCache, GlyphKey, HEADING_FAMILY, Layout, LayoutContext, TextStats,
    TextStyle, UI_FAMILY, push_layout,
};

use crate::hotkeys::MenuKey;
use crate::hud::{NumberText, Template, format_accuracy, number_text};

const NS_PER_MS: f64 = 1_000_000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (TRANSITION_DURATION, button_height, background_alpha)
/// How long the menu fades in and out, in nanoseconds.
pub const TRANSITION_NS: u64 = 200_000_000;
const BUTTON_HEIGHT: f32 = 80.0;
pub const BACKGROUND_ALPHA: f32 = 0.75;
/// Space between the buttons.
const BUTTON_SPACING: f32 = 2.0;
/// Space between the buttons and the window's sides.
const BUTTON_PADDING: f32 = 50.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailOverlay.cs (load)
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/TwoLayerButton.cs (SIZE_EXTENDED)
/// Height of the fail menu's footer, which holds the button to save the score there (not
/// drawn yet, but it takes its place).
const FAIL_FOOTER_HEIGHT: f32 = 50.0;
const TITLE_SIZE: f32 = 48.0;
const TITLE_WEIGHT: u16 = 600;
const INFO_SIZE: f32 = 18.0;
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuFont.cs (GetFont)
/// Lazer's default weight, medium.
const INFO_WEIGHT: u16 = 500;
const BOLD: u16 = 700;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/DialogButton.cs (idle_width, hover_width, hover_duration, spriteText)
/// The coloured part of a button, as a part of the button's width.
const IDLE_WIDTH: f32 = 0.8;
const HOVER_WIDTH: f32 = 0.9;
/// How long a button widens when selected, in nanoseconds; it narrows in half the time.
const HOVER_NS: u64 = 400_000_000;
const BUTTON_TEXT_SIZE: f32 = 28.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/ScalingContainer.cs (ScalingContainerTargetDrawSize)
/// The size the interface is laid out at; it scales to fit the window.
const TARGET_SIZE: [f32; 2] = [1024.0, 768.0];

/// Most buttons a menu has.
pub const MAX_BUTTONS: usize = 3;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/OsuColour.cs (Yellow, YellowDark, Green)
const YELLOW: Color = hex(0xff, 0xcc, 0x22);
const YELLOW_DARK: Color = hex(0xee, 0xaa, 0x00);
const GREEN: Color = hex(0x88, 0xb3, 0x00);
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (AddButton for OnQuit)
const QUIT_RED: Color = hex(170, 27, 39);

const fn hex(r: u8, g: u8, b: u8) -> Color {
    Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// Which menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// Paused: continue, retry or quit.
    Pause,
    /// Failed: retry or quit.
    Fail,
}

/// A button of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuButton {
    Continue,
    Retry,
    Quit,
}

impl MenuKind {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (load)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (FailOverlay, PauseOverlay)
    /// The buttons from top to bottom. The fail menu has no continue.
    pub fn buttons(self) -> &'static [MenuButton] {
        match self {
            MenuKind::Pause => &[MenuButton::Continue, MenuButton::Retry, MenuButton::Quit],
            MenuKind::Fail => &[MenuButton::Retry, MenuButton::Quit],
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PauseOverlay.cs (BackAction)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (BackAction)
    /// The button Back presses: the first when paused (continue), the last otherwise (quit).
    pub fn back_button(self) -> MenuButton {
        let buttons = self.buttons();
        match self {
            MenuKind::Pause => buttons[0],
            MenuKind::Fail => buttons[buttons.len() - 1],
        }
    }
}

impl MenuButton {
    pub fn colour(self) -> Color {
        match self {
            MenuButton::Continue => GREEN,
            MenuButton::Retry => YELLOW_DARK,
            MenuButton::Quit => QUIT_RED,
        }
    }
}

/// An easing of osu-framework's `DefaultEasingFunction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Easing {
    In,
    OutQuint,
}

impl Easing {
    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/DefaultEasingFunction.cs (In, OutQuint)
    fn apply(self, time: f64) -> f64 {
        match self {
            Easing::In => time * time,
            Easing::OutQuint => {
                let time = time - 1.0;
                time * time * time * time * time + 1.0
            }
        }
    }
}

/// A value moving from one number to another, as a framework transform started from the
/// value at the time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    from: f32,
    to: f32,
    start_ns: u64,
    duration_ns: u64,
    easing: Easing,
}

impl Tween {
    /// A value standing at `value`.
    const fn at(value: f32) -> Tween {
        Tween {
            from: value,
            to: value,
            start_ns: 0,
            duration_ns: 0,
            easing: Easing::In,
        }
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/Transform.cs (Apply)
    /// The value at engine time `now_ns`.
    pub fn value_at(&self, now_ns: u64) -> f32 {
        let elapsed = now_ns.saturating_sub(self.start_ns);
        if elapsed >= self.duration_ns {
            return self.to;
        }
        let t = self
            .easing
            .apply(elapsed as f64 / NS_PER_MS / (self.duration_ns as f64 / NS_PER_MS));
        self.from + (self.to - self.from) * t as f32
    }

    /// Moves to `to` from the value at `now_ns`.
    fn retarget(&mut self, to: f32, now_ns: u64, duration_ns: u64, easing: Easing) {
        *self = Tween {
            from: self.value_at(now_ns),
            to,
            start_ns: now_ns,
            duration_ns,
            easing,
        };
    }
}

/// What the menu shows below its buttons, as of when it showed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuInfo {
    /// Times the play was restarted.
    pub retries: u32,
    /// How far into the playable part of the beatmap, 0..=100.
    pub progress: u8,
    /// Accuracy, 0..=1.
    pub accuracy: f64,
}

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (getSongProgress)
/// The song progress the menu shows at gameplay time `time`: through the playable part of
/// the beatmap (`first..last`), 0..=100, truncated.
pub fn song_progress(time: f64, first: f64, last: f64) -> u8 {
    let length = last - first;
    if length == 0.0 {
        return 0;
    }
    ((time - first) / length * 100.0).clamp(0.0, 100.0) as u8
}

/// Where the menu's parts are on a window: lazer's grid of the title, the buttons, the
/// information and the footer, in the interface scaled to fit 1024×768.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuLayout {
    /// Window units per interface unit.
    pub scale: f32,
    /// Vertical centre of the title.
    pub title_y: f32,
    /// Vertical centre of the information.
    pub info_y: f32,
    /// Left edge and width of the buttons.
    button_x: f32,
    button_width: f32,
    /// Top of the first button.
    buttons_top: f32,
}

impl MenuLayout {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (load)
    /// The layout of menu `kind` on a window of `width` × `height` units (as
    /// `DrawSizePreservingFillContainer` with the `Minimum` strategy scales). The rows around
    /// the buttons share the height left by them and the footer; the title and the
    /// information are centred in theirs.
    pub fn new(width: f32, height: f32, kind: MenuKind) -> MenuLayout {
        let scale = Self::scale(width, height);
        let count = kind.buttons().len() as f32;
        let block = (count * BUTTON_HEIGHT + (count - 1.0).max(0.0) * BUTTON_SPACING) * scale;
        let footer = match kind {
            MenuKind::Pause => 0.0,
            MenuKind::Fail => FAIL_FOOTER_HEIGHT * scale,
        };
        let row = ((height - block - footer) / 2.0).max(0.0);
        let padding = BUTTON_PADDING * scale;
        MenuLayout {
            scale,
            title_y: row / 2.0,
            info_y: row + block + row / 2.0,
            button_x: padding,
            button_width: (width - 2.0 * padding).max(0.0),
            buttons_top: row,
        }
    }

    /// Window units per interface unit on a window of `width` × `height` units.
    pub fn scale(width: f32, height: f32) -> f32 {
        (width / TARGET_SIZE[0]).min(height / TARGET_SIZE[1])
    }

    /// Top left and size of button `index`: its whole hit area.
    pub fn button(&self, index: usize) -> ([f32; 2], [f32; 2]) {
        let step = (BUTTON_HEIGHT + BUTTON_SPACING) * self.scale;
        (
            [self.button_x, self.buttons_top + index as f32 * step],
            [self.button_width, BUTTON_HEIGHT * self.scale],
        )
    }

    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Primitives/RectangleF.cs (Contains)
    /// The button of `buttons` under `at`, if any.
    pub fn button_at(&self, at: Vec2, buttons: usize) -> Option<usize> {
        (0..buttons).find(|&index| {
            let ([left, top], [width, height]) = self.button(index);
            left <= at.x && at.x < left + width && top <= at.y && at.y < top + height
        })
    }
}

/// What the draw side needs of the menu. Fixed size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuSnapshot {
    /// The menu showing, or the last one while it fades out.
    pub kind: MenuKind,
    pub visible: bool,
    /// The whole menu's alpha.
    pub alpha: Tween,
    /// The coloured part of each button as a part of its width.
    pub widths: [Tween; MAX_BUTTONS],
    pub info: MenuInfo,
}

impl Default for MenuSnapshot {
    fn default() -> Self {
        MenuSnapshot {
            kind: MenuKind::Pause,
            visible: false,
            alpha: Tween::at(0.0),
            widths: [Tween::at(IDLE_WIDTH); MAX_BUTTONS],
            info: MenuInfo {
                retries: 0,
                progress: 0,
                accuracy: 1.0,
            },
        }
    }
}

/// The update side of the menu. Never allocates.
#[derive(Debug, Clone, Default)]
pub struct GameplayMenu {
    state: MenuSnapshot,
    /// The selected button (lazer's `SelectionCycleFillFlowContainer`).
    selected: Option<usize>,
    /// The button under the mouse.
    hovered: Option<usize>,
    /// The button the left mouse button went down on.
    pressed: Option<usize>,
    /// The mouse in window units, once it moved.
    pointer: Option<Vec2>,
}

impl GameplayMenu {
    /// What the draw side needs.
    pub fn snapshot(&self) -> MenuSnapshot {
        self.state
    }

    pub fn is_visible(&self) -> bool {
        self.state.visible
    }

    /// The menu showing, if one does.
    pub fn kind(&self) -> Option<MenuKind> {
        self.state.visible.then_some(self.state.kind)
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (PopIn, load)
    /// Shows `kind` at `now_ns` with `info` on a window of `size` units. Nothing is selected
    /// (a visibility change deselects); the button under the mouse is hovered but selects
    /// only once the mouse moves.
    pub fn show(&mut self, kind: MenuKind, info: MenuInfo, now_ns: u64, size: [f32; 2]) {
        self.state.kind = kind;
        self.state.visible = true;
        self.state.info = info;
        self.state
            .alpha
            .retarget(1.0, now_ns, TRANSITION_NS, Easing::In);
        self.deselect(now_ns);
        self.pressed = None;
        let layout = MenuLayout::new(size[0], size[1], kind);
        self.hovered = self
            .pointer
            .and_then(|at| layout.button_at(at, kind.buttons().len()));
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (PopOut)
    /// Fades the menu out from `now_ns`.
    pub fn hide(&mut self, now_ns: u64) {
        if !self.state.visible {
            return;
        }
        self.state.visible = false;
        self.state
            .alpha
            .retarget(0.0, now_ns, TRANSITION_NS, Easing::In);
        self.deselect(now_ns);
        self.pressed = None;
    }

    /// Hides the menu at once: the play starts over, as lazer's new player has no menu.
    pub fn reset(&mut self) {
        let pointer = self.pointer;
        *self = GameplayMenu {
            pointer,
            ..GameplayMenu::default()
        };
    }

    /// Follows the mouse with `event`, whether the menu shows or not: a menu that shows
    /// hovers the button under it. Call it for every event before
    /// [`on_event`](Self::on_event).
    pub fn follow_pointer(&mut self, event: &InputEvent) {
        if let InputKind::MouseMove { x, y } = event.kind {
            self.pointer = Some(Vec2::new(x, y));
        }
    }

    /// Takes one input event on a window of `size` units, with the menu key it is (if any),
    /// after [`follow_pointer`](Self::follow_pointer). Returns the button it clicks, and
    /// whether the menu took the event (it then reaches nothing else).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (OnPressed, Button)
    pub fn on_event(
        &mut self,
        event: &InputEvent,
        key: Option<MenuKey>,
        size: [f32; 2],
    ) -> (Option<MenuButton>, bool) {
        let time = event.time_ns;
        if !self.state.visible {
            return (None, false);
        }
        let buttons = self.state.kind.buttons();
        if let Some(key) = key {
            let clicked = match key {
                MenuKey::SelectPrevious => {
                    self.select_previous(time);
                    None
                }
                MenuKey::SelectNext => {
                    self.select_next(time);
                    None
                }
                MenuKey::Select => self.selected.map(|index| buttons[index]),
            };
            return (clicked, true);
        }
        let layout = MenuLayout::new(size[0], size[1], self.state.kind);
        match event.kind {
            InputKind::MouseMove { .. } => {
                let hovered = self
                    .pointer
                    .and_then(|at| layout.button_at(at, buttons.len()));
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/DialogButton.cs (OnHoverLost)
                if hovered != self.hovered
                    && let Some(left) = self.hovered
                    && self.selected == Some(left)
                {
                    self.set_selected(None, time);
                }
                self.hovered = hovered;
                // The button's `OnMouseMove`: moving over it selects it.
                if let Some(index) = hovered {
                    self.set_selected(Some(index), time);
                }
                (None, false)
            }
            // Ported from osu-framework 2026.921.1: osu.Framework/Input/MouseButtonEventManager.cs (HandleButtonUp)
            // A click is the left button going up over the button it went down on.
            InputKind::MouseButton {
                button: MouseButton::Left,
                down,
            } => {
                if down {
                    self.pressed = self.hovered;
                    return (None, self.hovered.is_some());
                }
                let clicked = self
                    .pressed
                    .take()
                    .filter(|&index| self.hovered == Some(index))
                    .map(|index| buttons[index]);
                (clicked, clicked.is_some())
            }
            _ => (None, false),
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/Containers/SelectionCycleFillFlowContainer.cs (SelectNext, SelectPrevious)
    fn select_next(&mut self, now_ns: u64) {
        let count = self.state.kind.buttons().len();
        let next = match self.selected {
            Some(index) if index + 1 < count => index + 1,
            _ => 0,
        };
        self.set_selected(Some(next), now_ns);
    }

    fn select_previous(&mut self, now_ns: u64) {
        let count = self.state.kind.buttons().len();
        let previous = match self.selected {
            Some(index) if index > 0 => index - 1,
            _ => count - 1,
        };
        self.set_selected(Some(previous), now_ns);
    }

    fn deselect(&mut self, now_ns: u64) {
        self.set_selected(None, now_ns);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Graphics/UserInterface/DialogButton.cs (selectionChanged)
    /// Selects `index` (deselecting the one before), widening and narrowing the buttons.
    fn set_selected(&mut self, index: Option<usize>, now_ns: u64) {
        if self.selected == index {
            return;
        }
        if let Some(old) = self.selected {
            self.state.widths[old].retarget(IDLE_WIDTH, now_ns, HOVER_NS / 2, Easing::OutQuint);
        }
        if let Some(new) = index {
            self.state.widths[new].retarget(HOVER_WIDTH, now_ns, HOVER_NS, Easing::OutQuint);
        }
        self.selected = index;
    }
}

/// The menus' localised texts.
#[derive(Debug, Clone)]
pub struct MenuLabels {
    pub paused: String,
    pub failed: String,
    pub continue_button: String,
    pub retry_button: String,
    pub quit_button: String,
    /// The retry count, with [`crate::hud::OFFSET_PLACEHOLDER`] for the number.
    pub retry_count: String,
    /// The song progress, with the placeholder for the percentage.
    pub song_progress: String,
    /// The accuracy, with the placeholder for the percentage.
    pub accuracy: String,
}

/// Every character a number below the buttons has, as long as the longest.
const WARM_NUMBER: &str = "0123456789.%0123456789";

/// A line of the information: a localised text around a bold number.
#[derive(Debug, Clone)]
struct InfoLine {
    template: Template,
    prefix: Layout,
    number: Layout,
    suffix: Layout,
    /// [`WARM_NUMBER`] laid out, for its glyphs.
    warm: Layout,
}

impl InfoLine {
    fn new(label: &str) -> InfoLine {
        InfoLine {
            template: Template::new(label),
            prefix: Layout::new(),
            number: Layout::new(),
            suffix: Layout::new(),
            warm: Layout::new(),
        }
    }

    fn width(&self) -> f32 {
        self.prefix.width() + self.number.width() + self.suffix.width()
    }

    fn height(&self) -> f32 {
        self.prefix
            .height()
            .max(self.number.height())
            .max(self.suffix.height())
    }
}

/// The draw side of the menus.
pub struct MenuView {
    registry: Arc<FontRegistry>,
    ui: FamilyId,
    heading: FamilyId,
    context: LayoutContext,
    /// The window size the layouts are for.
    frame: [f32; 2],
    /// The information the number layouts show, the accuracy as bits (NaN equals itself);
    /// `None` lays them out on the next update.
    info_shown: Option<(u32, u8, u64)>,
    titles: [(String, Layout); 2],
    /// Continue, retry, quit.
    buttons: [(String, Layout); MAX_BUTTONS],
    /// Retry count, song progress, accuracy.
    info: [InfoLine; 3],
}

impl std::fmt::Debug for MenuView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuView")
            .field("frame", &self.frame)
            .field("info_shown", &self.info_shown)
            .finish_non_exhaustive()
    }
}

impl MenuView {
    /// The menus with fonts from `registry` and the localised `labels`. Lays out every text
    /// once, so that later layouts do not allocate: call it off the hot paths.
    pub fn new(registry: Arc<FontRegistry>, labels: &MenuLabels) -> MenuView {
        // An unknown family lays out nothing; the built-in registry has both.
        let ui = registry.family(UI_FAMILY).unwrap_or(FamilyId(u32::MAX));
        let heading = registry.family(HEADING_FAMILY).unwrap_or(ui);
        let text = |s: &String| (s.clone(), Layout::new());
        let mut view = MenuView {
            registry,
            ui,
            heading,
            context: LayoutContext::new(),
            frame: [0.0, 0.0],
            info_shown: None,
            titles: [text(&labels.paused), text(&labels.failed)],
            buttons: [
                text(&labels.continue_button),
                text(&labels.retry_button),
                text(&labels.quit_button),
            ],
            info: [
                InfoLine::new(&labels.retry_count),
                InfoLine::new(&labels.song_progress),
                InfoLine::new(&labels.accuracy),
            ],
        };
        view.lay_out_texts(1.0);
        let bold = TextStyle::new(view.ui, BOLD, INFO_SIZE);
        for line in &mut view.info {
            view.context
                .layout_into(&view.registry, WARM_NUMBER, &bold, &mut line.warm);
            view.context
                .layout_into(&view.registry, WARM_NUMBER, &bold, &mut line.number);
        }
        view
    }

    /// Lays out the texts that do not change at interface scale `scale`.
    fn lay_out_texts(&mut self, scale: f32) {
        let title = TextStyle::new(self.heading, TITLE_WEIGHT, TITLE_SIZE * scale);
        for (text, layout) in &mut self.titles {
            self.context
                .layout_into(&self.registry, text, &title, layout);
        }
        let button = TextStyle::new(self.ui, BOLD, BUTTON_TEXT_SIZE * scale);
        for (text, layout) in &mut self.buttons {
            self.context
                .layout_into(&self.registry, text, &button, layout);
        }
        let info = TextStyle::new(self.ui, INFO_WEIGHT, INFO_SIZE * scale);
        for line in &mut self.info {
            self.context.layout_into(
                &self.registry,
                &line.template.prefix,
                &info,
                &mut line.prefix,
            );
            self.context.layout_into(
                &self.registry,
                &line.template.suffix,
                &info,
                &mut line.suffix,
            );
        }
    }

    /// Asks `glyphs` for every glyph the menus show. Returns how many are not ready yet.
    pub fn request_glyphs(&self, glyphs: &mut GlyphCache) -> usize {
        self.titles
            .iter()
            .chain(&self.buttons)
            .map(|(_, layout)| layout)
            .chain(
                self.info
                    .iter()
                    .flat_map(|line| [&line.prefix, &line.suffix, &line.warm]),
            )
            .flat_map(Layout::glyphs)
            .filter(|g| matches!(glyphs.glyph(GlyphKey::of(g)), slam_text::Glyph::Pending))
            .count()
    }

    /// Lays out what changed for `menu` on a window of `width` × `height` pixels.
    /// Allocation-free.
    pub fn update(&mut self, width: f32, height: f32, menu: &MenuSnapshot) {
        let scale = MenuLayout::scale(width, height);
        if self.frame != [width, height] {
            self.frame = [width, height];
            self.info_shown = None;
            self.lay_out_texts(scale);
        }
        let info = menu.info;
        let key = (info.retries, info.progress, info.accuracy.to_bits());
        if self.info_shown == Some(key) {
            return;
        }
        self.info_shown = Some(key);
        let numbers: [NumberText; 3] = [
            number_text(format_args!("{}", menu.info.retries)),
            number_text(format_args!("{}%", menu.info.progress)),
            format_accuracy(menu.info.accuracy),
        ];
        let bold = TextStyle::new(self.ui, BOLD, INFO_SIZE * scale);
        for (line, number) in self.info.iter_mut().zip(&numbers) {
            self.context
                .layout_into(&self.registry, number.as_str(), &bold, &mut line.number);
        }
    }

    /// Writes the menu's rectangles at engine time `now_ns`, tinting `white`: the dark
    /// background and the buttons. Allocation-free once `batch` has room.
    pub fn push_shapes(
        &self,
        batch: &mut SpriteBatch,
        menu: &MenuSnapshot,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let alpha = menu.alpha.value_at(now_ns);
        if alpha <= 0.0 {
            return;
        }
        let [width, height] = self.frame;
        let mut rect = |position: [f32; 2], size: [f32; 2], colour: Color| {
            let mut sprite = Sprite::new(white.texture, white.uv, position, size);
            sprite.colour = colour;
            batch.push(&sprite);
        };
        rect(
            [0.0, 0.0],
            [width, height],
            Color {
                a: BACKGROUND_ALPHA * alpha,
                ..Color::BLACK
            },
        );
        let buttons = menu.kind.buttons();
        let layout = MenuLayout::new(width, height, menu.kind);
        for (index, button) in buttons.iter().enumerate() {
            let ([x, y], [w, h]) = layout.button(index);
            let part = menu.widths[index].value_at(now_ns);
            let coloured = w * part;
            rect(
                [x + (w - coloured) / 2.0, y],
                [coloured, h],
                Color {
                    a: alpha,
                    ..button.colour()
                },
            );
        }
    }

    /// Writes the menu's text at engine time `now_ns` into a batch drawn with the MSDF
    /// shader. Allocation-free once `batch` has room.
    pub fn push_text(
        &self,
        batch: &mut SpriteBatch,
        menu: &MenuSnapshot,
        now_ns: u64,
        glyphs: &mut GlyphCache,
    ) -> TextStats {
        let mut stats = TextStats::default();
        let alpha = menu.alpha.value_at(now_ns);
        if alpha <= 0.0 {
            return stats;
        }
        let [width, height] = self.frame;
        let mut text = |layout: &Layout, position: [f32; 2], colour: Color| {
            let s = push_layout(batch, layout, position, colour, glyphs);
            stats.drawn += s.drawn;
            stats.pending += s.pending;
        };
        let white = Color {
            a: alpha,
            ..Color::WHITE
        };
        let buttons = menu.kind.buttons();
        let layout = MenuLayout::new(width, height, menu.kind);
        let title = match menu.kind {
            MenuKind::Pause => &self.titles[0].1,
            MenuKind::Fail => &self.titles[1].1,
        };
        text(
            title,
            [
                (width - title.width()) / 2.0,
                layout.title_y - title.height() / 2.0,
            ],
            Color { a: alpha, ..YELLOW },
        );
        for (index, button) in buttons.iter().enumerate() {
            let label = &self.buttons[*button as usize].1;
            let ([x, y], [w, h]) = layout.button(index);
            text(
                label,
                [
                    x + (w - label.width()) / 2.0,
                    y + (h - label.height()) / 2.0,
                ],
                white,
            );
        }
        let total: f32 = self.info.iter().map(InfoLine::height).sum();
        let mut y = layout.info_y - total / 2.0;
        for line in &self.info {
            let mut x = (width - line.width()) / 2.0;
            for part in [&line.prefix, &line.number, &line.suffix] {
                text(part, [x, y], white);
                x += part.width();
            }
            y += line.height();
        }
        stats
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The menus' texts in English.
    pub(crate) fn labels() -> MenuLabels {
        let placeholder = crate::hud::OFFSET_PLACEHOLDER;
        MenuLabels {
            paused: "paused".into(),
            failed: "failed".into(),
            continue_button: "Continue".into(),
            retry_button: "Retry".into(),
            quit_button: "Quit".into(),
            retry_count: format!("Retry count: {placeholder}"),
            song_progress: format!("Song progress: {placeholder}"),
            accuracy: format!("Accuracy: {placeholder}"),
        }
    }

    const SIZE: [f32; 2] = [1024.0, 768.0];

    fn info() -> MenuInfo {
        MenuInfo {
            retries: 2,
            progress: 40,
            accuracy: 0.95,
        }
    }

    fn key(menu: &mut GameplayMenu, key: MenuKey, time_ns: u64) -> (Option<MenuButton>, bool) {
        let event = InputEvent {
            time_ns,
            kind: InputKind::Key {
                scancode: 0,
                down: true,
            },
        };
        menu.on_event(&event, Some(key), SIZE)
    }

    fn mouse(menu: &mut GameplayMenu, kind: InputKind, time_ns: u64) -> (Option<MenuButton>, bool) {
        let event = InputEvent { time_ns, kind };
        menu.follow_pointer(&event);
        menu.on_event(&event, None, SIZE)
    }

    fn move_to(menu: &mut GameplayMenu, at: [f32; 2], time_ns: u64) {
        mouse(menu, InputKind::MouseMove { x: at[0], y: at[1] }, time_ns);
    }

    fn centre(kind: MenuKind, index: usize) -> [f32; 2] {
        let ([x, y], [w, h]) = MenuLayout::new(SIZE[0], SIZE[1], kind).button(index);
        [x + w / 2.0, y + h / 2.0]
    }

    fn button(down: bool) -> InputKind {
        InputKind::MouseButton {
            button: MouseButton::Left,
            down,
        }
    }

    #[test]
    fn menus_have_lazers_buttons() {
        use MenuButton::*;
        assert_eq!(MenuKind::Pause.buttons(), &[Continue, Retry, Quit]);
        assert_eq!(MenuKind::Fail.buttons(), &[Retry, Quit]);
        assert_eq!(MenuKind::Pause.back_button(), Continue);
        assert_eq!(MenuKind::Fail.back_button(), Quit);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Visual/Gameplay/TestSceneGameplayMenuOverlay.cs (TestKeyboardSelection, TestKeyboardSelectionWithoutSelection)
    #[test]
    fn keys_cycle_from_the_ends() {
        let mut menu = GameplayMenu::default();
        menu.show(MenuKind::Pause, info(), 0, SIZE);
        assert_eq!(menu.selected(), None);
        key(&mut menu, MenuKey::SelectNext, 1);
        assert_eq!(menu.selected(), Some(0));
        key(&mut menu, MenuKey::SelectNext, 2);
        key(&mut menu, MenuKey::SelectNext, 3);
        assert_eq!(menu.selected(), Some(2));
        key(&mut menu, MenuKey::SelectNext, 4);
        assert_eq!(menu.selected(), Some(0), "wraps around");
        key(&mut menu, MenuKey::SelectPrevious, 5);
        assert_eq!(menu.selected(), Some(2));

        menu.hide(6);
        menu.show(MenuKind::Fail, info(), 7, SIZE);
        assert_eq!(menu.selected(), None, "showing deselects");
        key(&mut menu, MenuKey::SelectPrevious, 8);
        assert_eq!(menu.selected(), Some(1), "up starts from the last");
    }

    #[test]
    fn select_clicks_the_selected_button_only() {
        let mut menu = GameplayMenu::default();
        menu.show(MenuKind::Pause, info(), 0, SIZE);
        assert_eq!(key(&mut menu, MenuKey::Select, 1), (None, true));
        key(&mut menu, MenuKey::SelectPrevious, 2);
        assert_eq!(
            key(&mut menu, MenuKey::Select, 3),
            (Some(MenuButton::Quit), true)
        );
    }

    #[test]
    fn hidden_menu_takes_nothing() {
        let mut menu = GameplayMenu::default();
        assert_eq!(key(&mut menu, MenuKey::Select, 0), (None, false));
        menu.show(MenuKind::Pause, info(), 0, SIZE);
        menu.hide(1);
        assert_eq!(key(&mut menu, MenuKey::SelectNext, 2), (None, false));
        assert_eq!(menu.selected(), None);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Tests/Visual/Gameplay/TestSceneGameplayMenuOverlay.cs (TestMouseSelectionAfterKeyboardSelection)
    #[test]
    fn moving_the_mouse_selects_and_leaving_deselects() {
        let mut menu = GameplayMenu::default();
        move_to(&mut menu, centre(MenuKind::Pause, 1), 0);
        menu.show(MenuKind::Pause, info(), 1, SIZE);
        assert_eq!(menu.selected(), None, "hovering alone does not select");
        let [x, y] = centre(MenuKind::Pause, 1);
        move_to(&mut menu, [x + 1.0, y], 2);
        assert_eq!(menu.selected(), Some(1));
        key(&mut menu, MenuKey::SelectNext, 3);
        assert_eq!(
            menu.selected(),
            Some(2),
            "keys go on from the mouse's button"
        );
        move_to(&mut menu, centre(MenuKind::Pause, 0), 4);
        assert_eq!(menu.selected(), Some(0));
        move_to(&mut menu, [5.0, 5.0], 5);
        assert_eq!(
            menu.selected(),
            None,
            "leaving the selected button deselects it"
        );
    }

    #[test]
    fn a_click_is_a_press_and_release_on_one_button() {
        let mut menu = GameplayMenu::default();
        menu.show(MenuKind::Fail, info(), 0, SIZE);
        move_to(&mut menu, centre(MenuKind::Fail, 0), 1);
        assert_eq!(mouse(&mut menu, button(true), 2), (None, true));
        assert_eq!(
            mouse(&mut menu, button(false), 3),
            (Some(MenuButton::Retry), true)
        );
        // Pressed on one button, released on another: no click.
        mouse(&mut menu, button(true), 4);
        move_to(&mut menu, centre(MenuKind::Fail, 1), 5);
        assert_eq!(mouse(&mut menu, button(false), 6), (None, false));
        // Outside the buttons the press is not taken.
        move_to(&mut menu, [5.0, 5.0], 7);
        assert_eq!(mouse(&mut menu, button(true), 8), (None, false));
    }

    #[test]
    fn selection_widens_the_button() {
        let mut menu = GameplayMenu::default();
        menu.show(MenuKind::Pause, info(), 0, SIZE);
        key(&mut menu, MenuKey::SelectNext, 0);
        let widths = menu.snapshot().widths;
        assert_eq!(widths[0].value_at(0), IDLE_WIDTH);
        assert_eq!(widths[0].value_at(HOVER_NS), HOVER_WIDTH);
        let half = widths[0].value_at(HOVER_NS / 2);
        assert!(half > IDLE_WIDTH && half < HOVER_WIDTH);
        assert_eq!(widths[1].value_at(HOVER_NS), IDLE_WIDTH);
    }

    #[test]
    fn menu_fades_from_where_it_is() {
        let mut menu = GameplayMenu::default();
        menu.show(MenuKind::Pause, info(), 1_000, SIZE);
        let alpha = menu.snapshot().alpha;
        assert_eq!(alpha.value_at(1_000), 0.0);
        assert_eq!(alpha.value_at(1_000 + TRANSITION_NS), 1.0);
        // Easing.In: a quarter of the way at half the time.
        let half = alpha.value_at(1_000 + TRANSITION_NS / 2);
        assert!((half - 0.25).abs() < 1e-6, "{half}");
        menu.hide(1_000 + TRANSITION_NS / 2);
        let alpha = menu.snapshot().alpha;
        assert!((alpha.value_at(1_000 + TRANSITION_NS / 2) - 0.25).abs() < 1e-6);
        assert_eq!(alpha.value_at(1_000 + 2 * TRANSITION_NS), 0.0);
        menu.reset();
        assert_eq!(menu.snapshot().alpha.value_at(0), 0.0);
        assert!(!menu.is_visible());
    }

    #[test]
    fn layout_centres_the_buttons() {
        let layout = MenuLayout::new(2048.0, 1536.0, MenuKind::Pause);
        assert_eq!(layout.scale, 2.0);
        let block = (3.0 * 80.0 + 2.0 * 2.0) * 2.0;
        let ([x, top], [w, h]) = layout.button(0);
        assert_eq!([x, w, h], [100.0, 2048.0 - 200.0, 160.0]);
        assert_eq!(top, (1536.0 - block) / 2.0);
        let (bottom, _) = layout.button(2);
        assert_eq!(bottom[1] + 160.0, top + block);
        assert_eq!(layout.title_y, top / 2.0);
        assert_eq!(layout.info_y, top + block + top / 2.0);
        // Wide windows scale by the height.
        assert_eq!(MenuLayout::new(4000.0, 768.0, MenuKind::Fail).scale, 1.0);
        // The fail menu's footer takes its place below the information.
        let fail = MenuLayout::new(2048.0, 1536.0, MenuKind::Fail);
        let fail_block = (2.0 * 80.0 + 2.0) * 2.0;
        let (fail_top, _) = fail.button(0);
        assert_eq!(fail_top[1], (1536.0 - fail_block - 100.0) / 2.0);
        assert_eq!(fail.info_y, fail_top[1] + fail_block + fail_top[1] / 2.0);
        assert_eq!(layout.button_at(Vec2::new(99.0, top + 1.0), 3), None);
        assert_eq!(layout.button_at(Vec2::new(100.0, top), 3), Some(0));
        assert_eq!(
            layout.button_at(Vec2::new(100.0, top + 160.0), 3),
            None,
            "the gap"
        );
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (getSongProgress)
    #[test]
    fn song_progress_is_clamped_and_truncated() {
        assert_eq!(song_progress(500.0, 1000.0, 2000.0), 0);
        assert_eq!(song_progress(1000.0, 1000.0, 2000.0), 0);
        assert_eq!(song_progress(1999.0, 1000.0, 2000.0), 99);
        assert_eq!(song_progress(2500.0, 1000.0, 2000.0), 100);
        assert_eq!(song_progress(1500.0, 1000.0, 1000.0), 0, "no length");
    }

    #[test]
    fn view_lays_out_the_information() {
        let mut view = MenuView::new(Arc::new(FontRegistry::with_builtin()), &labels());
        let mut menu = GameplayMenu::default();
        menu.show(MenuKind::Pause, info(), 0, SIZE);
        view.update(1280.0, 720.0, &menu.snapshot());
        assert_eq!(view.info[0].number.glyphs().len(), 1);
        assert_eq!(view.info[1].number.glyphs().len(), 3);
        assert_eq!(view.info[2].number.glyphs().len(), 6);
        assert!(view.info[0].prefix.width() > 0.0);
        assert_eq!(view.info[0].suffix.glyphs().len(), 0);
    }
}
