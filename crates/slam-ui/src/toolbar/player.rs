//! The toolbar's music player (ADR-0178): a button on the right of the bar opens a popup
//! after lazer's now-playing overlay: the playing beatmap's background dimmed behind its
//! title and artist, a progress bar to click or drag, and the previous, play/pause and next
//! buttons.
//!
//! The update side ([`Toolbar`]'s player state) turns presses into [`PlayerAction`]s that
//! the application carries out on its music; the draw side ([`PlayerView`]) draws the popup
//! from a [`NowPlayingFrame`] and the texts of a [`NowPlayingText`]. Neither allocates once
//! warm; the texts are laid out again (allocating) when the playing track changes.

use std::sync::Arc;

use slam_render::atlas::{AtlasRegion, UvRect};
use slam_render::shape::ShapeBatch;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_text::GlyphCache;

use super::{Target, Toolbar, ToolbarEvent, ToolbarLayout};
use crate::anim::{ease_out_cubic, progress};
use crate::marquee::{Marquee, MarqueeParams, push_marquee};
use crate::menu::Rect;
use crate::menu::view::{Line, Typesetter};
use crate::settings_screen::icons;
use crate::theme::{Theme, faded};
use crate::widgets::{Look, draw, push_left};

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
    /// The bar is being dragged.
    pub seeking: bool,
}

/// The open popup on the update thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlayerOpen {
    since_ns: u64,
    hover: Option<u8>,
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
        let inner = popup.width - 2.0 * pad;
        let title_h = theme.player_title_size * scale * 1.3;
        let artist_h = theme.player_artist_size * scale * 1.3;
        let title = Rect {
            x: popup.x + pad,
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
            x: popup.x + pad,
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

    fn button_at(&self, x: f32, y: f32) -> Option<usize> {
        self.buttons.iter().position(|b| b.contains(x, y))
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
        player.hover = rects.button_at(x, y).map(|i| i as u8);
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
        if !rects.popup.contains(x, y) {
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
        Some(match rects.button_at(x, y) {
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

/// The draw side of the popup: its texts and their marquees.
pub(super) struct PlayerView {
    title: Line,
    artist: Line,
    nothing: Line,
    seen: Option<(u64, Arc<str>, Arc<str>)>,
    title_marquee: Marquee,
    artist_marquee: Marquee,
    scale: f32,
}

impl PlayerView {
    pub(super) fn new(nothing: &str) -> PlayerView {
        PlayerView {
            title: Line::new(""),
            artist: Line::new(""),
            nothing: Line::new(nothing),
            seen: None,
            title_marquee: Marquee::default(),
            artist_marquee: Marquee::default(),
            scale: 0.0,
        }
    }

    pub(super) fn lay_out(&mut self, type_setter: &mut Typesetter, theme: &Theme, scale: f32) {
        self.scale = scale;
        self.seen = None;
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
    ) {
        let same = self.seen.as_ref().is_some_and(|(key, title, artist)| {
            *key == text.key && Arc::ptr_eq(title, &text.title) && Arc::ptr_eq(artist, &text.artist)
        });
        if same {
            return;
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

    /// Draws the popup of `frame` under the player button. `cover` is the playing
    /// beatmap's background, if one is ready. Returns nothing; does not allocate once the
    /// texts are laid out.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push(
        &mut self,
        type_setter: &mut Typesetter,
        shapes: &mut ShapeBatch,
        text: &mut SpriteBatch,
        glyphs: &mut GlyphCache,
        layout: &ToolbarLayout,
        popup: &PlayerFrame,
        now: &NowPlayingFrame,
        words: &NowPlayingText,
        cover: Option<AtlasRegion>,
        now_ns: u64,
        white: AtlasRegion,
    ) {
        let theme = layout.theme;
        let scale = layout.scale;
        let ms = (f64::from(theme.toolbar_popup_ms.max(0.0)) * 1e6) as u64;
        let t = if theme.effects {
            ease_out_cubic(progress(popup.since_ns, now_ns, ms))
        } else {
            1.0
        };
        let dy = (t - 1.0) * theme.toolbar_popup_slide * scale;
        let rects = layout.player_rects(dy);
        if now.available {
            self.follow(type_setter, &theme, words, now_ns);
        }
        draw(
            shapes,
            white,
            &rects.popup,
            faded(theme.widget_popup_fill, t),
            Look::Round,
            &theme,
            scale,
        );
        if let Some(region) = cover {
            push_cover(shapes, region, &rects.popup, t);
            let mut dim = Sprite::new(
                white.texture,
                white.uv,
                [rects.popup.x, rects.popup.y],
                [rects.popup.width, rects.popup.height],
            );
            dim.colour = slam_render::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: theme.player_cover_dim * t,
            };
            shapes.push(&dim);
        }
        let pad = 0.0;
        if now.available {
            let params = MarqueeParams::new(&theme, scale);
            push_marquee(
                text,
                &self.title,
                &rects.title,
                pad,
                faded(theme.text, t),
                glyphs,
                &mut self.title_marquee,
                &params,
                now_ns,
            );
            push_marquee(
                text,
                &self.artist,
                &rects.artist,
                pad,
                faded(theme.text, 0.75 * t),
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
                pad,
                faded(theme.text, 0.75 * t),
                glyphs,
            );
        }
        // The progress bar: a track and the part played.
        let track = faded(theme.text, 0.2 * t);
        draw(shapes, white, &rects.bar, track, Look::Pill, &theme, scale);
        let played = now.progress();
        if now.available && played > 0.0 {
            let fill = Rect {
                width: (rects.bar.width * played).max(rects.bar.height),
                ..rects.bar
            };
            draw(
                shapes,
                white,
                &fill,
                faded(theme.widget_accent_fill, t),
                Look::Pill,
                &theme,
                scale,
            );
        }
        let live = if now.available { 1.0 } else { 0.35 };
        let middle = if now.paused {
            icons::PLAY
        } else {
            icons::PAUSE
        };
        let faces = [icons::PREV, middle, icons::NEXT];
        for (i, (rect, parts)) in rects.buttons.iter().zip(faces).enumerate() {
            if popup.hover == Some(i as u8) && now.available {
                draw(
                    shapes,
                    white,
                    rect,
                    faded(theme.button_hover_fill, t),
                    Look::Round,
                    &theme,
                    scale,
                );
            }
            icons::push_icon(
                shapes,
                white,
                rect,
                theme.toolbar_icon_share + 0.2,
                faded(theme.text, live * t),
                parts,
            );
        }
    }
}

/// The part of `region` that covers `rect` (centre crop), tinted white at `opacity`.
fn push_cover(shapes: &mut ShapeBatch, region: AtlasRegion, rect: &Rect, opacity: f32) {
    let (iw, ih) = (region.size.width as f32, region.size.height as f32);
    if iw <= 0.0 || ih <= 0.0 || rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    // Share of the image's width and height the rectangle shows.
    let want = rect.width / rect.height;
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
    let mut sprite = Sprite::new(
        region.texture,
        crop,
        [rect.x, rect.y],
        [rect.width, rect.height],
    );
    sprite.colour = slam_render::Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: opacity,
    };
    shapes.push(&sprite);
}

#[cfg(test)]
mod tests;
