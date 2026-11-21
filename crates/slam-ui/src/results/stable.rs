//! The results screen in osu!stable's look (ADR-0158): the pure layout of the skin's ranking
//! elements, from the window size, the images' pixel sizes and the skin's settings. The
//! application resolves its uploaded images into a [`StableSkin`]; [`lay_out`] hands every
//! sprite of the screen to a sink, in drawing order, without allocating.
//!
//! Stable's screen is not in lazer, so the reference is McOsu's `OsuRankingScreen`
//! (ADR-0150): a 640x480 design fitted into the window as 4:3, sizes in "osu pixels".

use slam_render::atlas::UvRect;
use slam_render::textures::TextureId;

use crate::menu::Rect;

use super::{Rank, ResultsData, StatKind};

/// Letter images, in [`GRADE_NAMES`] order.
pub const GRADES: usize = 8;
/// Hit icons, in [`HIT_NAMES`] order.
pub const HITS: usize = 6;
/// Characters of a font: the digits, then dot, percent and `x`.
pub const GLYPHS: usize = 13;
/// Mod icons, in [`MODS`] order.
pub const MOD_COUNT: usize = 17;
/// The most sprites [`lay_out`] hands out: the panel, the title, the letter, the labels,
/// the perfect banner, the hit icons, the mods and the digits of the counters.
pub const MAX_STABLE_SPRITES: usize = 192;

/// Skin element names of the letters: `XH`, `SH`, `X`, `S`, `A`, `B`, `C`, `D`.
pub const GRADE_NAMES: [&str; GRADES] = [
    "ranking-XH",
    "ranking-SH",
    "ranking-X",
    "ranking-S",
    "ranking-A",
    "ranking-B",
    "ranking-C",
    "ranking-D",
];

/// Skin element names of the hit icons: 300, 100, 50, 300g, 100k, miss.
pub const HIT_NAMES: [&str; HITS] = ["hit300", "hit100", "hit50", "hit300g", "hit100k", "hit0"];

/// The mod icons in the order stable draws them, with the acronyms that show each.
// Ported from McOsu db2add2: src/App/Osu/OsuRankingScreen.cpp (draw)
pub const MODS: [(&str, &[&str]); MOD_COUNT] = [
    ("selection-mod-touchdevice", &["TD"]),
    ("selection-mod-perfect", &["PF"]),
    ("selection-mod-suddendeath", &["SD"]),
    ("selection-mod-easy", &["EZ"]),
    ("selection-mod-hidden", &["HD"]),
    ("selection-mod-hardrock", &["HR"]),
    ("selection-mod-nightcore", &["NC"]),
    ("selection-mod-doubletime", &["DT"]),
    ("selection-mod-target", &["TP"]),
    ("selection-mod-spunout", &["SO"]),
    ("selection-mod-relax", &["RX"]),
    ("selection-mod-nofail", &["NF"]),
    ("selection-mod-halftime", &["HT"]),
    ("selection-mod-daycore", &["DC"]),
    ("selection-mod-flashlight", &["FL"]),
    ("selection-mod-relax2", &["AP"]),
    ("selection-mod-autoplay", &["AT"]),
];

/// Index of the letter image of `rank`: stable has no F, so F shows the D.
pub const fn grade_index(rank: Rank) -> usize {
    match rank {
        Rank::XH => 0,
        Rank::SH => 1,
        Rank::X => 2,
        Rank::S => 3,
        Rank::A => 4,
        Rank::B => 5,
        Rank::C => 6,
        Rank::D | Rank::F => 7,
    }
}

/// The image name of the letter of `rank`.
pub const fn grade_name(rank: Rank) -> &'static str {
    GRADE_NAMES[grade_index(rank)]
}

/// A character of a legacy font.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Digit(u8),
    Dot,
    Percent,
    X,
}

impl Glyph {
    /// Position in a font's array.
    pub const fn index(self) -> usize {
        match self {
            Glyph::Digit(d) => d as usize,
            Glyph::Dot => 10,
            Glyph::Percent => 11,
            Glyph::X => 12,
        }
    }
}

