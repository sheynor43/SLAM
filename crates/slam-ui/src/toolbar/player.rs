//! The toolbar's music player (ADR-0178): a button on the right of the bar opens a popup
//! after lazer's now-playing overlay: the playing beatmap's background dimmed behind its
//! title and artist, a progress bar to click or drag, and the previous, play/pause and next
//! buttons.
//!
//! The update side ([`Toolbar`]'s player state) turns presses into [`PlayerAction`]s that
//! the application carries out on its music; the draw side ([`PlayerView`]) draws the popup
//! from a [`NowPlayingFrame`] and the texts of a [`NowPlayingText`]. Neither allocates once
//! warm; the texts are laid out again (allocating) when the playing track changes.

mod anim;

use std::sync::Arc;

use slam_render::Color;
use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::Shape;
use slam_render::shape::ShapeBatch;
use slam_render::sprite::{Affine, SpriteBatch};
use slam_text::GlyphCache;

use self::anim::{PlayerAnim, Target as AnimTarget};
use super::{Target, Toolbar, ToolbarEvent, ToolbarLayout};
use crate::anim::{ease_out_cubic, ease_out_quint};
use crate::marquee::{Marquee, MarqueeParams, push_marquee};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::settings_screen::icons;
use crate::theme::{Theme, faded};
use crate::widgets::{Look, draw, push_left, push_shadow};

/// What a press in the player's popup asks the application to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayerAction {
    /// Back one track, or to the start of this one.
    Previous,
    PlayPause,
    Next,
    /// Seek to this share of the track, 0 ..= 1: a press on the bar or a drag along it.
    Seek(f32),
}

/// What plays, as the draw side shows it; `Copy`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NowPlayingFrame {
    /// Whether there is a track to control.
    pub available: bool,
    /// Whether it is paused.
    pub paused: bool,
    /// Where the track is, milliseconds; 0 when unknown.
    pub position_ms: f32,
    /// How long the track is, milliseconds; 0 when unknown.
    pub length_ms: f32,
    /// How much the popup's glow flares on the music's beat, 0 to 1; `None` when the
    /// beat of what plays is not known (the glow then breathes with the clock).
    pub beat: Option<f32>,
    /// The star rating of what plays: the outline and the glow take its colour, easing
    /// to it when it changes (ADR-0191); the accent when unknown.
    pub stars: Option<f32>,
}

impl NowPlayingFrame {
    /// How far into the track, 0 ..= 1.
    pub fn progress(&self) -> f32 {
        if self.length_ms > 0.0 {
            (self.position_ms / self.length_ms).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// The texts of the playing track; cloning it shares the strings.
#[derive(Clone, Debug, Default)]
pub struct NowPlayingText {
    /// Changes with the track: the draw side lays the texts out again when it does.
    pub key: u64,
    pub title: Arc<str>,
    pub artist: Arc<str>,
}

/// The open popup as the draw side sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerFrame {
    pub since_ns: u64,
    /// The button under the pointer: 0 previous, 1 play/pause, 2 next.
    pub hover: Option<u8>,
    /// The button held down, same numbering.
    pub pressed: Option<u8>,
    /// The pointer is over the progress bar.
    pub bar_hover: bool,
    /// The bar is being dragged.
    pub seeking: bool,
}

/// The open popup on the update thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlayerOpen {
    since_ns: u64,
    hover: Option<u8>,
    pressed: Option<u8>,
    bar_hover: bool,
    seeking: bool,
}

/// Where the popup's parts are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerRects {
    pub popup: Rect,
    /// Title and artist texts.
    pub title: Rect,
    pub artist: Rect,
    /// The progress bar's track.
    pub bar: Rect,
    /// Previous, play/pause, next.
    pub buttons: [Rect; 3],
}

