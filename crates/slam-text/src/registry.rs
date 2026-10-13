//! Font registry: faces, families, weight matching, built-in fonts.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use harfrust::Font;
use skrifa::raw::{FileRef, types::NameId};
use skrifa::{FontRef, MetadataProvider, Tag};

/// Family name of the built-in UI font (Nunito Sans).
pub const UI_FAMILY: &str = "Nunito Sans";
/// Family name of the built-in heading font (Exo 2).
pub const HEADING_FAMILY: &str = "Exo 2";

static NUNITO_SANS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/fonts/nunito-sans/NunitoSans-VariableFont.ttf"
));
static EXO2: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/fonts/exo2/Exo2-VariableFont.ttf"
));

const WGHT: Tag = Tag::new(b"wght");

/// Identifier of one registered face (stable for the lifetime of the registry).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontId(pub u32);

/// Identifier of a font family in a [`FontRegistry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FamilyId(pub u32);

/// Why a font could not be registered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FontError {
    /// The bytes are not a parsable TrueType/OpenType font or collection.
    #[error("data is not a valid TrueType/OpenType font")]
    InvalidFont,
    /// A face has no usable family name in its `name` table.
    #[error("font has no family name")]
    NoFamilyName,
}

/// A face chosen for a family and a requested weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedFace {
    /// The chosen face.
    pub font: FontId,
    /// Weight actually used: the clamped `wght` axis value for variable faces,
    /// the face's own weight class for static ones.
    pub weight: u16,
}

/// Font bytes: either program-lifetime data or shared ownership.
#[derive(Clone)]
enum FontBytes {
    /// Data that lives for the whole program (the built-in fonts).
    Static(&'static [u8]),
    /// Reference-counted data.
    Shared(Arc<[u8]>),
}

impl AsRef<[u8]> for FontBytes {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Static(b) => b,
            Self::Shared(b) => b,
        }
    }
}

impl FontBytes {
    fn bytes(&self) -> &[u8] {
        self.as_ref()
    }

    fn blob(&self) -> harfrust::font::Blob {
        match self {
            Self::Static(b) => harfrust::font::Blob::Static(b),
            Self::Shared(b) => harfrust::font::Blob::Shared(Arc::new(b.clone())),
        }
    }
}

static NEXT_REGISTRY_ID: AtomicU64 = AtomicU64::new(1);

struct FaceEntry {
    data: FontBytes,
    index: u32,
    /// The face at its default location.
    font: Font,
    /// `usWeightClass` (default instance weight for variable faces).
    weight: u16,
    italic: bool,
    /// Range of the `wght` axis, if the face has one.
    wght_axis: Option<(f32, f32)>,
}

struct Family {
    name: String,
    faces: Vec<FontId>,
}

/// Owns font bytes and groups faces into families (ADR-0037).
pub struct FontRegistry {
    id: u64,
    faces: Vec<FaceEntry>,
    families: Vec<Family>,
    fallback: Vec<FamilyId>,
}

