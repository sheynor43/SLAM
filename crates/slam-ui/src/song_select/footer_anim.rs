//! The footer buttons' feedback (ADR-0175): the hover fade, the press pop, the Mods flash and
//! the sliding in and out of the presets, the pin and the reset button. A state machine over
//! time; the view feeds it the targets each frame and reads the amounts back. Without the
//! effects every amount jumps to its target.

use super::footer::{FooterButton, FooterButtons, MAX_PRESETS};
use crate::theme::Theme;

/// Buttons that have a hover and a press amount: Back, Mods, the presets, the pin, reset.
const SLOTS: usize = 4 + MAX_PRESETS;

fn slot(button: FooterButton) -> usize {
    match button {
        FooterButton::Back => 0,
        FooterButton::Mods => 1,
        FooterButton::Pin => 2,
        FooterButton::Reset => 3,
        FooterButton::Preset(i) => 4 + i.min(MAX_PRESETS - 1),
    }
}

/// How much of each optional button's place is open, 0 (gone) to 1 (fully there).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FooterPresence {
    pub presets: [f32; MAX_PRESETS],
    pub pin: f32,
    pub reset: f32,
}

/// Moves `value` towards `target` by `step` at most.
fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

/// The animation state of the footer.
#[derive(Clone, Copy, Debug, Default)]
pub struct FooterAnim {
    hover: [f32; SLOTS],
    press: [f32; SLOTS],
    presence: FooterPresence,
    last_ns: Option<u64>,
    flash_at: Option<u64>,
}

impl FooterAnim {
    /// Advances to `now_ns` towards the targets: the hovered and pressed buttons, and which
    /// buttons `target` shows.
    pub fn update(
        &mut self,
        now_ns: u64,
        target: &FooterButtons,
        hover: Option<FooterButton>,
        pressed: Option<FooterButton>,
        theme: &Theme,
    ) {
        let dt_ms = match self.last_ns {
            Some(last) => now_ns.saturating_sub(last) as f32 / 1.0e6,
            None => 0.0,
        };
        let first = self.last_ns.is_none();
        self.last_ns = Some(now_ns);
        // Without the effects (or on the first frame) everything stands at its target.
        let snap = !theme.effects || first;
        let step = |ms: f32| {
            if snap || ms <= 0.0 {
                1.0
            } else {
                (dt_ms / ms).min(1.0)
            }
        };
        let hot = hover.map(slot);
        let held = pressed.map(slot);
        for i in 0..SLOTS {
            let up = if hot == Some(i) { 1.0 } else { 0.0 };
            let down = if held == Some(i) { 1.0 } else { 0.0 };
            self.hover[i] = approach(self.hover[i], up, step(theme.footer_hover_ms));
            let ms = if down > self.press[i] {
                theme.footer_press_ms
            } else {
                theme.footer_release_ms
            };
            self.press[i] = approach(self.press[i], down, step(ms));
        }
        let slide = step(theme.footer_slot_ms);
        for (i, p) in self.presence.presets.iter_mut().enumerate() {
            let to = (i < target.preset_count) as u32 as f32;
            *p = approach(*p, to, slide);
        }
        self.presence.pin = approach(self.presence.pin, target.pin.is_some() as u32 as f32, slide);
        self.presence.reset = approach(
            self.presence.reset,
            target.reset.is_some() as u32 as f32,
            slide,
        );
    }

    /// The Mods button's selection changed: it flashes and its text fades in.
    pub fn flash_mods(&mut self, now_ns: u64, theme: &Theme) {
        if theme.effects {
            self.flash_at = Some(now_ns);
        }
    }

    /// How lit `button` is by the pointer, 0 to 1.
    pub fn hover(&self, button: FooterButton) -> f32 {
        self.hover[slot(button)]
    }

    /// How far `button` is pressed in, 0 to 1; it fades back after the release.
    pub fn press(&self, button: FooterButton) -> f32 {
        self.press[slot(button)]
    }

    /// The flash of the Mods button at `now_ns`: 1 at the change, 0 after
    /// [`Theme::footer_flash_ms`].
    pub fn mods_flash(&self, now_ns: u64, theme: &Theme) -> f32 {
        let Some(at) = self.flash_at else { return 0.0 };
        let ms = theme.footer_flash_ms.max(0.0);
        let done = crate::anim::progress(at, now_ns, (ms * 1.0e6) as u64);
        1.0 - crate::anim::ease_out_quint(done)
    }

    pub fn presence(&self) -> &FooterPresence {
        &self.presence
    }
}

#[cfg(test)]
mod tests {
    use super::super::footer::{FooterLayout, FooterState};
    use super::*;

    const MS: u64 = 1_000_000;

    fn buttons(state: FooterState) -> FooterButtons {
        FooterLayout::new(1920.0, 1080.0, &Theme::default()).buttons(state)
    }

    fn state(presets: usize, pin: bool, reset: bool) -> FooterState {
        FooterState {
            presets,
            pin,
            reset,
        }
    }

    /// Runs the animation for `ms` in 10 ms frames.
    fn run(
        a: &mut FooterAnim,
        from: u64,
        ms: u64,
        t: &FooterButtons,
        h: Option<FooterButton>,
        p: Option<FooterButton>,
    ) -> u64 {
        let theme = Theme::default();
        let mut now = from;
        while now < from + ms * MS {
            now += 10 * MS;
            a.update(now, t, h, p, &theme);
        }
        now
    }

