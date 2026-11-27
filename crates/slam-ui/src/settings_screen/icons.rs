//! The icons of the settings sections' tabs and the toolbar (ADR-0165, ADR-0182): SLAM's own
//! set, drawn as MSDF glyphs by `slam-text`, so they are crisp at any size and take any tint.

use slam_render::Color;
use slam_render::sprite::SpriteBatch;
use slam_text::GlyphCache;
pub use slam_text::Icon;

use crate::menu::Rect;

/// Cog (the toolbar's settings button, ADR-0168).
pub(crate) const GEAR: Icon = Icon::Gear;
/// House (the toolbar's home button).
pub(crate) const HOME: Icon = Icon::Home;
/// Music note (the toolbar's player button, ADR-0178).
pub(crate) const NOTE: Icon = Icon::Note;
/// Play triangle (the player's button while paused).
pub(crate) const PLAY: Icon = Icon::Play;
/// Two bars (the player's button while playing).
pub(crate) const PAUSE: Icon = Icon::Pause;
/// Bar and a triangle pointing left.
pub(crate) const PREV: Icon = Icon::Prev;
/// Triangle pointing right and a bar.
pub(crate) const NEXT: Icon = Icon::Next;

/// Number of section icons: one per section.
pub const ICONS: usize = 10;

/// The icon of section `section`; the last one's for any further section.
pub fn icon(section: usize) -> Icon {
    const ALL: [Icon; ICONS] = [
        Icon::Audio,
        Icon::Offset,
        Icon::Keys,
        Icon::Layout,
        Icon::Skin,
        Icon::Frame,
        Icon::Gameplay,
        Icon::Library,
        Icon::Tablet,
        Icon::Feedback,
    ];
    ALL[section.min(ICONS - 1)]
}

/// Records `icon` centred in `rect`, `share` of its side big, in `colour` (already faded),
/// into the text batch. Does not allocate once the batch has room.
pub fn push_icon(
    text: &mut SpriteBatch,
    glyphs: &mut GlyphCache,
    rect: &Rect,
    share: f32,
    colour: Color,
    icon: Icon,
) {
    let side = rect.width.min(rect.height) * share;
    let x0 = rect.x + (rect.width - side) / 2.0;
    let y0 = rect.y + (rect.height - side) / 2.0;
    slam_text::push_icon(text, glyphs, icon, [x0, y0], side, colour);
}