impl ToolbarLayout {
    /// The popup of the player button: under it, within the window, `dy` down.
    pub fn player_rects(&self, dy: f32) -> PlayerRects {
        let theme = &self.theme;
        let scale = self.scale;
        let width = theme.player_width * scale;
        let height = theme.player_height * scale;
        let pad = theme.player_pad * scale;
        let gap = theme.toolbar_gap * scale * 2.0;
        let popup = Rect {
            x: (self.player.x + self.player.width - width)
                .clamp(0.0, (self.window[0] - width).max(0.0)),
            y: self.bar.height + gap + dy,
            width,
            height,
        };
        // The plate leans: texts and bar keep clear of the edge at its leaning corners.
        let lean = popup.height * theme.button_skew().tan().abs() / 2.0;
        let inner = popup.width - 2.0 * (pad + lean);
        let title_h = theme.player_title_size * scale * 1.3;
        let artist_h = theme.player_artist_size * scale * 1.3;
        let title = Rect {
            x: popup.x + pad + lean,
            y: popup.y + pad,
            width: inner,
            height: title_h,
        };
        let artist = Rect {
            y: title.y + title_h,
            height: artist_h,
            ..title
        };
        let side = theme.player_button * scale;
        let buttons_y = popup.y + popup.height - pad - side;
        let bar_h = theme.player_bar_height * scale;
        let bar = Rect {
            x: popup.x + pad + lean,
            y: (artist.y + artist_h + buttons_y) / 2.0 - bar_h / 2.0,
            width: inner,
            height: bar_h,
        };
        let centre = popup.x + popup.width / 2.0;
        let step = side + pad;
        let button = |i: f32| Rect {
            x: centre - side / 2.0 + i * step,
            y: buttons_y,
            width: side,
            height: side,
        };
        PlayerRects {
            popup,
            title,
            artist,
            bar,
            buttons: [button(-1.0), button(0.0), button(1.0)],
        }
    }

    /// The share of the track (0 ..= 1) under pointer position `x` on the progress bar.
    pub fn seek_fraction(&self, x: f32) -> f32 {
        let bar = self.player_rects(0.0).bar;
        if bar.width <= 0.0 {
            return 0.0;
        }
        ((x - bar.x) / bar.width).clamp(0.0, 1.0)
    }
}

impl PlayerRects {
    /// The bar's area for the pointer: taller than the track, which is thin.
    fn bar_hit(&self, slack: f32) -> Rect {
        Rect {
            x: self.bar.x,
            y: self.bar.y - slack,
            width: self.bar.width,
            height: self.bar.height + 2.0 * slack,
        }
    }

    /// The button under the point; the buttons lean by `skew` radians like the plate.
    fn button_at(&self, x: f32, y: f32, skew: f32) -> Option<usize> {
        self.buttons
            .iter()
            .position(|b| b.contains_skewed(x, y, skew))
    }
}

impl Toolbar {
    /// Whether the player's popup is open.
    pub fn player_open(&self) -> bool {
        self.player.is_some()
    }

    /// Opens the player's popup; the pin popup closes.
    pub fn show_player(&mut self, now_ns: u64) {
        self.popup = None;
        self.player = Some(PlayerOpen {
            since_ns: now_ns,
            hover: None,
            pressed: None,
            bar_hover: false,
            seeking: false,
        });
    }

    /// Closes the player's popup, if open.
    pub fn close_player(&mut self) {
        self.player = None;
    }

    /// The popup as the draw side shows it.
    pub(super) fn player_frame(&self) -> Option<PlayerFrame> {
        self.player.map(|p| PlayerFrame {
            since_ns: p.since_ns,
            hover: p.hover,
            pressed: p.pressed,
            bar_hover: p.bar_hover,
            seeking: p.seeking,
        })
    }

    /// The pointer moved: the buttons' hover, and a drag along the bar.
    pub(super) fn player_pointer(
        &mut self,
        x: f32,
        y: f32,
        layout: &ToolbarLayout,
    ) -> Option<ToolbarEvent> {
        let player = self.player.as_mut()?;
        let rects = layout.player_rects(0.0);
        let skew = layout.theme.button_skew();
        player.hover = rects.button_at(x, y, skew).map(|i| i as u8);
        player.bar_hover = rects.bar_hit(layout.scale * 6.0).contains(x, y);
        player
            .seeking
            .then(|| ToolbarEvent::Player(PlayerAction::Seek(layout.seek_fraction(x))))
    }

