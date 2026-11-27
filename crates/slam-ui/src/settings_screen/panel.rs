//! The settings panel (ADR-0095): the settings screen in a panel that slides in from the
//! left over whatever is shown, which it dims. [`PanelMotion`] is its slide on the update
//! thread; [`push_backdrop`] draws the dim and the panel's own fill under the screen.

use slam_render::atlas::AtlasRegion;
use slam_render::shape::ShapeBatch;

use super::SettingsLayout;
use crate::anim::{ease_out_cubic, progress};
use crate::menu::Rect;
use crate::screen::Pose;
use crate::theme::{Theme, faded};
use crate::widgets::fill;

/// The panel's width in pixels on a `height`-pixel-high window: the theme's, at most the
/// window's width.
pub fn panel_width(width: f32, height: f32, theme: &Theme) -> f32 {
    (theme.settings_panel_width * theme.scale(height)).min(width)
}

/// The screen's layout inside the panel, at rest (fully in).
pub fn panel_layout(width: f32, height: f32, theme: &Theme) -> SettingsLayout {
    SettingsLayout::new(panel_width(width, height, theme), height, theme)
}

/// How far the panel is in: 0 out of sight, 1 fully in. Eased out, from where it was when
/// it last turned, so a close half-way slides back from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelMotion {
    from: f32,
    to: f32,
    start_ns: u64,
    duration_ns: u64,
}

impl PanelMotion {
    /// A panel that starts sliding in at `now_ns`, for the theme's duration.
    pub fn opening(now_ns: u64, theme: &Theme) -> PanelMotion {
        PanelMotion {
            from: 0.0,
            to: 1.0,
            start_ns: now_ns,
            duration_ns: (f64::from(theme.settings_panel_ms.max(0.0)) * 1e6) as u64,
        }
    }

    /// How far in it is at `now_ns`.
    pub fn openness(&self, now_ns: u64) -> f32 {
        let t = ease_out_cubic(progress(self.start_ns, now_ns, self.duration_ns));
        self.from + (self.to - self.from) * t
    }

    /// Starts sliding out from where it is.
    pub fn close(&mut self, now_ns: u64) {
        self.turn(now_ns, 0.0);
    }

    /// Starts sliding in again from where it is.
    pub fn reopen(&mut self, now_ns: u64) {
        self.turn(now_ns, 1.0);
    }

    fn turn(&mut self, now_ns: u64, to: f32) {
        if self.to == to {
            return;
        }
        let from = self.openness(now_ns);
        // The rest of the way takes its share of the time, at the same pace.
        let share = (to - from).abs();
        *self = PanelMotion {
            from,
            to,
            start_ns: now_ns,
            duration_ns: (self.duration_ns as f64 * f64::from(share)) as u64,
        };
    }

    /// Whether it is coming in or in (not leaving).
    pub fn is_open(&self) -> bool {
        self.to == 1.0
    }

    /// Whether it has left: closing and out of sight at `now_ns`.
    pub fn is_gone(&self, now_ns: u64) -> bool {
        !self.is_open() && self.openness(now_ns) <= 0.0
    }

    /// Pixels the panel is moved to the left at `now_ns`, of a panel `width` wide.
    pub fn shift(&self, now_ns: u64, width: f32) -> f32 {
        -(1.0 - self.openness(now_ns)) * width
    }
}

/// The pose to draw the screen in, a panel `openness` in: moved left by the part still out,
/// as a fraction of the panel's width (which the screen is laid out for).
pub fn panel_pose(openness: f32) -> Pose {
    Pose {
        opacity: 1.0,
        offset: -(1.0 - openness),
        scale: 1.0,
    }
}

/// The shift of the screens under the panel, a panel `openness` in, as a fraction of the
/// window width (the unit of [`Pose::offset`], positive to the right): the panel's width
/// times the theme's ratio, grown with the slide (ADR-0179). None without the effects.
///
/// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/OsuGame.cs (`adjustGlobalScreenOffset`:
/// `SettingsPanel.WIDTH * SIDE_OVERLAY_OFFSET_RATIO` while the settings are visible). The
/// lazer eases the offset with its own damping; here it follows the panel's slide, so both
/// move in step.
pub fn screen_shift(openness: f32, panel_width: f32, window_width: f32, theme: &Theme) -> f32 {
    if !theme.effects || window_width <= 0.0 {
        return 0.0;
    }
    openness.clamp(0.0, 1.0) * panel_width * theme.settings_screen_shift / window_width
}

