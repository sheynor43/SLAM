//! Font registry, text shaping and layout, MSDF atlases (ADR-0037).
//!
//! - [`FontRegistry`] owns font bytes and groups faces into families.
//! - [`LayoutContext`] shapes and lays out text into a reusable [`Layout`]
//!   without heap allocations once its buffers are warm.

mod glyphs;
mod icons;
mod layout;
mod msdf;
mod registry;
mod render;
#[cfg(test)]
mod test_support;

pub use glyphs::{Glyph, GlyphAtlasConfig, GlyphCache, GlyphStats, glyph_atlas};
pub use icons::{GRID as ICON_GRID, Icon};
pub use layout::{Align, Layout, LayoutContext, Line, PositionedGlyph, TextStyle};
pub use msdf::{
    DISTANCE_RANGE, EM_PX, EM_PX_HIRES, GlyphBitmap, GlyphError, GlyphKey, MAX_GLYPH_SIDE, PADDING,
    generate_glyph,
};
pub use registry::{
    FALLBACK_FAMILY, FamilyId, FontError, FontId, FontRegistry, HEADING_FAMILY, NORMAL_WIDTH,
    ResolvedFace, UI_FAMILY, match_weight,
};
pub use render::{
    Clip, TextStats, push_icon, push_layout, push_layout_clipped, push_layout_styled,
};
