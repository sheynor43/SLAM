//! Font registry, text shaping and layout, MSDF atlases (ADR-0037).
//!
//! - [`FontRegistry`] owns font bytes and groups faces into families.
//! - [`LayoutContext`] shapes and lays out text into a reusable [`Layout`]
//!   without heap allocations once its buffers are warm.

mod glyphs;
mod layout;
mod msdf;
mod registry;
mod render;
#[cfg(test)]
mod test_support;

pub use glyphs::{Glyph, GlyphAtlasConfig, GlyphCache, GlyphStats, glyph_atlas};
pub use layout::{Align, Layout, LayoutContext, Line, PositionedGlyph, TextStyle};
pub use msdf::{
    DISTANCE_RANGE, EM_PX, GlyphBitmap, GlyphError, GlyphKey, MAX_GLYPH_SIDE, PADDING,
    generate_glyph,
};
pub use registry::{
    FamilyId, FontError, FontId, FontRegistry, HEADING_FAMILY, ResolvedFace, UI_FAMILY,
    match_weight,
};
pub use render::{TextStats, push_layout};