/// One value per image the screen uses. The loader holds file names, the update thread
/// texture handles and the draw thread sprites.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StableImages<T> {
    pub panel: T,
    pub title: T,
    pub max_combo: T,
    pub accuracy: T,
    pub perfect: T,
    pub grades: [T; GRADES],
    pub hits: [T; HITS],
    /// Score font: the digits, dot and percent ([`Glyph::index`]).
    pub score_font: [T; GLYPHS],
    /// Combo font: the digits and `x`.
    pub combo_font: [T; GLYPHS],
    pub mods: [T; MOD_COUNT],
    /// `menu-back`: the Back button at the bottom left (ADR-0161).
    pub back: T,
    /// `ranking-retry`.
    pub retry: T,
    /// `ranking-graph`, under the health line.
    pub graph: T,
}

impl<T: Default> Default for StableImages<T> {
    fn default() -> Self {
        StableImages {
            panel: T::default(),
            title: T::default(),
            max_combo: T::default(),
            accuracy: T::default(),
            perfect: T::default(),
            grades: std::array::from_fn(|_| T::default()),
            hits: std::array::from_fn(|_| T::default()),
            score_font: std::array::from_fn(|_| T::default()),
            combo_font: std::array::from_fn(|_| T::default()),
            mods: std::array::from_fn(|_| T::default()),
            back: T::default(),
            retry: T::default(),
            graph: T::default(),
        }
    }
}

impl<T> StableImages<T> {
    /// Every value, in field order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        [
            &self.panel,
            &self.title,
            &self.max_combo,
            &self.accuracy,
            &self.perfect,
            &self.back,
            &self.retry,
            &self.graph,
        ]
        .into_iter()
        .chain(&self.grades)
        .chain(&self.hits)
        .chain(&self.score_font)
        .chain(&self.combo_font)
        .chain(&self.mods)
    }

    /// `f` applied to every value.
    pub fn map<U>(&self, mut f: impl FnMut(&T) -> U) -> StableImages<U> {
        StableImages {
            panel: f(&self.panel),
            title: f(&self.title),
            max_combo: f(&self.max_combo),
            accuracy: f(&self.accuracy),
            perfect: f(&self.perfect),
            grades: std::array::from_fn(|i| f(&self.grades[i])),
            hits: std::array::from_fn(|i| f(&self.hits[i])),
            score_font: std::array::from_fn(|i| f(&self.score_font[i])),
            combo_font: std::array::from_fn(|i| f(&self.combo_font[i])),
            mods: std::array::from_fn(|i| f(&self.mods[i])),
            back: f(&self.back),
            retry: f(&self.retry),
            graph: f(&self.graph),
        }
    }
}

/// An uploaded image of the skin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StableSprite {
    pub texture: TextureId,
    pub uv: UvRect,
    /// The file's size in pixels.
    pub size: [f32; 2],
    /// 2 for an `@2x` file, 1 otherwise.
    pub density: f32,
}

/// The skin as the layout needs it. Missing images are skipped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StableSkin {
    /// Whether the skin's `Version` is above 1.0: stable moves several elements then.
    pub modern: bool,
    pub score_overlap: f32,
    pub combo_overlap: f32,
    pub images: StableImages<Option<StableSprite>>,
}

/// The pixel size of a skin image and its density: all the layout needs of an image that
/// it does not draw itself (the update thread knows the sizes from the files' headers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtSize {
    pub size: [f32; 2],
    pub density: f32,
}

/// The sizes of the images that decide where the buttons and the scroll range are: shared
/// by hit testing (update thread) and drawing, so that both see the same rectangles.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StableArt {
    pub title: Option<ArtSize>,
    pub back: Option<ArtSize>,
    pub retry: Option<ArtSize>,
}

impl StableSprite {
    pub fn art(&self) -> ArtSize {
        ArtSize {
            size: self.size,
            density: self.density,
        }
    }
}

impl StableSkin {
    pub fn art(&self) -> StableArt {
        StableArt {
            title: self.images.title.as_ref().map(StableSprite::art),
            back: self.images.back.as_ref().map(StableSprite::art),
            retry: self.images.retry.as_ref().map(StableSprite::art),
        }
    }
}

