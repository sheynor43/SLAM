//! The player popup's motion (ADR-0178): levels that follow what the popup shows, stepped
//! once per drawn frame. Pure and allocation-free; the draw side reads the levels.

use crate::theme::Theme;

/// What the levels head for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Target {
    /// The popup is open.
    pub open: bool,
    /// The button under the pointer.
    pub hover: Option<u8>,
    /// The button held down.
    pub pressed: Option<u8>,
    /// The pointer is on the bar or drags it.
    pub bar_hot: bool,
    /// The middle button shows "play".
    pub paused: bool,
    /// How far into the track, 0 ..= 1.
    pub progress: f32,
}

/// The levels, each 0 ..= 1 and linear in time unless noted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlayerAnim {
    last_ns: Option<u64>,
    /// 0 closed, 1 open.
    pub open: f32,
    /// Per button: how lit.
    pub hover: [f32; 3],
    /// Per button: how pressed.
    pub press: [f32; 3],
    /// How thick the bar is, 0 thin to 1 grown.
    pub bar: f32,
    /// The fill's share of the bar: eases to the position (not linear).
    pub fill: f32,
    /// 0 pause icon, 1 play icon.
    pub play: f32,
    /// The current texts' share of the cross-fade, 1 settled.
    pub text: f32,
    /// The current cover's share of the cross-fade, 1 settled.
    pub cover: f32,
}

impl Default for PlayerAnim {
    fn default() -> Self {
        PlayerAnim {
            last_ns: None,
            open: 0.0,
            hover: [0.0; 3],
            press: [0.0; 3],
            bar: 0.0,
            fill: 0.0,
            play: 0.0,
            text: 1.0,
            cover: 1.0,
        }
    }
}

/// Frames further apart than this are stepped as if they were this far: a window that was
/// not drawn does not jump the motion.
const MAX_STEP_NS: u64 = 100_000_000;

/// `level` moved towards `target` by `dt_ns` of a motion that takes `ms` end to end.
fn approach(level: f32, target: f32, dt_ns: u64, ms: f32) -> f32 {
    let step = if ms <= 0.0 {
        1.0
    } else {
        (dt_ns as f64 / (f64::from(ms) * 1e6)) as f32
    };
    if target > level {
        (level + step).min(target)
    } else {
        (level - step).max(target)
    }
}

impl PlayerAnim {
    /// The current texts replaced the ones shown: they fade in over the old.
    pub fn text_changed(&mut self) {
        self.text = 0.0;
    }

    /// The cover is another picture: it fades in over the old.
    pub fn cover_changed(&mut self) {
        self.cover = 0.0;
    }

    /// Whether anything of the popup is on screen.
    pub fn visible(&self, target: &Target) -> bool {
        target.open || self.open > 0.0
    }

