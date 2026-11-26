//! A scrollbar after lazer's `OsuScrollbar` (ADR-0173): a thin, semi-transparent, rounded
//! thumb on the edge of a list that grows and brightens when pointed at or dragged. It can
//! be dragged, and a press on the track outside the thumb jumps there (the thumb centres
//! on the point).
//!
//! Three parts, so any scrolling list can use it:
//! * [`Scrollbar`]: the geometry, a pure function of the track, the content and view
//!   extents and the offset (thumb rectangle, hit test, drag mapping);
//! * [`ScrollbarDrag`]: the update side's drag state;
//! * [`ScrollbarLook`]: the draw side's fades and [`ScrollbarLook::push`].
//!
//! Extents and offsets are in one unit of the caller's choice (pixels, lines); the bar
//! works in fractions. Nothing here allocates.

use slam_render::atlas::AtlasRegion;
use slam_render::shape::{Shape, ShapeBatch};
use slam_render::{Blend, Color};

use crate::anim::{Towards, ease_out_cubic};
use crate::menu::Rect;
use crate::theme::Theme;
use crate::widgets::{Look, draw};

/// What a point on a scrollbar's track is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollbarHit {
    /// Not on the track (or the content does not scroll).
    None,
    Thumb,
    /// On the track outside the thumb.
    Track,
}

/// The geometry of a vertical scrollbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scrollbar {
    /// The strip the bar answers in: its whole length is the way the thumb travels.
    pub track: Rect,
    /// Extent of the content and of the part of it in view.
    pub content: f32,
    pub view: f32,
    /// The shortest the thumb gets, pixels.
    pub min_thumb: f32,
}

impl Scrollbar {
    pub fn new(track: Rect, content: f32, view: f32, min_thumb: f32) -> Scrollbar {
        Scrollbar {
            track,
            content,
            view,
            min_thumb,
        }
    }

    /// The farthest the content scrolls.
    pub fn max_offset(&self) -> f32 {
        (self.content - self.view).max(0.0)
    }

    /// Whether there is anything to scroll to: a bar for content that fits is not shown.
    pub fn is_scrollable(&self) -> bool {
        self.max_offset() > 0.0 && self.track.height > 0.0 && self.content > 0.0
    }

    /// The thumb's length: the share of the content in view, at least `min_thumb` (and at
    /// most the track).
    fn thumb_length(&self) -> f32 {
        if !self.is_scrollable() {
            return self.track.height;
        }
        (self.track.height * (self.view / self.content).clamp(0.0, 1.0))
            .max(self.min_thumb.min(self.track.height))
    }

    /// The thumb at `offset` (clamped): as wide as the track.
    pub fn thumb(&self, offset: f32) -> Rect {
        let length = self.thumb_length();
        let travel = (self.track.height - length).max(0.0);
        let max = self.max_offset();
        let t = if max > 0.0 {
            (offset / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Rect {
            y: self.track.y + travel * t,
            height: length,
            ..self.track
        }
    }

    /// What is at `x`, `y` with the content at `offset`.
    pub fn hit(&self, offset: f32, x: f32, y: f32) -> ScrollbarHit {
        if !self.is_scrollable() || !self.track.contains(x, y) {
            return ScrollbarHit::None;
        }
        if self.thumb(offset).contains(x, y) {
            ScrollbarHit::Thumb
        } else {
            ScrollbarHit::Track
        }
    }

    /// The offset that puts the thumb's top at `top` (clamped to the track).
    fn offset_for_top(&self, top: f32) -> f32 {
        let length = self.thumb_length();
        let travel = self.track.height - length;
        if travel <= 0.0 {
            return 0.0;
        }
        ((top - self.track.y) / travel).clamp(0.0, 1.0) * self.max_offset()
    }

    /// The offset a press on the track at `y` jumps to: the thumb centres on `y`.
    pub fn offset_for_click(&self, y: f32) -> f32 {
        self.offset_for_top(y - self.thumb_length() / 2.0)
    }

    /// The offset of a thumb held `grab` pixels below its top, the pointer at `y`.
    pub fn offset_for_drag(&self, grab: f32, y: f32) -> f32 {
        self.offset_for_top(y - grab)
    }
}

/// The update side's state of a scrollbar: whether (and where on the thumb) it is held.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollbarDrag {
    /// Pixels between the thumb's top and the pointer when the press went down.
    grab: Option<f32>,
}

impl ScrollbarDrag {
    pub const fn new() -> ScrollbarDrag {
        ScrollbarDrag { grab: None }
    }