/// McOsu's `OsuUIBackButton` base size at `osuSize` 54.
const BACK_BASE: [f32; 2] = [225.0, 87.0];
/// `ranking-retry` and `ranking-graph` are placed on stable's 1024x768 screen.
const STABLE_SCREEN_HEIGHT: f32 = 768.0;
/// Centre of `ranking-retry` on stable's screen (estimated from screenshots, ADR-0161).
const RETRY_CENTRE_Y: f32 = 576.0;
/// Height of the bar across the top on a screen 768 high, whatever the title (stable's
/// results at 1920×1080 with and without a `ranking-title`: 135 px; McOsu sizes it by the
/// title instead).
const BAR_HEIGHT: f32 = 96.0;
/// The bar's opacity: stable lets about a fifth of the background through it (measured on
/// stable's results; McOsu's bar is opaque).
pub const BAR_ALPHA: f32 = 0.8;
/// Gap between `ranking-title`'s right edge and the screen's on a screen 768 high (stable's
/// results at 1920×1080: 45 px).
const TITLE_RIGHT_MARGIN: f32 = 32.0;
/// Top-left of `ranking-graph` on stable's screen (estimated from screenshots, ADR-0161).
pub const GRAPH_AT: [f32; 2] = [256.0, 608.0];
/// The most segments of the health line.
pub const MAX_HEALTH_SEGMENTS: usize = 256;
/// How long the Back button's highlight takes, in ns.
pub const HOVER_NS: u64 = 100_000_000;
/// How much brighter a hovered button's sprite gets at full hover: its colour is scaled
/// by `1 + HOVER_GAIN` (no shape behind it, like stable).
pub const HOVER_GAIN: f32 = 0.28;

/// The colour factor of a button sprite at hover animation `amount` (0..=1).
pub fn hover_factor(amount: f32) -> f32 {
    1.0 + HOVER_GAIN * amount.clamp(0.0, 1.0)
}

/// How tall the details block is and how its rows are spaced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailsMetrics {
    /// Pitch of the grid's and the extra lines' rows, shrunk when the block would not fit.
    pub pitch: f32,
    /// Rows of the two-column grid (the taller column).
    pub grid_rows: usize,
    /// Total height of the panel.
    pub height: f32,
}

/// The details block: a heading, a grid of `grid_rows` rows and `extra` lines under it,
/// all in `pad` padding. The pitch is cut so that the panel stays within `available`
/// (the scroll range must reach all of it).
pub fn details_metrics(
    grid_rows: usize,
    extra: usize,
    pitch: f32,
    heading: f32,
    pad: f32,
    available: f32,
) -> DetailsMetrics {
    // A half row of air between the grid and the extra lines.
    let gap = if extra > 0 && grid_rows > 0 { 0.5 } else { 0.0 };
    let rows = grid_rows as f32 + extra as f32 + gap;
    let fixed = 2.0 * pad + heading;
    let pitch = if rows > 0.0 && fixed + rows * pitch > available {
        ((available - fixed) / rows).max(0.0)
    } else {
        pitch
    };
    DetailsMetrics {
        pitch,
        grid_rows,
        height: fixed + rows * pitch,
    }
}

/// How long the scroll eases to its target, in ns.
pub const SCROLL_NS: u64 = 200_000_000;

/// Digits the score shows at least, with leading zeros (stable's `06848980`).
const SCORE_DIGITS: usize = 8;
/// Where the score's middle is, in units of 640×480 from the panel's left (stable's).
const SCORE_CENTRE_X: f32 = 220.0;
/// How much lower than McOsu stable draws the score and the other numbers, in units.
const SCORE_DROP: f32 = 10.0;
const NUMBER_DROP: f32 = 3.0;

/// Decimal digits of `n`, 1 for 0.
fn digit_count(mut n: u64) -> usize {
    let mut count = 1;
    while n >= 10 {
        n /= 10;
        count += 1;
    }
    count
}

/// Height of the score's `0` image on stable's results, in units of 640×480.
const SCORE_DIGIT_HEIGHT: f32 = 30.0;
/// Height of the `0` image of the counts, the combo and the accuracy, in units of 640×480.
const NUMBER_DIGIT_HEIGHT: f32 = 26.0;

/// Line height of the header's font in osu pixels: a stand-in for McOsu's font height,
/// matched to stable's results at 1920×1080 (the title about 36 px).
const HEADER_FONT: f32 = 16.0;
/// `m_fSubTitleScale`: 0.6 * 1.3.
// Ported from McOsu db2add2: src/App/Osu/OsuUIRankingScreenInfoLabel.cpp
pub const SUBTITLE_SCALE: f32 = 0.78;
/// `m_iMargin` (10 px in McOsu, not scaled); in osu pixels, matched to stable's spacing.
const HEADER_MARGIN: f32 = 2.0;

/// The header font's em size at a unit, before the header's scale.
pub fn header_em(unit: f32) -> f32 {
    unit * HEADER_FONT
}

