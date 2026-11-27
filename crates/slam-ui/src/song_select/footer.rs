//! The footer bar of the song select (ADR-0175): Back, Mods, the pinned mod presets, a
//! button that pins the current mods and one that resets them. Geometry only: the bar and
//! where its buttons are for a given number of presets, shared by hit testing (update) and
//! drawing.

use super::footer_anim::FooterPresence;
use crate::menu::Rect;
use crate::theme::Theme;

/// Most presets the bar holds.
pub const MAX_PRESETS: usize = 6;

/// A button of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FooterButton {
    Back,
    Mods,
    /// The pinned preset at this position.
    Preset(usize),
    /// Pins the current mods as a preset.
    Pin,
    /// Deselects every mod.
    Reset,
}

/// Which buttons the bar shows beyond Back and Mods.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FooterState {
    pub presets: usize,
    pub pin: bool,
    pub reset: bool,
}

/// The bar and the sizes its buttons are laid out with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FooterLayout {
    /// The strip along the bottom of the window.
    pub bar: Rect,
    /// `tan` of the buttons' skew's angle is the lean; this is the angle in radians.
    skew: f32,
    margin: f32,
    width: f32,
    top: f32,
    height: f32,
    gap: f32,
    button_width: f32,
    mods_width: f32,
    preset_width: f32,
    pin_width: f32,
}

/// Where the buttons are for one [`FooterState`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FooterButtons {
    pub back: Rect,
    pub mods: Rect,
    pub presets: [Rect; MAX_PRESETS],
    pub preset_count: usize,
    pub pin: Option<Rect>,
    pub reset: Option<Rect>,
    skew: f32,
    bar: Rect,
}

impl FooterLayout {
    pub fn new(width: f32, height: f32, theme: &Theme) -> FooterLayout {
        let scale = theme.scale(height);
        let bar_height = theme.footer_height * scale;
        let button_height = bar_height * theme.footer_button_share;
        FooterLayout {
            bar: Rect {
                x: 0.0,
                y: height - bar_height,
                width,
                height: bar_height,
            },
            skew: theme.button_skew(),
            margin: theme.song_margin * scale,
            width,
            top: height - bar_height + (bar_height - button_height) / 2.0,
            height: button_height,
            gap: theme.footer_gap * scale,
            button_width: theme.footer_button_width * scale,
            mods_width: theme.footer_mods_width * scale,
            preset_width: theme.footer_preset_width * scale,
            pin_width: theme.footer_pin_width * scale,
        }
    }

    /// The buttons of the bar showing `state`, left to right in the order of
    /// [`FooterButton`]. Presets past [`MAX_PRESETS`] are not shown.
    pub fn buttons(&self, state: FooterState) -> FooterButtons {
        // Sheared buttons lean right at their top; the gap keeps their edges apart.
        let lean = self.height * self.skew.tan();
        let mut x = self.margin;
        let mut place = |width: f32| {
            let rect = Rect {
                x,
                y: self.top,
                width,
                height: self.height,
            };
            x += width + self.gap + lean;
            rect
        };
        let back = place(self.button_width);
        let mods = place(self.mods_width);
        // Presets that would run past the window (or over the pin and reset buttons) are
        // not shown; a narrow window shows fewer.
        let step = self.gap + lean;
        let tail = state.pin as u32 as f32 * (self.pin_width + step)
            + state.reset as u32 as f32 * (self.button_width + step);
        let room = self.width - self.margin - tail - (mods.x + mods.width + step);
        let fit = if room > 0.0 {
            (room / (self.preset_width + step)).floor() as usize
        } else {
            0
        };
        let preset_count = state.presets.min(MAX_PRESETS).min(fit);
        let mut presets = [Rect::default(); MAX_PRESETS];
        for slot in presets.iter_mut().take(preset_count) {
            *slot = place(self.preset_width);
        }
        let pin = state.pin.then(|| place(self.pin_width));
        let reset = state.reset.then(|| place(self.button_width));
        FooterButtons {
            back,
            mods,
            presets,
            preset_count,
            pin,
            reset,
            skew: self.skew,
            bar: self.bar,
        }
    }
}

impl FooterLayout {
    /// The buttons with the optional ones partly open, as [`FooterAnim`] has them: a button
    /// with presence `p` takes `p` of its place, so the ones after it slide as it opens or
    /// closes. For drawing only; hit testing uses [`FooterLayout::buttons`].
    pub fn animated(&self, presence: &FooterPresence) -> FooterButtons {
        let lean = self.height * self.skew.tan();
        let step = self.gap + lean;
        let mut x = self.margin;
        let mut place = |width: f32, open: f32| {
            let rect = Rect {
                x,
                y: self.top,
                width,
                height: self.height,
            };
            x += (width + step) * open;
            rect
        };
        let back = place(self.button_width, 1.0);
        let mods = place(self.mods_width, 1.0);
        let mut presets = [Rect::default(); MAX_PRESETS];
        let mut preset_count = 0;
        for (i, slot) in presets.iter_mut().enumerate() {
            let open = presence.presets[i].clamp(0.0, 1.0);
            *slot = place(self.preset_width, open);
            if open > 0.0 {
                preset_count = i + 1;
            }
        }
        let pin = (presence.pin > 0.0).then(|| place(self.pin_width, presence.pin.min(1.0)));
        let reset =
            (presence.reset > 0.0).then(|| place(self.button_width, presence.reset.min(1.0)));
        FooterButtons {
            back,
            mods,
            presets,
            preset_count,
            pin,
            reset,
            skew: self.skew,
            bar: self.bar,
        }
    }
}