    pub fn is_dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// A press at `x`, `y` with the content at `offset`. On the thumb it takes hold; on the
    /// track it jumps (and holds the thumb by its middle, so the drag goes on from there).
    /// Returns the new offset when the press is the bar's.
    pub fn press(&mut self, bar: &Scrollbar, offset: f32, x: f32, y: f32) -> Option<f32> {
        match bar.hit(offset, x, y) {
            ScrollbarHit::None => None,
            ScrollbarHit::Thumb => {
                self.grab = Some(y - bar.thumb(offset).y);
                Some(offset)
            }
            ScrollbarHit::Track => {
                self.grab = Some(bar.thumb(offset).height / 2.0);
                Some(bar.offset_for_click(y))
            }
        }
    }

    /// The pointer at `y`: the new offset while held.
    pub fn drag(&mut self, bar: &Scrollbar, y: f32) -> Option<f32> {
        self.grab.map(|grab| bar.offset_for_drag(grab, y))
    }

    pub fn release(&mut self) {
        self.grab = None;
    }
}

/// The draw side of a scrollbar: its opacity and width ease between rest, pointed at and
/// held (ADR-0173).
#[derive(Clone, Copy, Debug)]
pub struct ScrollbarLook {
    alpha: Towards,
    width: Towards,
    ready: bool,
    /// Which edge of its track the thumb hugs (see [`ScrollbarLook::on_left`]).
    left: bool,
}

impl Default for ScrollbarLook {
    fn default() -> ScrollbarLook {
        ScrollbarLook::new()
    }
}

impl ScrollbarLook {
    pub fn new() -> ScrollbarLook {
        ScrollbarLook {
            alpha: Towards::default(),
            width: Towards::default(),
            ready: false,
            left: false,
        }
    }

    /// A look whose thumb hugs the track's left edge, for a list with its bar on the left.
    pub fn on_left() -> ScrollbarLook {
        ScrollbarLook {
            left: true,
            ..ScrollbarLook::new()
        }
    }

    /// The thumb's opacity and width (pixels) at `now_ns` for the given state.
    pub fn pose(
        &mut self,
        hovered: bool,
        dragging: bool,
        now_ns: u64,
        theme: &Theme,
        scale: f32,
    ) -> (f32, f32) {
        self.ease(
            hovered,
            dragging,
            now_ns,
            theme,
            scale,
            [
                (theme.scrollbar_alpha, theme.scrollbar_width),
                (theme.scrollbar_hover_alpha, theme.scrollbar_active_width),
                (theme.scrollbar_drag_alpha, theme.scrollbar_active_width),
            ],
        )
    }

    /// Eases to the (level, width in reference pixels) of the state: `[rest, hovered,
    /// held]`.
    fn ease(
        &mut self,
        hovered: bool,
        dragging: bool,
        now_ns: u64,
        theme: &Theme,
        scale: f32,
        states: [(f32, f32); 3],
    ) -> (f32, f32) {
        let fade = (theme.scrollbar_fade_ms.max(0.0) * 1.0e6) as u64;
        let (alpha, width) = if dragging {
            states[2]
        } else if hovered {
            states[1]
        } else {
            states[0]
        };
        if !self.ready {
            // The first frame: start at rest rather than fading in from nothing.
            self.ready = true;
            self.alpha = Towards::rest(alpha, now_ns);
            self.width = Towards::rest(width * scale, now_ns);
        }
        self.alpha.aim(alpha, now_ns, fade, ease_out_cubic);
        self.width.aim(width * scale, now_ns, fade, ease_out_cubic);
        (
            self.alpha.at(now_ns, fade, ease_out_cubic),
            self.width.at(now_ns, fade, ease_out_cubic),
        )
    }