impl Default for FontRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FontRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self {
            id: NEXT_REGISTRY_ID.fetch_add(1, Ordering::Relaxed),
            faces: Vec::new(),
            families: Vec::new(),
            fallback: Vec::new(),
        }
    }

    /// Process-unique id of this registry. Faces are only ever appended, so a
    /// [`FontId`] keeps meaning the same face within one registry id.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether the face has a `wght` variation axis.
    pub fn is_weight_variable(&self, font: FontId) -> bool {
        self.faces
            .get(font.0 as usize)
            .is_some_and(|e| e.wght_axis.is_some())
    }

    /// Registry with the built-in fonts ([`UI_FAMILY`], [`HEADING_FAMILY`]);
    /// the fallback chain is `[UI_FAMILY]`.
    pub fn with_builtin() -> Self {
        let mut reg = Self::new();
        // The built-in fonts are validated by tests; a failure here would only
        // leave the registry without them.
        let _ = reg.add_static_font(NUNITO_SANS);
        let _ = reg.add_static_font(EXO2);
        if let Some(ui) = reg.family(UI_FAMILY) {
            reg.fallback = vec![ui];
        }
        reg
    }

    /// Registers all faces of a font file or collection. Nothing is registered on error.
    pub fn add_font(&mut self, data: impl Into<Arc<[u8]>>) -> Result<Vec<FontId>, FontError> {
        self.add(FontBytes::Shared(data.into()))
    }

    /// Like [`add_font`](Self::add_font) for data that lives for the whole program.
    pub fn add_static_font(&mut self, data: &'static [u8]) -> Result<Vec<FontId>, FontError> {
        self.add(FontBytes::Static(data))
    }

    fn add(&mut self, data: FontBytes) -> Result<Vec<FontId>, FontError> {
        let file = FileRef::new(data.bytes()).map_err(|_| FontError::InvalidFont)?;
        let count = match &file {
            FileRef::Font(_) => 1,
            FileRef::Collection(c) => c.len(),
        };
        let mut parsed = Vec::new();
        for index in 0..count {
            let face =
                FontRef::from_index(data.bytes(), index).map_err(|_| FontError::InvalidFont)?;
            let name = family_name(&face).ok_or(FontError::NoFamilyName)?;
            let wght_axis = face
                .axes()
                .get_by_tag(WGHT)
                .map(|a| (a.min_value(), a.max_value()));
            let attrs = face.attributes();
            let weight = (attrs.weight.value().round() as i64).clamp(1, 1000) as u16;
            let italic = !matches!(attrs.style, skrifa::attribute::Style::Normal);
            let font = Font::new(data.blob(), index).ok_or(FontError::InvalidFont)?;
            parsed.push((index, name, weight, italic, wght_axis, font));
        }
        let mut ids = Vec::with_capacity(parsed.len());
        for (index, name, weight, italic, wght_axis, font) in parsed {
            let family = self.family_or_insert(&name);
            let id = FontId(self.faces.len() as u32);
            self.faces.push(FaceEntry {
                data: data.clone(),
                index,
                font,
                weight,
                italic,
                wght_axis,
            });
            self.families[family.0 as usize].faces.push(id);
            ids.push(id);
        }
        Ok(ids)
    }

    fn family_or_insert(&mut self, name: &str) -> FamilyId {
        if let Some(id) = self.family(name) {
            return id;
        }
        let id = FamilyId(self.families.len() as u32);
        self.families.push(Family {
            name: name.to_owned(),
            faces: Vec::new(),
        });
        id
    }

    /// Finds a family by name (ASCII case-insensitive).
    pub fn family(&self, name: &str) -> Option<FamilyId> {
        self.families
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))
            .map(|i| FamilyId(i as u32))
    }

    /// Name of a family.
    pub fn family_name(&self, family: FamilyId) -> Option<&str> {
        self.families
            .get(family.0 as usize)
            .map(|f| f.name.as_str())
    }

    /// Sets the global fallback chain, tried in order after the style's own family.
    pub fn set_fallback(&mut self, families: &[FamilyId]) {
        self.fallback = families.to_vec();
    }

    /// The global fallback chain.
    pub fn fallback(&self) -> &[FamilyId] {
        &self.fallback
    }

    /// Picks the face of `family` for `weight`.
    ///
    /// Upright faces are preferred over italic ones. A face with a `wght` axis
    /// is used with the weight clamped to the axis range; otherwise the nearest
    /// static face by the CSS font-matching rules.
    pub fn resolve(&self, family: FamilyId, weight: u16) -> Option<ResolvedFace> {
        let fam = self.families.get(family.0 as usize)?;
        let any_upright = fam.faces.iter().any(|f| !self.faces[f.0 as usize].italic);
        let mut candidates = fam
            .faces
            .iter()
            .copied()
            .filter(|f| !any_upright || !self.faces[f.0 as usize].italic);
        let variable = candidates
            .clone()
            .find(|f| self.faces[f.0 as usize].wght_axis.is_some());
        if let Some(font) = variable {
            let (lo, hi) = self.faces[font.0 as usize].wght_axis?;
            let w = f32::from(weight).clamp(lo, hi).round();
            return Some(ResolvedFace {
                font,
                weight: w as u16,
            });
        }
        let first = candidates.next()?;
        let mut best = first;
        let mut best_key = css_weight_key(weight, self.faces[first.0 as usize].weight);
        for f in candidates {
            let key = css_weight_key(weight, self.faces[f.0 as usize].weight);
            if key < best_key {
                best = f;
                best_key = key;
            }
        }
        Some(ResolvedFace {
            font: best,
            weight: self.faces[best.0 as usize].weight,
        })
    }

    /// Raw bytes and collection index of a face (for glyph outline extraction).
    pub fn face_data(&self, font: FontId) -> Option<(&[u8], u32)> {
        let e = self.faces.get(font.0 as usize)?;
        Some((e.data.bytes(), e.index))
    }

    /// The face at its default location (cheap to clone; shares its tables).
    pub fn font(&self, font: FontId) -> Option<&Font> {
        self.faces.get(font.0 as usize).map(|e| &e.font)
    }

    /// The face at the location of `face`: the `wght` axis set for variable
    /// faces. Builds a new instance (allocates); layout code caches it.
    pub fn instance(&self, face: ResolvedFace) -> Option<Font> {
        let e = self.faces.get(face.font.0 as usize)?;
        if e.wght_axis.is_some() {
            Some(
                e.font
                    .instance_builder()
                    .variations([(WGHT, f32::from(face.weight))])
                    .build(),
            )
        } else {
            Some(e.font.clone())
        }
    }
}