/// `getMinimumHeight` of the info label: the three lines and the margins.
// Ported from McOsu db2add2: src/App/Osu/OsuUIRankingScreenInfoLabel.cpp (getMinimumHeight)
pub fn header_min_height(unit: f32) -> f32 {
    let h = unit * HEADER_FONT;
    h + 2.0 * h * SUBTITLE_SCALE + 2.0 * HEADER_MARGIN * unit
}

/// What a header line says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderKind {
    Title,
    By,
    Played,
}

/// One line of the header: its top-left corner and the factor of [`header_em`] it is
/// set in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeaderLine {
    pub kind: HeaderKind,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
}

/// The header's lines for a bar of `bar_height`; the "Beatmap by" line is skipped when
/// `has_by` is false. Returns the lines and how many are used.
// Ported from McOsu db2add2: src/App/Osu/OsuUIRankingScreenInfoLabel.cpp (draw)
pub fn header_lines(unit: f32, bar_height: f32, has_by: bool) -> ([HeaderLine; 3], usize) {
    let h = unit * HEADER_FONT;
    let global = (bar_height / header_min_height(unit) * 0.741).max(1.0);
    let sub = SUBTITLE_SCALE * global;
    let margin = HEADER_MARGIN * unit;
    let (x, top) = (5.0 * unit, 5.0 * unit);
    let second = top + h * global + margin;
    let line = |kind, y, scale| HeaderLine { kind, x, y, scale };
    let mut lines = [line(HeaderKind::Title, top, global); 3];
    let mut count = 1;
    let mut y = second;
    if has_by {
        lines[count] = line(HeaderKind::By, y, sub);
        count += 1;
        y += h * sub + margin;
    }
    lines[count] = line(HeaderKind::Played, y, sub);
    (lines, count + 1)
}

/// Where the interactive parts of the stable look are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// Height of the black bar.
    pub bar_height: f32,
    /// `menu-back`, at the bottom left.
    pub back: Rect,
    /// `ranking-retry` unscrolled; empty without the image.
    pub retry: Rect,
    /// Pixels the content scrolls over its whole range: the area below the bar.
    pub scroll_area: f32,
    /// Pixels per window pixel of height / 768: the scale of `ranking-retry` and
    /// `ranking-graph`.
    pub screen_scale: f32,
}

/// Height of the bar across the top of a window `height` high, at least `min_bar` (its
/// text's height).
fn bar_height(height: f32, min_bar: f32) -> f32 {
    (BAR_HEIGHT * height / STABLE_SCREEN_HEIGHT).max(min_bar)
}

/// The geometry for a `width` x `height` window.
// Ported from McOsu db2add2: src/App/Osu/OsuUIBackButton.cpp (updateLayout)
// Ported from McOsu db2add2: src/App/Osu/OsuRankingScreen.cpp (updateLayout)
pub fn geometry(width: f32, height: f32, art: &StableArt) -> Geometry {
    let m = Metrics::new(width, height);
    let bar_height = bar_height(height, header_min_height(m.ui(1.0)));

    // The back button: the image at its display size (`@2x` halved) on a screen 768 high,
    // its height clamped to 1.5 times McOsu's base height (stable's results, measured with
    // skins of `@1x` and `@2x` buttons; McOsu stretches it to the base width).
    let f = height / STABLE_SCREEN_HEIGHT;
    let (raw, density) = art.back.map_or((BACK_BASE, 1.0), |a| (a.size, a.density));
    let natural = [raw[0] / density * f, raw[1] / density * f];
    let clamped = natural[1].min(BACK_BASE[1] * 1.5 * f);
    let back_size = [natural[0] * clamped / natural[1], clamped];
    let back = Rect {
        x: 0.0,
        y: height - back_size[1],
        width: back_size[0],
        height: back_size[1],
    };

    let retry = art.retry.map_or_else(Rect::default, |a| {
        let size = [a.size[0] / a.density * f, a.size[1] / a.density * f];
        Rect {
            x: width - size[0],
            y: RETRY_CENTRE_Y * f - size[1] / 2.0,
            width: size[0],
            height: size[1],
        }
    });
    Geometry {
        bar_height,
        back,
        retry,
        scroll_area: (height - bar_height).max(0.0),
        screen_scale: f,
    }
}