    /// A press while the popup is open: inside it is the popup's, outside closes it.
    pub(super) fn player_press(
        &mut self,
        x: f32,
        y: f32,
        layout: &ToolbarLayout,
    ) -> Option<ToolbarEvent> {
        let rects = layout.player_rects(0.0);
        let skew = layout.theme.button_skew();
        if !rects.popup.contains_skewed(x, y, skew) {
            return None;
        }
        let player = self.player.as_mut()?;
        let slack = layout.scale * 6.0;
        if rects.bar_hit(slack).contains(x, y) {
            player.seeking = true;
            return Some(ToolbarEvent::Player(PlayerAction::Seek(
                layout.seek_fraction(x),
            )));
        }
        let hit = rects.button_at(x, y, skew);
        player.pressed = hit.map(|i| i as u8);
        Some(match hit {
            Some(0) => ToolbarEvent::Player(PlayerAction::Previous),
            Some(1) => ToolbarEvent::Player(PlayerAction::PlayPause),
            Some(2) => ToolbarEvent::Player(PlayerAction::Next),
            _ => ToolbarEvent::Absorbed,
        })
    }

    /// The primary button released: ends a drag along the bar.
    pub(super) fn player_release(&mut self) {
        if let Some(player) = &mut self.player {
            player.seeking = false;
            player.pressed = None;
        }
    }

    /// What a press does when the player's popup is open and the press is outside it.
    pub(super) fn player_outside(&mut self, target: Option<Target>) -> ToolbarEvent {
        self.player = None;
        match target {
            // The button of the open popup closes it.
            Some(Target::Player) | None => ToolbarEvent::ClosePlayer,
            Some(Target::Pin(pin)) => ToolbarEvent::OpenPopup { pin },
            Some(Target::Settings) => ToolbarEvent::OpenSettings,
            Some(Target::Home) => ToolbarEvent::Home,
        }
    }
}

/// The draw side of the popup: its texts and their marquees, and the levels of its motion.
pub(super) struct PlayerView {
    title: Line,
    artist: Line,
    /// The texts of the previous track, fading out.
    old_title: Line,
    old_artist: Line,
    has_old: bool,
    nothing: Line,
    seen: Option<(u64, Arc<str>, Arc<str>)>,
    title_marquee: Marquee,
    artist_marquee: Marquee,
    old_title_marquee: Marquee,
    old_artist_marquee: Marquee,
    cover: Option<AtlasRegion>,
    old_cover: Option<AtlasRegion>,
    anim: PlayerAnim,
    /// The last open popup: what the closing animation goes on drawing.
    last: Option<(PlayerFrame, NowPlayingFrame)>,
    scale: f32,
    /// The outline's colour as shown, easing to the playing difficulty's, and when it was
    /// last stepped.
    tint: Option<Color>,
    tint_ns: Option<u64>,
}

impl PlayerView {
    pub(super) fn new(nothing: &str) -> PlayerView {
        PlayerView {
            title: Line::new(""),
            artist: Line::new(""),
            old_title: Line::new(""),
            old_artist: Line::new(""),
            has_old: false,
            nothing: Line::new(nothing),
            seen: None,
            title_marquee: Marquee::default(),
            artist_marquee: Marquee::default(),
            old_title_marquee: Marquee::default(),
            old_artist_marquee: Marquee::default(),
            cover: None,
            old_cover: None,
            anim: PlayerAnim::default(),
            last: None,
            scale: 0.0,
            tint: None,
            tint_ns: None,
        }
    }