/// Sort key of a static face for the CSS Fonts 4 §5.2 weight matching:
/// the smallest key is the best match.
fn css_weight_key(desired: u16, w: u16) -> (u8, u16) {
    let (d, w) = (i32::from(desired), i32::from(w));
    let key = if w == d {
        (0, 0)
    } else if d < 400 {
        // Lighter first (closest), then heavier (closest).
        if w < d { (1, d - w) } else { (2, w - d) }
    } else if d > 500 {
        // Heavier first (closest), then lighter (closest).
        if w > d { (1, w - d) } else { (2, d - w) }
    } else if d < 500 {
        // 400..500: weights up to 500 ascending, then lighter descending, then heavier ascending.
        if w > d && w <= 500 {
            (1, w - d)
        } else if w < d {
            (2, d - w)
        } else {
            (3, w - 500)
        }
    } else {
        // 500: 400 first, then lighter descending, then heavier ascending.
        if (400..500).contains(&w) {
            (1, 500 - w)
        } else if w < 400 {
            (2, 400 - w)
        } else {
            (3, w - 500)
        }
    };
    (key.0, key.1 as u16)
}

/// Index of the best static weight in `available` for `desired` (CSS Fonts 4 §5.2).
pub fn match_weight(desired: u16, available: &[u16]) -> Option<usize> {
    (0..available.len()).min_by_key(|&i| css_weight_key(desired, available[i]))
}

/// Typographic family (ID 16), else family (ID 1); English record preferred.
fn family_name(face: &FontRef<'_>) -> Option<String> {
    [NameId::TYPOGRAPHIC_FAMILY_NAME, NameId::FAMILY_NAME]
        .into_iter()
        .find_map(|id| {
            let s: String = face
                .localized_strings(id)
                .english_or_first()?
                .chars()
                .collect();
            (!s.is_empty()).then_some(s)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(desired: u16, avail: &[u16]) -> u16 {
        avail[match_weight(desired, avail).unwrap()]
    }

    #[test]
    fn css_weight_matching() {
        let all = [100, 300, 400, 500, 700, 900];
        assert_eq!(pick(400, &all), 400);
        assert_eq!(pick(500, &all), 500);
        // Exact match wins; otherwise the rules below.
        assert_eq!(pick(400, &[300, 500, 700]), 500);
        assert_eq!(pick(500, &[300, 400, 700]), 400);
        assert_eq!(pick(500, &[300, 700]), 300);
        assert_eq!(pick(450, &[300, 500, 700]), 500);
        assert_eq!(pick(450, &[300, 700]), 300);
        assert_eq!(pick(300, &[100, 400, 700]), 100);
        assert_eq!(pick(300, &[400, 700]), 400);
        assert_eq!(pick(600, &[300, 400, 700, 900]), 700);
        assert_eq!(pick(600, &[300, 400, 500]), 500);
        assert_eq!(pick(800, &[400, 700]), 700);
        assert_eq!(pick(200, &[300, 400]), 300);
        assert_eq!(match_weight(400, &[]), None);
    }
}