    /// Draws the bar at `offset`: a white rounded thumb on the track's edge (right, or left for [`ScrollbarLook::on_left`]), the
    /// layer faded by `opacity`. Nothing for content that does not scroll.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        shapes: &mut ShapeBatch,
        white: AtlasRegion,
        bar: &Scrollbar,
        offset: f32,
        pointer: [f32; 2],
        dragging: bool,
        opacity: f32,
        theme: &Theme,
        scale: f32,
        now_ns: u64,
    ) {
        if !bar.is_scrollable() {
            return;
        }
        let hovered = bar.track.contains(pointer[0], pointer[1]);
        let (alpha, width) = self.pose(hovered, dragging, now_ns, theme, scale);
        let hit = bar.thumb(offset);
        let margin = theme.scrollbar_margin * scale;
        let x = if self.left {
            hit.x + margin
        } else {
            hit.x + hit.width - margin - width
        };
        let rect = Rect { x, width, ..hit };
        let colour = Color {
            a: alpha * opacity,
            ..Color::WHITE
        };
        draw(shapes, white, &rect, colour, Look::Pill, theme, scale);
    }
}

impl ScrollbarLook {
    /// Draws the bar as lazer's carousel does (`Carousel.ScrollBar`, ADR-0174): the thumb
    /// is a capsule centred in the track, half as wide as it at rest and as wide as it
    /// when pointed at or held, added to what is under it; grey at rest, lighter pointed
    /// at, the accent held. Nothing for content that does not scroll.
    #[allow(clippy::too_many_arguments)]
    pub fn push_carousel(
        &mut self,
        shapes: &mut ShapeBatch,
        white: AtlasRegion,
        bar: &Scrollbar,
        offset: f32,
        pointer: [f32; 2],
        dragging: bool,
        opacity: f32,
        theme: &Theme,
        scale: f32,
        now_ns: u64,
    ) {
        if !bar.is_scrollable() {
            return;
        }
        let hovered = bar.track.contains(pointer[0], pointer[1]);
        let full = theme.song_scrollbar_width;
        // The "alpha" eased here is the colour's level: 0 rest, 1 pointed at, 2 held.
        let (level, width) = self.ease(
            hovered,
            dragging,
            now_ns,
            theme,
            scale,
            [
                (0.0, full * theme.song_scrollbar_rest_width),
                (1.0, full),
                (2.0, full),
            ],
        );
        let thumb = bar.thumb(offset);
        let rect = Rect {
            x: thumb.x + (thumb.width - width) / 2.0,
            width,
            ..thumb
        };
        draw_carousel(shapes, white, &rect, level, opacity, theme);
    }
}

/// The carousel thumb's colour at `level` (0 rest, 1 pointed at, 2 held): grey, lighter,
/// the accent.
pub(crate) fn carousel_colour(level: f32, theme: &Theme) -> Color {
    let grey = |v: f32| Color::rgb(v, v, v);
    let mix = |a: Color, b: Color, t: f32| Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    };
    let (rest, hover) = (
        grey(theme.song_scrollbar_rest),
        grey(theme.song_scrollbar_hover),
    );
    if level <= 1.0 {
        mix(rest, hover, level.clamp(0.0, 1.0))
    } else {
        mix(hover, theme.accent, (level - 1.0).clamp(0.0, 1.0))
    }
}