impl FooterButtons {
    /// The button under the point, if any.
    pub fn at(&self, x: f32, y: f32) -> Option<FooterButton> {
        let hit = |r: &Rect| r.contains_skewed(x, y, self.skew);
        if hit(&self.back) {
            return Some(FooterButton::Back);
        }
        if hit(&self.mods) {
            return Some(FooterButton::Mods);
        }
        if let Some(i) = (0..self.preset_count).find(|&i| hit(&self.presets[i])) {
            return Some(FooterButton::Preset(i));
        }
        if self.pin.as_ref().is_some_and(hit) {
            return Some(FooterButton::Pin);
        }
        if self.reset.as_ref().is_some_and(hit) {
            return Some(FooterButton::Reset);
        }
        None
    }

    /// Whether the point is on the bar (a button or not).
    pub fn on_bar(&self, x: f32, y: f32) -> bool {
        self.bar.contains(x, y)
    }

    /// The rectangle of `button`, if it is shown.
    pub fn rect(&self, button: FooterButton) -> Option<Rect> {
        match button {
            FooterButton::Back => Some(self.back),
            FooterButton::Mods => Some(self.mods),
            FooterButton::Preset(i) => (i < self.preset_count).then(|| self.presets[i]),
            FooterButton::Pin => self.pin,
            FooterButton::Reset => self.reset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> FooterLayout {
        FooterLayout::new(1920.0, 1080.0, &Theme::default())
    }

    fn right_edge(buttons: &FooterButtons) -> f32 {
        let last = buttons
            .reset
            .or(buttons.pin)
            .or_else(|| {
                (buttons.preset_count > 0).then(|| buttons.presets[buttons.preset_count - 1])
            })
            .unwrap_or(buttons.mods);
        last.x + last.width
    }

    #[test]
    fn buttons_are_in_order_and_inside_the_bar() {
        let l = layout();
        let b = l.buttons(FooterState {
            presets: MAX_PRESETS,
            pin: true,
            reset: true,
        });
        assert!(b.back.x + b.back.width < b.mods.x);
        assert!(b.mods.x + b.mods.width < b.presets[0].x);
        for i in 1..MAX_PRESETS {
            assert!(b.presets[i - 1].x + b.presets[i - 1].width < b.presets[i].x);
        }
        let pin = b.pin.unwrap();
        let reset = b.reset.unwrap();
        assert!(b.presets[MAX_PRESETS - 1].x < pin.x && pin.x + pin.width < reset.x);
        assert!(right_edge(&b) < l.bar.width);
        for r in [b.back, b.mods, pin, reset] {
            assert!(r.y >= l.bar.y && r.y + r.height <= l.bar.y + l.bar.height);
        }
    }

    #[test]
    fn the_reset_and_pin_places_are_gone_when_hidden() {
        let l = layout();
        let shown = l.buttons(FooterState {
            presets: 2,
            pin: true,
            reset: true,
        });
        let hidden = l.buttons(FooterState {
            presets: 2,
            pin: false,
            reset: false,
        });
        assert!(hidden.pin.is_none() && hidden.reset.is_none());
        assert!(right_edge(&hidden) < right_edge(&shown));
        // The point where the reset button was now answers nothing.
        let [x, y] = shown.reset.unwrap().centre();
        assert_eq!(shown.at(x, y), Some(FooterButton::Reset));
        assert_eq!(hidden.at(x, y), None);
    }

    #[test]
    fn hit_testing_finds_each_button() {
        let b = layout().buttons(FooterState {
            presets: 3,
            pin: true,
            reset: true,
        });
        let at = |r: Rect| {
            let [x, y] = r.centre();
            b.at(x, y)
        };
        assert_eq!(at(b.back), Some(FooterButton::Back));
        assert_eq!(at(b.mods), Some(FooterButton::Mods));
        for i in 0..3 {
            assert_eq!(at(b.presets[i]), Some(FooterButton::Preset(i)));
        }
        assert_eq!(at(b.pin.unwrap()), Some(FooterButton::Pin));
        assert_eq!(at(b.reset.unwrap()), Some(FooterButton::Reset));
        assert_eq!(b.at(1800.0, b.back.y + 2.0), None);
        assert!(b.on_bar(1800.0, b.back.y + 2.0));
        assert!(!b.on_bar(1800.0, 10.0));
    }

    #[test]
    fn presets_past_the_maximum_are_not_shown() {
        let b = layout().buttons(FooterState {
            presets: 9,
            pin: false,
            reset: false,
        });
        assert_eq!(b.preset_count, MAX_PRESETS);
        assert_eq!(b.rect(FooterButton::Preset(MAX_PRESETS)), None);
    }
}