    /// Steps the levels to `now_ns`. Without `effects` they take their targets at once.
    pub fn step(&mut self, now_ns: u64, target: &Target, theme: &Theme) {
        let play = if target.paused { 1.0 } else { 0.0 };
        // Closed and staying so: rest at the targets, so that opening starts from
        // nothing and its first step has no elapsed time.
        if !target.open && self.open <= 0.0 {
            *self = PlayerAnim {
                fill: target.progress,
                play,
                ..PlayerAnim::default()
            };
            return;
        }
        let dt = match self.last_ns {
            Some(last) => now_ns.saturating_sub(last).min(MAX_STEP_NS),
            None => 0,
        };
        self.last_ns = Some(now_ns);
        if !theme.effects {
            self.open = f32::from(target.open);
            for i in 0..3 {
                self.hover[i] = f32::from(target.hover == Some(i as u8));
                self.press[i] = f32::from(target.pressed == Some(i as u8));
            }
            self.bar = f32::from(target.bar_hot);
            self.fill = target.progress;
            self.play = play;
            self.text = 1.0;
            self.cover = 1.0;
            return;
        }
        let open_ms = if target.open {
            theme.player_open_ms
        } else {
            theme.player_close_ms
        };
        self.open = approach(self.open, f32::from(target.open), dt, open_ms);
        for i in 0..3 {
            let hover = f32::from(target.hover == Some(i as u8));
            self.hover[i] = approach(self.hover[i], hover, dt, theme.player_hover_ms);
            let press = f32::from(target.pressed == Some(i as u8));
            self.press[i] = approach(self.press[i], press, dt, theme.player_press_ms);
        }
        self.bar = approach(self.bar, f32::from(target.bar_hot), dt, theme.player_bar_ms);
        // Exponential: a share of the distance per step, 99% of it gone in `fill_ms`.
        let ms = f64::from(theme.player_fill_ms.max(1.0));
        let keep = (-4.6 * dt as f64 / (ms * 1e6)).exp() as f32;
        self.fill = target.progress + (self.fill - target.progress) * keep;
        self.play = approach(self.play, play, dt, theme.player_swap_ms);
        self.text = approach(self.text, 1.0, dt, theme.player_swap_ms);
        self.cover = approach(self.cover, 1.0, dt, theme.player_cover_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    fn open() -> Target {
        Target {
            open: true,
            ..Target::default()
        }
    }

    fn run(anim: &mut PlayerAnim, target: &Target, theme: &Theme, from_ms: u64, to_ms: u64) {
        for t in (from_ms..=to_ms).step_by(8) {
            anim.step(t * MS, target, theme);
        }
    }

    fn settled(theme: &Theme) -> PlayerAnim {
        let mut anim = PlayerAnim::default();
        anim.step(0, &open(), theme);
        run(&mut anim, &open(), theme, 8, 400);
        anim
    }

    #[test]
    fn opening_takes_its_time_and_ends_open() {
        let theme = Theme::default();
        let mut anim = PlayerAnim::default();
        anim.step(0, &open(), &theme);
        assert_eq!(anim.open, 0.0);
        anim.step(96 * MS, &open(), &theme);
        assert!(anim.open > 0.2 && anim.open < 0.8, "{}", anim.open);
        run(&mut anim, &open(), &theme, 104, 400);
        assert_eq!(anim.open, 1.0);
    }

    #[test]
    fn closing_animates_instead_of_vanishing() {
        let theme = Theme::default();
        let mut anim = settled(&theme);
        let closed = Target::default();
        anim.step(408 * MS, &closed, &theme);
        assert!(anim.visible(&closed));
        anim.step(448 * MS, &closed, &theme);
        assert!(anim.open > 0.0 && anim.open < 1.0, "{}", anim.open);
        assert!(anim.visible(&closed));
        run(&mut anim, &closed, &theme, 456, 700);
        assert_eq!(anim.open, 0.0);
        assert!(!anim.visible(&closed));
    }

    #[test]
    fn closing_can_turn_back_to_open_from_where_it_is() {
        let theme = Theme::default();
        let mut anim = settled(&theme);
        anim.step(408 * MS, &Target::default(), &theme);
        anim.step(456 * MS, &Target::default(), &theme);
        let mid = anim.open;
        assert!(mid > 0.0 && mid < 1.0);
        anim.step(464 * MS, &open(), &theme);
        assert!(anim.open > mid && anim.open < 1.0);
    }

    #[test]
    fn hover_brightens_in_its_time_and_fades_back() {
        let theme = Theme::default();
        let mut anim = settled(&theme);
        let hovered = Target {
            open: true,
            hover: Some(1),
            ..Target::default()
        };
        anim.step(408 * MS, &hovered, &theme);
        anim.step(483 * MS, &hovered, &theme);
        assert!(
            anim.hover[1] > 0.3 && anim.hover[1] < 0.7,
            "{}",
            anim.hover[1]
        );
        assert_eq!(anim.hover[0], 0.0);
        run(&mut anim, &hovered, &theme, 583, 700);
        assert_eq!(anim.hover[1], 1.0);
        run(&mut anim, &open(), &theme, 708, 900);
        assert_eq!(anim.hover[1], 0.0);
    }

    #[test]
    fn a_press_shrinks_faster_than_a_hover_lights() {
        let theme = Theme::default();
        let mut anim = settled(&theme);
        let held = Target {
            open: true,
            hover: Some(0),
            pressed: Some(0),
            ..Target::default()
        };
        anim.step(408 * MS, &held, &theme);
        anim.step(448 * MS, &held, &theme);
        assert!(anim.press[0] > anim.hover[0]);
        run(&mut anim, &open(), &theme, 548, 800);
        assert_eq!(anim.press[0], 0.0);
    }

    #[test]
    fn the_bar_grows_on_hover_and_the_fill_eases_to_the_position() {
        let theme = Theme::default();
        let mut anim = settled(&theme);
        let hot = Target {
            open: true,
            bar_hot: true,
            progress: 0.8,
            ..Target::default()
        };
        anim.step(408 * MS, &hot, &theme);
        anim.step(448 * MS, &hot, &theme);
        assert!(anim.bar > 0.0 && anim.bar < 1.0);
        assert!(anim.fill > 0.0 && anim.fill < 0.8, "{}", anim.fill);
        run(&mut anim, &hot, &theme, 548, 1200);
        assert_eq!(anim.bar, 1.0);
        assert!((anim.fill - 0.8).abs() < 0.01);
    }

    #[test]
    fn the_play_icon_cross_fades_and_texts_and_cover_settle() {
        let theme = Theme::default();
        let mut anim = settled(&theme);
        let paused = Target {
            open: true,
            paused: true,
            ..Target::default()
        };
        anim.step(408 * MS, &paused, &theme);
        anim.step(468 * MS, &paused, &theme);
        assert!(anim.play > 0.0 && anim.play < 1.0);
        anim.text_changed();
        anim.cover_changed();
        assert_eq!((anim.text, anim.cover), (0.0, 0.0));
        run(&mut anim, &paused, &theme, 568, 1200);
        assert_eq!((anim.play, anim.text, anim.cover), (1.0, 1.0, 1.0));
    }

    #[test]
    fn without_effects_everything_is_at_once() {
        let theme = Theme {
            effects: false,
            ..Theme::default()
        };
        let mut anim = PlayerAnim::default();
        let held = Target {
            open: true,
            hover: Some(2),
            progress: 0.5,
            ..Target::default()
        };
        anim.step(0, &held, &theme);
        anim.step(MS, &held, &theme);
        assert_eq!((anim.open, anim.hover[2], anim.fill), (1.0, 1.0, 0.5));
        anim.step(2 * MS, &Target::default(), &theme);
        assert!(!anim.visible(&Target::default()));
    }

    #[test]
    fn a_long_gap_does_not_jump_the_motion() {
        let theme = Theme::default();
        let mut anim = PlayerAnim::default();
        anim.step(0, &open(), &theme);
        anim.step(10_000 * MS, &open(), &theme);
        assert!(anim.open < 0.5, "{}", anim.open);
    }
}