fn draw_carousel(
    shapes: &mut ShapeBatch,
    white: AtlasRegion,
    rect: &Rect,
    level: f32,
    opacity: f32,
    theme: &Theme,
) {
    let colour = Color {
        a: opacity,
        ..carousel_colour(level, theme)
    };
    shapes.push_shape(&Shape {
        radius: rect.width.min(rect.height) / 2.0,
        blend: Blend::Additive,
        ..Shape::new(
            white.texture,
            white.uv,
            [rect.x, rect.y],
            [rect.width, rect.height],
            colour,
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_carousel_thumb_is_grey_lighter_pointed_at_and_the_accent_held() {
        let theme = Theme::default();
        let rest = carousel_colour(0.0, &theme);
        let hover = carousel_colour(1.0, &theme);
        let held = carousel_colour(2.0, &theme);
        assert_eq!(rest.r, theme.song_scrollbar_rest);
        assert_eq!(hover.r, theme.song_scrollbar_hover);
        assert!(rest.r < hover.r);
        assert!((held.g - theme.accent.g).abs() < 1e-5 && (held.b - theme.accent.b).abs() < 1e-5);
    }

    #[test]
    fn the_carousel_thumb_is_half_as_wide_at_rest_and_added() {
        let theme = Theme::default();
        let white = AtlasRegion {
            texture: slam_render::textures::TextureId(0),
            origin: slam_render::Origin::new(0, 0),
            size: slam_render::Extent::new(1, 1),
            uv: slam_render::atlas::UvRect::FULL,
        };
        let track = Rect {
            x: 100.0,
            y: 10.0,
            width: theme.song_scrollbar_width,
            height: 200.0,
        };
        let bar = Scrollbar::new(track, 800.0, 200.0, 10.0);
        let mut look = ScrollbarLook::new();
        let mut shapes = ShapeBatch::with_capacity(4);
        let ms = 1_000_000;
        look.push_carousel(
            &mut shapes,
            white,
            &bar,
            0.0,
            [0.0, 0.0],
            false,
            1.0,
            &theme,
            1.0,
            0,
        );
        assert_eq!(shapes.len(), 1);
        let rest = shapes.vertices().to_vec();
        // The quad reaches a margin past the shape, so widths are compared to each other.
        let rest_width = rest[1].position[0] - rest[0].position[0];
        // Pointed at: the full width, after the fade.
        shapes.clear();
        look.push_carousel(
            &mut shapes,
            white,
            &bar,
            0.0,
            [104.0, 50.0],
            false,
            1.0,
            &theme,
            1.0,
            5000 * ms,
        );
        shapes.clear();
        look.push_carousel(
            &mut shapes,
            white,
            &bar,
            0.0,
            [104.0, 50.0],
            false,
            1.0,
            &theme,
            1.0,
            6000 * ms,
        );
        let hovered = shapes.vertices();
        let width = hovered[1].position[0] - hovered[0].position[0];
        let grown = width - rest_width;
        let expected = theme.song_scrollbar_width * (1.0 - theme.song_scrollbar_rest_width);
        assert!((grown - expected).abs() < 1e-3, "{grown}");
        // A list that fits has none.
        shapes.clear();
        let short = Scrollbar::new(track, 100.0, 200.0, 10.0);
        look.push_carousel(
            &mut shapes,
            white,
            &short,
            0.0,
            [0.0, 0.0],
            false,
            1.0,
            &theme,
            1.0,
            0,
        );
        assert_eq!(shapes.len(), 0);
    }

    fn bar() -> Scrollbar {
        // 100 px track; content 400, view 100: thumb of 25 px.
        Scrollbar::new(
            Rect {
                x: 90.0,
                y: 10.0,
                width: 10.0,
                height: 100.0,
            },
            400.0,
            100.0,
            8.0,
        )
    }

    #[test]
    fn the_thumb_is_the_view_share_and_travels_with_the_offset() {
        let bar = bar();
        let top = bar.thumb(0.0);
        assert_eq!((top.y, top.height), (10.0, 25.0));
        let end = bar.thumb(300.0);
        assert_eq!(end.y + end.height, 110.0);
        // Out-of-range offsets are held.
        assert_eq!(bar.thumb(-5.0), top);
        assert_eq!(bar.thumb(9999.0), end);
        assert_eq!(bar.thumb(150.0).y, 10.0 + 75.0 * 0.5);
    }

    #[test]
    fn a_short_thumb_is_held_to_the_minimum() {
        let bar = Scrollbar::new(bar().track, 100_000.0, 100.0, 8.0);
        assert_eq!(bar.thumb(0.0).height, 8.0);
    }

    #[test]
    fn content_that_fits_has_no_bar() {
        let bar = Scrollbar::new(bar().track, 80.0, 100.0, 8.0);
        assert!(!bar.is_scrollable());
        assert_eq!(bar.hit(0.0, 95.0, 50.0), ScrollbarHit::None);
        assert_eq!(bar.max_offset(), 0.0);
    }

    #[test]
    fn hits_tell_the_thumb_from_the_track_and_the_rest() {
        let bar = bar();
        assert_eq!(bar.hit(0.0, 95.0, 20.0), ScrollbarHit::Thumb);
        assert_eq!(bar.hit(0.0, 95.0, 80.0), ScrollbarHit::Track);
        assert_eq!(bar.hit(0.0, 50.0, 20.0), ScrollbarHit::None);
        assert_eq!(bar.hit(0.0, 95.0, 5.0), ScrollbarHit::None);
    }

    #[test]
    fn a_click_on_the_track_centres_the_thumb_there() {
        let bar = bar();
        let offset = bar.offset_for_click(60.0);
        let thumb = bar.thumb(offset);
        assert!((thumb.y + thumb.height / 2.0 - 60.0).abs() < 1e-3);
        // Near the ends it is held to them.
        assert_eq!(bar.offset_for_click(0.0), 0.0);
        assert_eq!(bar.offset_for_click(1000.0), 300.0);
    }

    #[test]
    fn a_held_thumb_keeps_its_grab_point_under_the_pointer() {
        let bar = bar();
        let mut drag = ScrollbarDrag::new();
        // Press 5 px into the thumb at offset 0.
        assert_eq!(drag.press(&bar, 0.0, 95.0, 15.0), Some(0.0));
        assert!(drag.is_dragging());
        let offset = drag.drag(&bar, 45.0).unwrap();
        let thumb = bar.thumb(offset);
        assert!((thumb.y + 5.0 - 45.0).abs() < 1e-3);
        drag.release();
        assert!(!drag.is_dragging());
        assert_eq!(drag.drag(&bar, 45.0), None);
    }

    #[test]
    fn a_press_on_the_track_jumps_and_holds_the_middle() {
        let bar = bar();
        let mut drag = ScrollbarDrag::new();
        let offset = drag.press(&bar, 0.0, 95.0, 90.0).unwrap();
        assert_eq!(offset, bar.offset_for_click(90.0));
        // Dragging on keeps the thumb centred on the pointer.
        let moved = drag.drag(&bar, 60.0).unwrap();
        let thumb = bar.thumb(moved);
        assert!((thumb.y + thumb.height / 2.0 - 60.0).abs() < 1e-3);
        // A press outside is not the bar's.
        assert_eq!(ScrollbarDrag::new().press(&bar, 0.0, 10.0, 90.0), None);
    }

    #[test]
    fn the_look_brightens_and_widens_on_hover_and_drag() {
        let theme = Theme::default();
        let mut look = ScrollbarLook::new();
        let ms = 1_000_000;
        let (rest_a, rest_w) = look.pose(false, false, 0, &theme, 1.0);
        assert_eq!(rest_a, theme.scrollbar_alpha);
        assert_eq!(rest_w, theme.scrollbar_width);
        // Starts to change at once, ends at the hover look after the fade.
        let (a, _) = look.pose(true, false, 10 * ms, &theme, 1.0);
        assert!(a >= rest_a);
        let (hover_a, hover_w) = look.pose(true, false, 1000 * ms, &theme, 1.0);
        assert!((hover_a - theme.scrollbar_hover_alpha).abs() < 1e-4);
        assert!((hover_w - theme.scrollbar_active_width).abs() < 1e-4);
        look.pose(true, true, 2000 * ms, &theme, 1.0);
        let (drag_a, _) = look.pose(true, true, 3000 * ms, &theme, 1.0);
        assert!((drag_a - theme.scrollbar_drag_alpha).abs() < 1e-4);
        assert!(rest_a < hover_a && hover_a < drag_a);
    }
}

#[cfg(test)]
mod left_tests {
    use super::*;
    use slam_render::atlas::AtlasRegion;

    #[test]
    fn a_left_look_hugs_the_left_edge_of_the_track() {
        let theme = Theme::default();
        let track = Rect {
            x: 100.0,
            y: 0.0,
            width: 11.0,
            height: 100.0,
        };
        let bar = Scrollbar::new(track, 400.0, 100.0, 8.0);
        let white = AtlasRegion {
            texture: slam_render::textures::TextureId(0),
            origin: slam_render::Origin::new(0, 0),
            size: slam_render::Extent::new(1, 1),
            uv: slam_render::atlas::UvRect::FULL,
        };
        let mut shapes = ShapeBatch::new();
        ScrollbarLook::on_left().push(
            &mut shapes,
            white,
            &bar,
            0.0,
            [0.0, 0.0],
            false,
            1.0,
            &theme,
            1.0,
            0,
        );
        let v = shapes.vertices();
        let left = v.iter().map(|p| p.position[0]).fold(f32::MAX, f32::min);
        assert!(
            left >= 100.0 && left < 100.0 + theme.scrollbar_margin + 1.0,
            "{left}"
        );
    }
}