    #[test]
    fn hover_fades_in_and_out() {
        let theme = Theme::default();
        let t = buttons(state(0, false, false));
        let mut a = FooterAnim::default();
        a.update(0, &t, None, None, &theme);
        a.update(50 * MS, &t, Some(FooterButton::Back), None, &theme);
        let mid = a.hover(FooterButton::Back);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert_eq!(a.hover(FooterButton::Mods), 0.0);
        let now = run(&mut a, 50 * MS, 300, &t, Some(FooterButton::Back), None);
        assert_eq!(a.hover(FooterButton::Back), 1.0);
        run(&mut a, now, 300, &t, None, None);
        assert_eq!(a.hover(FooterButton::Back), 0.0);
    }

    #[test]
    fn press_pops_quickly_and_fades_after_the_release() {
        let theme = Theme::default();
        let t = buttons(state(0, false, false));
        let mut a = FooterAnim::default();
        a.update(0, &t, None, None, &theme);
        let b = FooterButton::Mods;
        let now = run(&mut a, 0, 100, &t, Some(b), Some(b));
        assert_eq!(a.press(b), 1.0);
        a.update(now + 50 * MS, &t, Some(b), None, &theme);
        let after = a.press(b);
        assert!(after > 0.0 && after < 1.0, "released slowly: {after}");
    }

    #[test]
    fn presets_slide_in_and_out() {
        let theme = Theme::default();
        let mut a = FooterAnim::default();
        let none = buttons(state(0, false, false));
        a.update(0, &none, None, None, &theme);
        let two = buttons(state(2, false, false));
        a.update(80 * MS, &two, None, None, &theme);
        let p = a.presence().presets;
        assert!(p[0] > 0.0 && p[0] < 1.0 && p[2] == 0.0, "{p:?}");
        let now = run(&mut a, 80 * MS, 400, &two, None, None);
        assert_eq!(&a.presence().presets[..3], &[1.0, 1.0, 0.0]);
        // One is removed: the last place closes gradually.
        let one = buttons(state(1, false, false));
        a.update(now + 80 * MS, &one, None, None, &theme);
        let p = a.presence().presets;
        assert!(p[1] > 0.0 && p[1] < 1.0 && p[0] == 1.0, "{p:?}");
        run(&mut a, now + 80 * MS, 400, &one, None, None);
        assert_eq!(a.presence().presets[1], 0.0);
    }

    #[test]
    fn reset_and_pin_open_and_close() {
        let theme = Theme::default();
        let mut a = FooterAnim::default();
        let off = buttons(state(0, false, false));
        a.update(0, &off, None, None, &theme);
        let on = buttons(state(0, true, true));
        a.update(100 * MS, &on, None, None, &theme);
        assert!(a.presence().reset > 0.0 && a.presence().reset < 1.0);
        assert!(a.presence().pin > 0.0 && a.presence().pin < 1.0);
        let now = run(&mut a, 100 * MS, 400, &on, None, None);
        assert_eq!((a.presence().reset, a.presence().pin), (1.0, 1.0));
        a.update(now + 100 * MS, &off, None, None, &theme);
        assert!(a.presence().reset > 0.0 && a.presence().reset < 1.0);
        run(&mut a, now + 100 * MS, 400, &off, None, None);
        assert_eq!(a.presence().reset, 0.0);
    }

    #[test]
    fn the_animated_layout_slides_what_follows_an_opening_place() {
        let layout = FooterLayout::new(1920.0, 1080.0, &Theme::default());
        let open = |p: f32| {
            layout.animated(&FooterPresence {
                presets: [1.0, p, 0.0, 0.0, 0.0, 0.0],
                pin: 1.0,
                reset: 0.0,
            })
        };
        let (shut, half, full) = (open(0.001), open(0.5), open(1.0));
        let pin = |b: &FooterButtons| b.pin.unwrap().x;
        assert!(pin(&shut) < pin(&half) && pin(&half) < pin(&full));
        // Fully open it equals the target layout.
        let target = layout.buttons(state(2, true, false));
        assert!((pin(&full) - target.pin.unwrap().x).abs() < 0.01);
        assert_eq!(full.preset_count, 2);
    }

    #[test]
    fn the_mods_flash_decays_and_effects_off_skip_everything() {
        let theme = Theme::default();
        let mut a = FooterAnim::default();
        assert_eq!(a.mods_flash(0, &theme), 0.0);
        a.flash_mods(0, &theme);
        assert_eq!(a.mods_flash(0, &theme), 1.0);
        let mid = a.mods_flash(100 * MS, &theme);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(a.mods_flash(2000 * MS, &theme), 0.0);
        let calm = Theme::default().with_effects(false);
        let mut b = FooterAnim::default();
        b.flash_mods(0, &calm);
        assert_eq!(b.mods_flash(0, &calm), 0.0);
        let t = buttons(state(2, true, true));
        b.update(0, &t, None, None, &calm);
        b.update(MS, &t, Some(FooterButton::Back), None, &calm);
        assert_eq!(b.hover(FooterButton::Back), 1.0);
        assert_eq!(b.presence().presets[1], 1.0);
        assert_eq!(b.presence().reset, 1.0);
    }
}