/// Hands the segments of the health line inside `graph` (the image's rectangle, inset by
/// 8% on every side) to `sink` as `(from, to, below_half)`. At most
/// [`MAX_HEALTH_SEGMENTS`] segments; longer curves are sampled.
pub fn health_segments(
    graph: Rect,
    health: &[f32],
    mut sink: impl FnMut([f32; 2], [f32; 2], bool),
) {
    if health.len() < 2 {
        return;
    }
    let inset = 0.08;
    let (x, y) = (
        graph.x + graph.width * inset,
        graph.y + graph.height * inset,
    );
    let (w, h) = (
        graph.width * (1.0 - 2.0 * inset),
        graph.height * (1.0 - 2.0 * inset),
    );
    let segments = (health.len() - 1).min(MAX_HEALTH_SEGMENTS);
    let at = |i: usize| {
        let index = i * (health.len() - 1) / segments;
        let value = health[index].clamp(0.0, 1.0);
        (
            [x + w * i as f32 / segments as f32, y + h - value * h],
            value,
        )
    };
    let mut previous = at(0);
    for i in 1..=segments {
        let next = at(i);
        sink(previous.0, next.0, (previous.1 + next.1) / 2.0 < 0.5);
        previous = next;
    }
}

/// What a placed sprite is, for tests and for callers that tint parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Panel,
    Title,
    Grade,
    MaxComboLabel,
    AccuracyLabel,
    Perfect,
    /// Index into [`HIT_NAMES`].
    Hit(usize),
    /// Index into [`MODS`].
    Mod(usize),
    /// A glyph of the score font (the score, the accuracy).
    Score(Glyph),
    /// A glyph of the combo font (hit counts, max combo).
    Combo(Glyph),
}

/// What [`lay_out`] hands out, in drawing order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Item<'a> {
    /// The black bar across the top.
    Bar { height: f32 },
    Sprite {
        part: Part,
        sprite: &'a StableSprite,
        centre: [f32; 2],
        size: [f32; 2],
    },
}

/// Where the screen ended up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StableInfo {
    /// Height of the black bar; the beatmap's text sits in it.
    pub bar_height: f32,
    /// Left edge of the title image (or the window's right edge less its margin when the
    /// skin has none): the text must end before it.
    pub title_left: f32,
    /// Pixels per osu pixel of the 640x480 design.
    pub unit: f32,
}

/// The 4:3 area of the window the design is fitted into.
// Ported from McOsu db2add2: src/App/Osu/Osu.cpp (getUIScale, getImageScale)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    x: f32,
    y: f32,
}

impl Metrics {
    pub fn new(width: f32, height: f32) -> Metrics {
        // Integers, as the original does: the area is clipped to 4:3.
        let (mut w, mut h) = (width as i32, height as i32);
        if w * 3 > h * 4 {
            w = h * 4 / 3;
        } else {
            h = w * 3 / 4;
        }
        Metrics {
            x: w as f32 / 640.0,
            y: h as f32 / 480.0,
        }
    }

    /// `getUIScale`: `v` osu pixels in window pixels.
    pub fn ui(&self, v: f32) -> f32 {
        (v * self.x).max(v * self.y)
    }

    /// `getImageScale`: the scale that makes an image of `size` pixels `osu_size` osu
    /// pixels big.
    pub fn image_scale(&self, size: [f32; 2], osu_size: f32) -> f32 {
        let (dx, dy) = (osu_size * self.x / size[0], osu_size * self.y / size[1]);
        dx.max(dy)
    }
}

