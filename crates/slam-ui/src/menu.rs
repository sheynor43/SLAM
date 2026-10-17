//! The main menu: its update-side state and layout, input-agnostic (the application maps
//! its events to [`MenuInput`]). The draw side is [`view::MenuView`].

use crate::theme::Theme;

pub mod view;

pub use view::{MenuLabels, MenuView};

/// What the menu understands; the application turns keys and mouse events into these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuInput {
    Up,
    Down,
    Activate,
    Back,
    /// Pointer moved to `x`, `y` in layout units (the units of [`MenuLayout`]).
    PointerMove {
        x: f32,
        y: f32,
    },
    /// Primary button pressed at `x`, `y`.
    PointerPress {
        x: f32,
        y: f32,
    },
}

/// What the menu asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Play,
    Settings,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    Play,
    Settings,
    Exit,
}

impl MenuItem {
    /// The items, top to bottom.
    pub const ALL: [MenuItem; 3] = [MenuItem::Play, MenuItem::Settings, MenuItem::Exit];

    /// Whether the item can be selected and activated.
    pub const fn enabled(self) -> bool {
        true
    }

    fn action(self) -> Option<MenuAction> {
        match self {
            MenuItem::Play => Some(MenuAction::Play),
            MenuItem::Settings => Some(MenuAction::Settings),
            MenuItem::Exit => Some(MenuAction::Exit),
        }
    }
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

    pub fn centre(&self) -> [f32; 2] {
        [self.x + self.width / 2.0, self.y + self.height / 2.0]
    }
}

/// Where the menu's title and buttons are: a vertically and horizontally centred column.
/// A pure function of the window size and theme, shared by hit testing (update) and
/// drawing, so they agree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuLayout {
    pub title: Rect,
    pub buttons: [Rect; MenuItem::ALL.len()],
    /// Pixels per reference unit: text sizes scale with it.
    pub scale: f32,
}

impl MenuLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> MenuLayout {
        let scale = theme.scale(height);
        let count = MenuItem::ALL.len() as f32;
        let title_height = theme.title_size * scale;
        let (bw, bh) = (theme.button_width * scale, theme.button_height * scale);
        let spacing = theme.button_spacing * scale;
        let block = title_height + theme.title_gap * scale + count * bh + (count - 1.0) * spacing;
        let top = (height - block) / 2.0;
        let left = (width - bw) / 2.0;
        let first = top + title_height + theme.title_gap * scale;
        let mut buttons = [Rect::default(); MenuItem::ALL.len()];
        for (i, button) in buttons.iter_mut().enumerate() {
            *button = Rect {
                x: left,
                y: first + i as f32 * (bh + spacing),
                width: bw,
                height: bh,
            };
        }
        MenuLayout {
            title: Rect {
                x: left,
                y: top,
                width: bw,
                height: title_height,
            },
            buttons,
            scale,
        }
    }

    /// The item whose button contains the point.
    pub fn item_at(&self, x: f32, y: f32) -> Option<MenuItem> {
        MenuItem::ALL
            .into_iter()
            .zip(&self.buttons)
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(item, _)| item)
    }
}

/// What the draw side needs of the menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuSnapshot {
    /// Index into [`MenuItem::ALL`] of the selected or hovered item.
    pub selected: usize,
}

/// The menu's state on the update thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MainMenu {
    selected: usize,
}

impl MainMenu {
    pub fn new() -> MainMenu {
        MainMenu::default()
    }

    pub fn selected(&self) -> MenuItem {
        MenuItem::ALL[self.selected]
    }

    pub fn snapshot(&self) -> MenuSnapshot {
        MenuSnapshot {
            selected: self.selected,
        }
    }

    /// Moves the selection one item, wrapping and skipping
    /// disabled items.
    fn step(&mut self, forward: bool) {
        let len = MenuItem::ALL.len();
        for _ in 0..len {
            self.selected = if forward {
                (self.selected + 1) % len
            } else {
                (self.selected + len - 1) % len
            };
            if MenuItem::ALL[self.selected].enabled() {
                return;
            }
        }
    }

    /// Applies `input`; `layout` places the buttons for pointer input. Does not allocate.
    pub fn input(&mut self, input: MenuInput, layout: &MenuLayout) -> Option<MenuAction> {
        match input {
            MenuInput::Up => self.step(false),
            MenuInput::Down => self.step(true),
            MenuInput::Activate => return self.selected().action(),
            MenuInput::Back => return Some(MenuAction::Exit),
            MenuInput::PointerMove { x, y } => {
                if let Some(index) = Self::enabled_at(layout, x, y) {
                    self.selected = index;
                }
            }
            MenuInput::PointerPress { x, y } => {
                if let Some(index) = Self::enabled_at(layout, x, y) {
                    self.selected = index;
                    return MenuItem::ALL[index].action();
                }
            }
        }
        None
    }

    fn enabled_at(layout: &MenuLayout, x: f32, y: f32) -> Option<usize> {
        let item = layout.item_at(x, y).filter(|item| item.enabled())?;
        MenuItem::ALL.iter().position(|i| *i == item)
    }
}

#[cfg(test)]
mod tests;