    pub(super) fn lay_out(&mut self, type_setter: &mut Typesetter, theme: &Theme, scale: f32) {
        self.scale = scale;
        self.seen = None;
        self.has_old = false;
        type_setter.lay_out_face(
            &mut self.nothing,
            theme.regular_face(theme.player_artist_size * scale),
        );
    }

    fn follow(
        &mut self,
        type_setter: &mut Typesetter,
        theme: &Theme,
        text: &NowPlayingText,
        now_ns: u64,
        animate: bool,
    ) {
        let same = self.seen.as_ref().is_some_and(|(key, title, artist)| {
            *key == text.key && Arc::ptr_eq(title, &text.title) && Arc::ptr_eq(artist, &text.artist)
        });
        if same {
            return;
        }
        // The texts shown go behind, to fade out; the first ones have nothing to replace,
        // and a popup that opens shows what plays now, not the change it missed closed.
        if self.seen.is_some() && animate {
            std::mem::swap(&mut self.title, &mut self.old_title);
            std::mem::swap(&mut self.artist, &mut self.old_artist);
            std::mem::swap(&mut self.title_marquee, &mut self.old_title_marquee);
            std::mem::swap(&mut self.artist_marquee, &mut self.old_artist_marquee);
            self.has_old = true;
            self.anim.text_changed();
        } else {
            self.has_old = false;
        }
        self.seen = Some((text.key, Arc::clone(&text.title), Arc::clone(&text.artist)));
        self.title.text.clear();
        self.title.text.push_str(&text.title);
        self.artist.text.clear();
        self.artist.text.push_str(&text.artist);
        let scale = self.scale;
        type_setter.lay_out_face(
            &mut self.title,
            theme.heading_face(theme.player_title_size * scale),
        );
        type_setter.lay_out_face(
            &mut self.artist,
            theme.regular_face(theme.player_artist_size * scale),
        );
        self.title_marquee.restart(now_ns);
        self.artist_marquee.restart(now_ns);
    }

    /// Notes the cover shown: another picture fades in over the previous one, a first one
    /// fades in and a cover that goes fades out. A popup that opens (`animate` false) shows
    /// the cover of now at once.
    fn follow_cover(&mut self, cover: Option<AtlasRegion>, animate: bool) {
        if cover == self.cover {
            return;
        }
        self.old_cover = self.cover.filter(|_| animate);
        self.cover = cover;
        if animate {
            self.anim.cover_changed();
        }
    }

    /// How much of a cover the plate shows, 0 to 1, at cross-fade `mix`: what the dim over
    /// it follows, so that it comes and goes with the picture.
    fn cover_presence(&self, mix: f32) -> f32 {
        match (self.old_cover.is_some() && mix < 1.0, self.cover.is_some()) {
            (true, true) => 1.0,
            (false, true) => mix,
            (true, false) => 1.0 - mix,
            (false, false) => 0.0,
        }
    }