/// The decimal digits of `n`, at least `min_len` of them, as 0..=9 values.
fn digits(mut n: u64, min_len: usize, out: &mut [u8; 20]) -> usize {
    let mut len = 0;
    loop {
        out[len] = (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    while len < min_len.min(out.len()) {
        out[len] = 0;
        len += 1;
    }
    out[..len].reverse();
    len
}

/// Splits an accuracy in percent as stable shows it: the integer part and the fraction
/// rounded to two digits, clamped to 99.
// Ported from McOsu db2add2: src/App/Osu/OsuHUD.cpp (drawAccuracySimple)
pub fn accuracy_parts(percent: f32) -> (u64, u64) {
    let int = percent as i32;
    let frac = ((percent - int as f32) * 100.0).round() as i32;
    (int.max(0) as u64, frac.clamp(0, 99) as u64)
}

/// The mod icons `data` shows: indices into [`MODS`], in drawing order. PF hides SD and NC
/// hides DT.
// Ported from McOsu db2add2: src/App/Osu/OsuRankingScreen.cpp (draw)
pub fn mod_icons(data: &ResultsData) -> impl Iterator<Item = usize> + '_ {
    let has = |acronym: &str| data.mods.iter().any(|m| m.as_str() == acronym);
    MODS.iter()
        .enumerate()
        .filter_map(move |(i, (_, acronyms))| {
            let shown = acronyms.iter().any(|a| has(a))
                && !(acronyms[0] == "SD" && has("PF"))
                && !(acronyms[0] == "DT" && has("NC"));
            shown.then_some(i)
        })
}

/// The right edge of the panel (it is placed at the window's left).
pub fn panel_right(width: f32, height: f32) -> f32 {
    let m = Metrics::new(width, height);
    // Independent of the file's density: the panel is 622x505 osu pixels at most as wide
    // as the scale of its height or width allows.
    let hard = [622.0, 505.0];
    hard[0] * m.image_scale(hard, 317.0)
}

/// A number being drawn in a font: McOsu advances by the width of the score font's `0`
/// for every digit.
struct Pen<'a, 'b, F: FnMut(Item<'a>)> {
    sink: &'b mut F,
    font: &'a [Option<StableSprite>; GLYPHS],
    part: fn(Glyph) -> Part,
    /// Digit advance in pixels (width of the font's `0` times `scale`).
    advance: f32,
    scale: f32,
    spacing: f32,
    x: f32,
    cy: f32,
}

impl<'a, F: FnMut(Item<'a>)> Pen<'a, '_, F> {
    fn put(&mut self, glyph: Glyph, centre_x: f32) {
        if let Some(sprite) = &self.font[glyph.index()] {
            (self.sink)(Item::Sprite {
                part: (self.part)(glyph),
                sprite,
                centre: [centre_x, self.cy],
                size: [sprite.size[0] * self.scale, sprite.size[1] * self.scale],
            });
        }
    }

    // Ported from McOsu db2add2: src/App/Osu/OsuHUD.cpp (drawScoreNumber, drawComboNumber)
    fn number(&mut self, value: u64, min_len: usize) {
        let mut buffer = [0; 20];
        let len = digits(value, min_len, &mut buffer);
        for &d in &buffer[..len] {
            self.put(Glyph::Digit(d), self.x + self.advance / 2.0);
            self.x += self.advance - self.spacing;
        }
    }

    /// A symbol that ends the number (`x`, `%`).
    fn tail(&mut self, glyph: Glyph) {
        if let Some(sprite) = &self.font[glyph.index()] {
            let width = sprite.size[0] * self.scale;
            self.put(glyph, self.x + width / 2.0);
            self.x += width;
        }
    }

    /// The dot between the integer and fractional part.
    fn dot(&mut self) {
        if let Some(sprite) = &self.font[Glyph::Dot.index()] {
            let width = sprite.size[0] * self.scale;
            self.put(Glyph::Dot, self.x + width / 2.0);
            self.x += width - self.spacing;
        }
    }
}

/// A pen on the score font whose line's top is at `top`.
fn score_pen<'a, 'b, F: FnMut(Item<'a>)>(
    sink: &'b mut F,
    font: &'a [Option<StableSprite>; GLYPHS],
    zero: &StableSprite,
    spacing: f32,
    scale: f32,
    x: f32,
    top: f32,
) -> Pen<'a, 'b, F> {
    Pen {
        sink,
        font,
        part: Part::Score,
        advance: zero.size[0] * scale,
        scale,
        spacing: spacing * scale,
        x,
        cy: top + zero.size[1] / 2.0 * scale,
    }
}

