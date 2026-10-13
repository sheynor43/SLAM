//! Font registry, text shaping and layout, MSDF atlases (ADR-0037).
//!
//! - [`FontRegistry`] owns font bytes and groups faces into families.
//! - [`LayoutContext`] shapes and lays out text into a reusable [`Layout`]
//!   without heap allocations once its buffers are warm.

mod layout;
mod registry;

pub use layout::{Align, Layout, LayoutContext, Line, PositionedGlyph, TextStyle};
pub use registry::{
    FamilyId, FontError, FontId, FontRegistry, HEADING_FAMILY, ResolvedFace, UI_FAMILY,
    match_weight,
};