    /// Draws the popup of `popup` under the player button, or goes on drawing the closing
    /// one when it is `None`. `cover` is the playing beatmap's background, if one is
    /// ready. Does not allocate once the texts are laid out.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push(
        &mut self,
        type_setter: &mut Typesetter,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        layout: &ToolbarLayout,
        popup: Option<&PlayerFrame>,
        now: &NowPlayingFrame,
        words: &NowPlayingText,
        cover: Option<AtlasRegion>,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let theme = layout.theme;
        let scale = layout.scale;
        // Fully closed until now: the popup opens on what plays, without the cross-fades
        // of changes made while it was shut.
        let animate = self.last.is_some();
        if let Some(popup) = popup {
            self.last = Some((*popup, *now));
        }
        let Some((shown, now)) = self.last else {
            return;
        };
        let target = AnimTarget {
            open: popup.is_some(),
            hover: popup.and(shown.hover).filter(|_| now.available),
            pressed: popup.and(shown.pressed).filter(|_| now.available),
            bar_hot: popup.is_some() && now.available && (shown.bar_hover || shown.seeking),
            paused: now.paused,
            progress: now.progress(),
        };
        self.anim.step(now_ns, &target, &theme);
        if !self.anim.visible(&target) {
            self.last = None;
            return;
        }
        let first = (shapes.len(), text.len());
        let rects = layout.player_rects(0.0);
        if now.available {
            self.follow(type_setter, &theme, words, now_ns, animate);
        }
        self.follow_cover(cover, animate);
        let plate = Shape {
            radius: theme.panel_radius * scale,
            skew: theme.button_skew(),
            ..Shape::new(
                white.texture,
                white.uv,
                [rects.popup.x, rects.popup.y],
                [rects.popup.width, rects.popup.height],
                theme.widget_popup_fill,
            )
        };
        push_shadow(shapes, &plate, &theme, scale, 1.0);
        let tint = self.step_tint(&now, &theme, now_ns);
        self.push_glow(shapes, &plate, &theme, scale, &now, tint, now_ns);
        shapes.push_shape(&plate);
        // The cover, cut by the plate's rounded and leaning edge, the previous one under
        // the one fading in (or fading out alone); then the dim, as strong as the cover.
        let mix = ease_out_cubic(self.anim.cover);
        if let Some(old) = self.old_cover.filter(|_| mix < 1.0) {
            let under = if self.cover.is_some() { 1.0 } else { 1.0 - mix };
            push_cover(shapes, old, &plate, under);
        }
        if let Some(region) = self.cover {
            push_cover(shapes, region, &plate, mix);
        }
        let presence = self.cover_presence(mix);
        if presence > 0.0 {
            shapes.push_shape(&Shape {
                colour: Color {
                    a: theme.player_cover_dim * presence,
                    ..Color::rgb(0.0, 0.0, 0.0)
                },
                ..plate
            });
        }
        let params = MarqueeParams::new(&theme, scale);
        let swap = ease_out_cubic(self.anim.text);
        if now.available {
            if self.has_old && swap < 1.0 {
                push_marquee(
                    text,
                    &self.old_title,
                    &rects.title,
                    0.0,
                    faded(theme.text, 1.0 - swap),
                    glyphs,
                    &mut self.old_title_marquee,
                    &params,
                    now_ns,
                );
                push_marquee(
                    text,
                    &self.old_artist,
                    &rects.artist,
                    0.0,
                    faded(theme.text, 0.75 * (1.0 - swap)),
                    glyphs,
                    &mut self.old_artist_marquee,
                    &params,
                    now_ns,
                );
            }
            push_marquee(
                text,
                &self.title,
                &rects.title,
                0.0,
                faded(theme.text, swap),
                glyphs,
                &mut self.title_marquee,
                &params,
                now_ns,
            );
            push_marquee(
                text,
                &self.artist,
                &rects.artist,
                0.0,
                faded(theme.text, 0.75 * swap),
                glyphs,
                &mut self.artist_marquee,
                &params,
                now_ns,
            );
        } else {
            push_left(
                text,
                &self.nothing,
                &rects.title,
                0.0,
                faded(theme.text, 0.75),
                glyphs,
            );
        }
        // The outline in the difficulty's colour, over the cover's edge.
        shapes.push_shape(&Shape {
            outline: theme.player_outline_width * scale,
            colour: faded(tint, 0.9),
            ..plate
        });
        self.push_bar(shapes, &rects, &now, &theme, scale, white);
        self.push_buttons(shapes, text, glyphs, &rects, &now, &theme, scale, white);
        // Fade, slide and grow from the button the popup belongs to.
        let t = ease_out_quint(self.anim.open);
        if t < 1.0 {
            let pivot = [layout.player.centre()[0], rects.popup.y];
            let size = theme.player_scale_from + (1.0 - theme.player_scale_from) * t;
            let dy = (1.0 - t) * theme.player_slide * scale;
            let map = Affine::about(pivot, size, 0.0, [0.0, dy]);
            let tint = [1.0, 1.0, 1.0, t];
            shapes.transform_from(first.0, &map, tint);
            text.transform_from(first.1, &map, tint);
        }
    }

    /// The accent glow around the plate, with the effects on (ADR-0123): it flares on the
    /// music's beat like the song select's selection (ADR-0167), or breathes with the clock
    /// when the beat is not known.
    #[allow(clippy::too_many_arguments)]
    fn push_glow(
        &self,
        shapes: &mut ShapeBatch,
        plate: &Shape,
        theme: &Theme,
        scale: f32,
        now: &NowPlayingFrame,
        tint: Color,
        now_ns: u64,
    ) {
        if !theme.effects {
            return;
        }
        let strength = match now.beat.filter(|_| now.available && !now.paused) {
            Some(beat) => {
                theme.player_glow_rest + (1.0 - theme.player_glow_rest) * beat.clamp(0.0, 1.0)
            }
            None => {
                let seconds = (now_ns as f64 / 1e9) as f32;
                let wave =
                    0.5 + 0.5 * (seconds * theme.player_glow_breath * std::f32::consts::TAU).sin();
                theme.player_glow_rest * (0.75 + 0.25 * wave)
            }
        };
        shapes.push_shape(&Shape {
            glow: theme.player_glow_width * scale,
            sheen: 0.0,
            colour: faded(tint, theme.glow_alpha * strength),
            ..*plate
        });
    }

    /// Eases the outline's colour towards the playing difficulty's star colour (the accent
    /// without one) by `now_ns`; at once without effects or on the first frame.
    fn step_tint(&mut self, now: &NowPlayingFrame, theme: &Theme, now_ns: u64) -> Color {
        let target = match now.stars.filter(|_| now.available) {
            Some(stars) => crate::star_colour::for_stars(stars),
            None => theme.accent,
        };
        let dt = self
            .tint_ns
            .map_or(0, |last| now_ns.saturating_sub(last).min(100_000_000));
        self.tint_ns = Some(now_ns);
        let ms = f64::from(theme.player_outline_ms.max(1.0));
        let tint = match self.tint {
            Some(shown) if theme.effects => {
                // A share of the way per step: 99 % of it in `player_outline_ms`.
                let keep = (-4.6 * dt as f64 / (ms * 1e6)).exp() as f32;
                let mix = |a: f32, b: f32| b + (a - b) * keep;
                Color {
                    r: mix(shown.r, target.r),
                    g: mix(shown.g, target.g),
                    b: mix(shown.b, target.b),
                    a: mix(shown.a, target.a),
                }
            }
            _ => target,
        };
        self.tint = Some(tint);
        tint
    }

    /// The progress bar: a track and the part played; it grows under the pointer.
    fn push_bar(
        &self,
        shapes: &mut ShapeBatch,
        rects: &PlayerRects,
        now: &NowPlayingFrame,
        theme: &Theme,
        scale: f32,
        white: AtlasRegion,
    ) {
        let grow = 1.0 + (theme.player_bar_hot - 1.0) * self.anim.bar;
        let height = rects.bar.height * grow;
        let bar = Rect {
            y: rects.bar.y + (rects.bar.height - height) / 2.0,
            height,
            ..rects.bar
        };
        draw(
            shapes,
            white,
            &bar,
            faded(theme.text, 0.2),
            Look::Pill,
            theme,
            scale,
        );
        if now.available && self.anim.fill > 0.0 {
            let fill = Rect {
                width: (bar.width * self.anim.fill).max(bar.height),
                ..bar
            };
            draw(
                shapes,
                white,
                &fill,
                theme.widget_accent_fill,
                Look::Pill,
                theme,
                scale,
            );
        }
    }

    /// The three buttons: rounded leaning plates that brighten under the pointer and pop
    /// when pressed, with a faint shadow; the play and pause icons cross-fade.
    #[allow(clippy::too_many_arguments)]
    fn push_buttons(
        &self,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        rects: &PlayerRects,
        now: &NowPlayingFrame,
        theme: &Theme,
        scale: f32,
        white: AtlasRegion,
    ) {
        let live = if now.available { 1.0 } else { 0.35 };
        let skew = theme.button_skew();
        let swap = ease_out_cubic(self.anim.play);
        for (i, rect) in rects.buttons.iter().enumerate() {
            let hover = self.anim.hover[i];
            let press = self.anim.press[i];
            // Held buttons shrink about their centre.
            let size = 1.0 - (1.0 - theme.player_press_scale) * press;
            let [cx, cy] = rect.centre();
            let rect = Rect {
                x: cx - rect.width * size / 2.0,
                y: cy - rect.height * size / 2.0,
                width: rect.width * size,
                height: rect.height * size,
            };
            let mix = |a: f32, b: f32| a + (b - a) * hover;
            let (base, lit) = (theme.button_fill, theme.button_hover_fill);
            let plate = Shape {
                radius: theme.button_radius * scale,
                skew,
                ..Shape::new(
                    white.texture,
                    white.uv,
                    [rect.x, rect.y],
                    [rect.width, rect.height],
                    Color {
                        r: mix(base.r, lit.r),
                        g: mix(base.g, lit.g),
                        b: mix(base.b, lit.b),
                        a: mix(base.a * 0.6, lit.a) * (0.5 + 0.5 * live),
                    },
                )
            };
            push_shadow(
                shapes,
                &plate,
                theme,
                scale,
                theme.player_button_shadow * (1.0 - 0.6 * press),
            );
            shapes.push_shape(&plate);
            if theme.effects && hover > 0.0 {
                shapes.push_shape(&Shape {
                    outline: theme.outline_width * scale,
                    colour: faded(theme.accent, 0.8 * hover),
                    ..plate
                });
            }
            let share = theme.toolbar_icon_share + 0.2 + 0.04 * hover;
            if i == 1 {
                let pause = faded(theme.text, live * (1.0 - swap));
                let play = faded(theme.text, live * swap);
                if swap < 1.0 {
                    icons::push_icon(text, glyphs, &rect, share, pause, icons::PAUSE);
                }
                if swap > 0.0 {
                    icons::push_icon(text, glyphs, &rect, share, play, icons::PLAY);
                }
            } else {
                let parts = if i == 0 { icons::PREV } else { icons::NEXT };
                icons::push_icon(text, glyphs, &rect, share, faded(theme.text, live), parts);
            }
        }
    }
}