/// Hands every sprite of the screen for a `width` x `height` window to `sink`, in drawing
/// order. `min_bar` is the least height of the black bar (the height of its text).
///
/// The caller draws only when the skin has a panel; the layout itself skips every missing
/// image.
// Ported from McOsu db2add2: src/App/Osu/OsuRankingScreen.cpp (updateLayout, setGrade, draw)
// Ported from McOsu db2add2: src/App/Osu/OsuUIRankingScreenRankingPanel.cpp (draw)
pub fn lay_out<'a>(
    skin: &'a StableSkin,
    data: &ResultsData,
    width: f32,
    height: f32,
    min_bar: f32,
    mut sink: impl FnMut(Item<'a>),
) -> StableInfo {
    let m = Metrics::new(width, height);
    let images = &skin.images;
    let modern = skin.modern;
    let sprite_item =
        |part, sprite: &'a StableSprite, centre: [f32; 2], size: [f32; 2]| Item::Sprite {
            part,
            sprite,
            centre,
            size,
        };
    // McOsu's container is as wide as the screen plus two pixels.
    let rankings_width = width + 2.0;

    // The title: at its own size on a screen 768 high (`@2x` halved), at the top, a margin
    // off the right edge (stable's results; McOsu fits it to 75 units and the bar to it).
    let f = height / STABLE_SCREEN_HEIGHT;
    let title = images.title.as_ref().map(|s| {
        let scale = f / s.density;
        (s, [s.size[0] * scale, s.size[1] * scale])
    });
    let bar_height = bar_height(height, min_bar);
    let title_left = title.map_or(width, |(_, size)| width - size[0] - TITLE_RIGHT_MARGIN * f);

    // The panel and everything on it are placed from its top-left corner.
    let panel_x = 0.0;
    let panel_y = bar_height - 1.0;
    let global_y = m.ui(-1.0).trunc();

    if let Some(panel) = &images.panel {
        let hard = [622.0 * panel.density, 505.0 * panel.density];
        let scale = m.image_scale(hard, 317.0);
        let size = [
            (hard[0] * scale).max(panel.size[0] * scale),
            (hard[1] * scale).max(panel.size[1] * scale),
        ];
        sink(sprite_item(
            Part::Panel,
            panel,
            [panel_x + size[0] / 2.0, panel_y + size[1] / 2.0],
            size,
        ));
    }

    // Numbers are scaled by the score font's `0`. Stable fits the image's height: the
    // score's `0` is 30 units of 640×480 high and the other numbers' 26, whatever its
    // width (measured on stable's results with skins of narrow and wide digits; McOsu's
    // `getImageScale(…, 20 or 17) × 1.3` fits the wider side and draws narrow digits far
    // too big). All of them are in the score font, as stable draws them (ADR-0161).
    let _ = modern;
    if let Some(zero) = &images.score_font[0] {
        let scale20 = m.ui(SCORE_DIGIT_HEIGHT) / zero.size[1];
        let scale17 = m.ui(NUMBER_DIGIT_HEIGHT) / zero.size[1];
        let half = zero.size[1] / 2.0;
        let score_spacing = skin.score_overlap * zero.density;
        let combo_spacing = score_spacing;
        let combo_advance = zero.size[0];
        // The score: eight digits at least, centred on a point of the panel, a little lower
        // than McOsu draws it (both measured on stable's results).
        let score = data.score.max(0) as u64;
        let len = digit_count(score).max(SCORE_DIGITS);
        let width = len as f32 * (zero.size[0] - score_spacing) * scale20 + score_spacing * scale20;
        let mut pen = score_pen(
            &mut sink,
            &images.score_font,
            zero,
            score_spacing,
            scale20,
            panel_x + m.ui(SCORE_CENTRE_X) - width / 2.0,
            panel_y + m.ui(11.0 + SCORE_DROP) + global_y,
        );
        pen.number(score, SCORE_DIGITS);

        // The hit icons and their counts: two columns of three.
        let start = [40.0, 99.0];
        let count_start = [
            start[0] + 40.0,
            start[1] + if modern { -16.0 } else { -25.0 } + NUMBER_DROP,
        ];
        let counts = [
            data.stats[StatKind::Great.index()].value,
            data.stats[StatKind::Ok.index()].value,
            data.stats[StatKind::Meh.index()].value,
            // Gekis and katus: not counted in the lazer profile (ADR-0158).
            0,
            0,
            data.stats[StatKind::Miss.index()].value,
        ];
        let hit_scale = m.image_scale([128.0, 128.0], 42.0);
        for i in 0..HITS {
            let (column, row) = ((i / 3) as f32, (i % 3) as f32);
            if let Some(sprite) = &images.hits[i] {
                let scale = hit_scale / sprite.density;
                sink(sprite_item(
                    Part::Hit(i),
                    sprite,
                    [
                        panel_x + m.ui(start[0] + 200.0 * column),
                        panel_y + m.ui(start[1] + 60.0 * row),
                    ],
                    [sprite.size[0] * scale, sprite.size[1] * scale],
                ));
            }
        }
        for (i, count) in counts.into_iter().enumerate() {
            let (column, row) = ((i / 3) as f32, (i % 3) as f32);
            let mut pen = Pen {
                sink: &mut sink,
                font: &images.score_font,
                part: Part::Combo,
                advance: combo_advance * scale17,
                scale: scale17,
                spacing: combo_spacing * scale17,
                x: panel_x + m.ui(count_start[0] + 200.0 * column),
                cy: panel_y + half * scale17 + m.ui(count_start[1] + 60.0 * row),
            };
            pen.number(u64::from(count), 1);
            pen.tail(Glyph::X);
        }

        // The fourth row: max combo and accuracy with their labels.
        let row4 = 260.0;
        let label_offset = if modern { 20.0 } else { 8.0 } - 20.0;
        let row_cy = panel_y + half * scale17 + m.ui(row4 + 10.0 + NUMBER_DROP) + global_y;
        let mut pen = Pen {
            sink: &mut sink,
            font: &images.score_font,
            part: Part::Combo,
            advance: combo_advance * scale17,
            scale: scale17,
            spacing: combo_spacing * scale17,
            x: panel_x + m.ui(15.0),
            cy: row_cy,
        };
        pen.number(u64::from(data.combo), 1);
        pen.tail(Glyph::X);

        if let Some(label) = &images.max_combo {
            let scale = m.image_scale([162.0 * label.density, 50.0 * label.density], 32.0);
            let size = [label.size[0] * scale, label.size[1] * scale];
            sink(sprite_item(
                Part::MaxComboLabel,
                label,
                [
                    panel_x + size[0] * 0.5 + m.ui(4.0),
                    panel_y + m.ui(row4 - 5.0 - label_offset) + global_y,
                ],
                size,
            ));
        }

        let (int, frac) = accuracy_parts((data.accuracy * 100.0) as f32);
        let mut pen = score_pen(
            &mut sink,
            &images.score_font,
            zero,
            score_spacing,
            scale17,
            panel_x + m.ui(195.0),
            0.0,
        );
        pen.cy = row_cy;
        pen.number(int, 2);
        pen.dot();
        pen.number(frac, 2);
        pen.tail(Glyph::Percent);

        if let Some(label) = &images.accuracy {
            let scale = m.image_scale([192.0 * label.density, 58.0 * label.density], 36.0);
            let size = [label.size[0] * scale, label.size[1] * scale];
            sink(sprite_item(
                Part::AccuracyLabel,
                label,
                [
                    panel_x + size[0] * 0.5 + m.ui(183.0),
                    panel_y + m.ui(row4 - 3.0 - label_offset) + global_y,
                ],
                size,
            ));
        }
    }

    // The banner of a full combo.
    if data.full_combo()
        && let Some(sprite) = &images.perfect
    {
        let base = [478.0 * sprite.density, 150.0 * sprite.density];
        let scale = m.image_scale(base, 94.0);
        let x = panel_x + m.ui(if modern { 260.0 } else { 200.0 });
        let y = panel_y + (m.ui(430.0) + global_y) * 0.97;
        sink(sprite_item(
            Part::Perfect,
            sprite,
            [x, y - base[1] * scale * 0.5],
            [sprite.size[0] * scale, sprite.size[1] * scale],
        ));
    }

    // The big letter. F shows the D.
    if let Some(sprite) = &images.grades[grade_index(data.rank)] {
        let hard = [369.0 * sprite.density, 422.0 * sprite.density];
        let scale = m.image_scale(hard, 230.0);
        let size = [sprite.size[0] * scale, sprite.size[1] * scale];
        sink(sprite_item(
            Part::Grade,
            sprite,
            [
                rankings_width - m.ui(120.0),
                m.ui(if modern { 200.0 } else { 170.0 }),
            ],
            size,
        ));
    }

    // The mods, right to left.
    let mut x = rankings_width - m.ui(20.0);
    for i in mod_icons(data).take(super::MAX_MODS) {
        if let Some(sprite) = &images.mods[i] {
            let scale = m.image_scale([68.0, 66.0], 38.0) / sprite.density;
            let size = [sprite.size[0] * scale, sprite.size[1] * scale];
            sink(sprite_item(
                Part::Mod(i),
                sprite,
                [x - size[0] / 2.0, m.ui(260.0)],
                size,
            ));
        }
        x -= m.ui(20.0);
    }

    sink(Item::Bar { height: bar_height });
    if let Some((sprite, size)) = title {
        sink(sprite_item(
            Part::Title,
            sprite,
            [title_left + size[0] / 2.0, size[1] / 2.0],
            size,
        ));
    }
    StableInfo {
        bar_height,
        title_left,
        unit: m.ui(1.0),
    }
}

#[cfg(test)]
mod tests;