/// Draws the dim over a `width` × `height` window and the panel's fill, a panel `openness`
/// in. The screen goes on top ([`super::SettingsView::push`] with [`panel_pose`]).
pub fn push_backdrop(
    shapes: &mut ShapeBatch,
    theme: &Theme,
    white: AtlasRegion,
    width: f32,
    height: f32,
    openness: f32,
) {
    let window = Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
    };
    fill(
        shapes,
        white,
        &window,
        faded(theme.settings_dim_fill, openness),
    );
    let panel = panel_width(width, height, theme);
    // Rounded on the right only: the other corners lie past the window's edges.
    let radius = theme.panel_radius * theme.scale(height);
    let rect = Rect {
        x: -(1.0 - openness) * panel - radius,
        y: -radius,
        width: panel + radius,
        height: height + 2.0 * radius,
    };
    crate::widgets::panel(
        shapes,
        white,
        &rect,
        theme,
        theme.scale(height),
        theme.settings_panel_fill,
        1.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn it_slides_in_eased_and_out_from_where_it_is() {
        let theme = Theme::default();
        let mut motion = PanelMotion::opening(1000 * MS, &theme);
        assert_eq!(motion.openness(1000 * MS), 0.0);
        let early = motion.openness(1050 * MS);
        // Eased out: a fifth of the time goes much more than a fifth of the way.
        assert!(early > 0.4 && early < 1.0, "{early}");
        assert_eq!(motion.openness(1250 * MS), 1.0);
        assert!(motion.is_open());
        assert!(!motion.is_gone(1250 * MS));
        // Closed half-way back: from where it is, in its share of the time.
        let mut half = PanelMotion::opening(0, &theme);
        let at = half.openness(50 * MS);
        half.close(50 * MS);
        assert_eq!(half.openness(50 * MS), at);
        assert!(!half.is_open());
        let end = 50 * MS + (250.0 * f64::from(at)) as u64 * MS;
        assert!(half.is_gone(end + MS));
        assert!(!half.is_gone(50 * MS));
        // Opened again before it left.
        half.reopen(60 * MS);
        assert!(half.is_open());
        assert_eq!(half.openness(1000 * MS), 1.0);
        motion.close(2000 * MS);
        assert!(motion.is_gone(2250 * MS));
    }

    #[test]
    fn the_shift_and_pose_follow_the_openness() {
        let theme = Theme::default();
        let motion = PanelMotion::opening(0, &theme);
        assert_eq!(motion.shift(0, 700.0), -700.0);
        assert_eq!(motion.shift(1000 * MS, 700.0), 0.0);
        assert_eq!(panel_pose(1.0).offset, 0.0);
        assert_eq!(panel_pose(0.25).offset, -0.75);
    }

    #[test]
    fn the_screens_under_the_panel_shift_with_its_slide() {
        let theme = Theme::default();
        let (w, h) = (1920.0, 1080.0);
        let panel = panel_width(w, h, &theme);
        let full = panel * theme.settings_screen_shift / w;
        assert_eq!(screen_shift(0.0, panel, w, &theme), 0.0);
        assert!((screen_shift(1.0, panel, w, &theme) - full).abs() < 1e-6);
        assert!((screen_shift(0.5, panel, w, &theme) - full / 2.0).abs() < 1e-6);
        assert!(full > 0.0);
        // Over time: in step with the panel's own motion, back to zero when it has left.
        let mut motion = PanelMotion::opening(0, &theme);
        let ms = 1_000_000;
        let at = |m: &PanelMotion, t: u64| screen_shift(m.openness(t * ms), panel, w, &theme);
        assert_eq!(at(&motion, 0), 0.0);
        let mid = at(&motion, 100);
        assert!(mid > 0.0 && mid < full);
        let end = (theme.settings_panel_ms as u64) + 50;
        assert!((at(&motion, end) - full).abs() < 1e-6);
        motion.close(end * ms);
        assert_eq!(at(&motion, end + 1000), 0.0);
        // Without the effects there is none.
        let plain = Theme {
            effects: false,
            ..theme
        };
        assert_eq!(screen_shift(1.0, panel, w, &plain), 0.0);
    }

    #[test]
    fn the_panel_is_the_themes_width_within_the_window() {
        let theme = Theme::default();
        assert_eq!(panel_width(2000.0, 768.0, &theme), 560.0);
        assert_eq!(panel_width(500.0, 768.0, &theme), 500.0);
        assert_eq!(panel_width(4000.0, 1536.0, &theme), 1120.0);
    }
}