/// The part of `region` that covers `plate` (centre crop), cut by the plate's rounded,
/// leaning edge with the picture upright, at `opacity`.
pub(crate) fn push_cover(
    shapes: &mut ShapeBatch,
    region: AtlasRegion,
    plate: &Shape,
    opacity: f32,
) {
    let (iw, ih) = (region.size.width as f32, region.size.height as f32);
    let [w, h] = plate.size;
    // An upright texture runs across the plate's bounding box.
    let box_w = w + plate.skew_offset().abs();
    if iw <= 0.0 || ih <= 0.0 || box_w <= 0.0 || h <= 0.0 {
        return;
    }
    // Share of the image's width and height the box shows.
    let want = box_w / h;
    let have = iw / ih;
    let (share_w, share_h) = if have > want {
        (want / have, 1.0)
    } else {
        (1.0, have / want)
    };
    let uv = region.uv;
    let (du, dv) = (uv.u1 - uv.u0, uv.v1 - uv.v0);
    let u0 = uv.u0 + du * (1.0 - share_w) / 2.0;
    let v0 = uv.v0 + dv * (1.0 - share_h) / 2.0;
    let crop = UvRect {
        u0,
        v0,
        u1: u0 + du * share_w,
        v1: v0 + dv * share_h,
    };
    shapes.push_shape(&Shape {
        texture: region.texture,
        uv: crop,
        colour: Color {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: opacity,
        },
        upright: true,
        ..*plate
    });
}

#[cfg(test)]
mod tests;
