//! Shaping, fallback itemization, line breaking and alignment (ADR-0037).

use std::ops::Range;

use harfrust::{Buffer, Direction, Font, ShapeOptions, ShapePlan, ShaperFont};
use unicode_linebreak::{BreakClass, BreakOpportunity, break_property, linebreaks};

use crate::registry::{FamilyId, FontId, FontRegistry, ResolvedFace};

/// Maximum number of candidate fonts per layout call: the style's family
/// plus the fallback chain; the rest of a longer chain is ignored.
const MAX_FACES: usize = 8;

/// Horizontal alignment of lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// Lines start at x = 0 (or at the container's left edge).
    #[default]
    Left,
    /// Lines are centred in the container.
    Center,
    /// Lines end at the container's right edge.
    Right,
}

/// Parameters of a layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Primary family; the registry's fallback chain follows it.
    pub family: FamilyId,
    /// Requested weight (CSS scale, 100..=900 typical).
    pub weight: u16,
    /// Font size in pixels per em.
    pub size: f32,
    /// Multiplier of the natural line height; 1.0 is the font's own.
    pub line_height: f32,
    /// Horizontal alignment.
    pub align: Align,
    /// Wrap width in pixels; `None` disables wrapping.
    pub max_width: Option<f32>,
}

impl TextStyle {
    /// Left-aligned, unwrapped style with line height 1.0.
    pub fn new(family: FamilyId, weight: u16, size: f32) -> Self {
        Self {
            family,
            weight,
            size,
            line_height: 1.0,
            align: Align::Left,
            max_width: None,
        }
    }
}

/// A glyph placed by the layout. Glyph images are keyed by
/// `(font, weight, glyph_id)`; the pixel size is not part of the key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositionedGlyph {
    /// Face the glyph comes from.
    pub font: FontId,
    /// Effective weight of the face (see [`ResolvedFace::weight`]).
    pub weight: u16,
    /// Glyph index in the face.
    pub glyph_id: u16,
    /// Pen x of the glyph origin in pixels, relative to the layout's left edge.
    pub x: f32,
    /// Baseline y in pixels, y down, layout top = 0.
    pub y: f32,
    /// Byte offset in the source text of the cluster this glyph belongs to.
    pub cluster: u32,
}

/// One line of a [`Layout`].
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Range into [`Layout::glyphs`].
    pub glyphs: Range<usize>,
    /// Range of source bytes covered by the line (without the paragraph terminator).
    pub text: Range<usize>,
    /// Width in pixels without trailing whitespace.
    pub width: f32,
    /// Baseline y in pixels.
    pub baseline: f32,
}

/// Result of a layout. Reused between calls to keep its buffers.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    glyphs: Vec<PositionedGlyph>,
    lines: Vec<Line>,
    width: f32,
    height: f32,
    font_size: f32,
}

impl Layout {
    /// Empty layout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Glyphs of all lines in logical order.
    pub fn glyphs(&self) -> &[PositionedGlyph] {
        &self.glyphs
    }

    /// Lines top to bottom. Empty text still gives one (empty) line.
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// Width of the widest line in pixels.
    pub fn width(&self) -> f32 {
        self.width
    }

    /// Total height: the number of lines times the line advance.
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Font size (px per em) the layout was made with.
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    /// Clears glyphs and lines, keeping capacity.
    pub fn clear(&mut self) {
        self.glyphs.clear();
        self.lines.clear();
        self.width = 0.0;
        self.height = 0.0;
        self.font_size = 0.0;
    }
}

#[derive(Clone, Copy)]
struct Shaped {
    font: FontId,
    weight: u16,
    glyph_id: u16,
    cluster: u32,
    advance: f32,
    x_off: f32,
    y_off: f32,
    space: bool,
}

/// A face instance at one weight, cached by the context.
struct CachedFace {
    font: FontId,
    weight: u16,
    face: Font,
}

#[derive(Default)]
struct Scratch {
    plans: Vec<(FontId, u16, Option<harfrust::Script>, ShapePlan)>,
    buffer: Buffer,
    breaks: Vec<(u32, bool)>,
    shaped: Vec<Shaped>,
    prefix: Vec<f32>,
}

/// Reusable shaping state: parsed face and shape plan caches, buffers and
/// scratch vectors. A context may be used with several registries; switching
/// registries drops its caches.
///
/// After a layout, repeating it (or laying out text no longer in bytes and
/// glyphs, with the same fonts and scripts) into the same [`Layout`] does not
/// allocate. The first use of each (font, script) pair builds a shape plan, and
/// the first use of a font parses it; both allocate once.
#[derive(Default)]
pub struct LayoutContext {
    registry_id: u64,
    faces: Vec<CachedFace>,
    scratch: Scratch,
}

struct Slot<'a> {
    resolved: ResolvedFace,
    face: &'a Font,
    shaper: ShaperFont<'a, 'static>,
    charmap: skrifa::raw::model::charmap::Charmap<'a>,
    scale: f32,
}

impl LayoutContext {
    /// New context with empty caches.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lays out `text` and returns a fresh [`Layout`].
    pub fn layout(&mut self, registry: &FontRegistry, text: &str, style: &TextStyle) -> Layout {
        let mut out = Layout::new();
        self.layout_into(registry, text, style, &mut out);
        out
    }

    /// Makes sure the instance for `resolved` is cached.
    fn ensure_face(&mut self, registry: &FontRegistry, resolved: ResolvedFace) -> bool {
        if self
            .faces
            .iter()
            .any(|c| c.font == resolved.font && c.weight == resolved.weight)
        {
            return true;
        }
        let Some(face) = registry.instance(resolved) else {
            return false;
        };
        self.faces.push(CachedFace {
            font: resolved.font,
            weight: resolved.weight,
            face,
        });
        true
    }

    /// Lays out `text` into `out`, replacing its contents.
    ///
    /// Vertical metrics: the line advance is `(ascender - descender + line_gap)
    /// * line_height` of the primary font; the extra leading over the natural
    /// height is split half above and half below the line (CSS half-leading).
    /// Empty text gives one empty line; a trailing newline gives a trailing
    /// empty line. If the style's family is unknown, `out` is left empty.
    pub fn layout_into(
        &mut self,
        registry: &FontRegistry,
        text: &str,
        style: &TextStyle,
        out: &mut Layout,
    ) {
        out.clear();
        out.font_size = style.size;

        if self.registry_id != registry.id() {
            self.registry_id = registry.id();
            self.faces.clear();
            self.scratch.plans.clear();
        }

        // Candidate faces: the style's family, then the fallback chain.
        let Some(primary) = registry.resolve(style.family, style.weight) else {
            return;
        };
        if !self.ensure_face(registry, primary) {
            return;
        }
        let mut resolved: [Option<ResolvedFace>; MAX_FACES] = [None; MAX_FACES];
        resolved[0] = Some(primary);
        let mut n = 1;
        for &family in registry.fallback() {
            if n == MAX_FACES {
                break;
            }
            let Some(r) = registry.resolve(family, style.weight) else {
                continue;
            };
            if resolved[..n].iter().flatten().any(|x| x.font == r.font)
                || !self.ensure_face(registry, r)
            {
                continue;
            }
            resolved[n] = Some(r);
            n += 1;
        }

        let faces = &self.faces;
        let sc = &mut self.scratch;
        let mut slots: [Option<Slot<'_>>; MAX_FACES] = std::array::from_fn(|_| None);
        for (slot, r) in slots.iter_mut().zip(resolved.iter().flatten()) {
            let Some(cached) = faces
                .iter()
                .find(|c| c.font == r.font && c.weight == r.weight)
            else {
                return;
            };
            let face = &cached.face;
            let upem = face.units_per_em();
            *slot = Some(Slot {
                resolved: *r,
                face,
                shaper: ShaperFont::new(face).with_scale(i32::from(upem)),
                charmap: face.charmap(),
                scale: style.size / f32::from(upem),
            });
        }
        let slots = &slots[..n];
        let Some(primary) = slots[0].as_ref() else {
            return;
        };

        let line = primary.face.metrics().h_line().unwrap_or_default();
        let asc = line.ascender.to_f32() * primary.scale;
        let desc = line.descender.to_f32() * primary.scale;
        let gap = line.line_gap.to_f32() * primary.scale;
        let natural = asc - desc + gap;
        let advance = natural * style.line_height;
        let first_baseline = (advance - (asc - desc)) * 0.5 + asc;

        sc.breaks.clear();
        sc.breaks
            .extend(linebreaks(text).map(|(o, k)| (o as u32, k == BreakOpportunity::Mandatory)));

        let container = style.max_width.unwrap_or(f32::INFINITY);
        let mut widest = 0.0f32;
        let mut para_start = 0usize;
        let mut brk = 0usize;
        let mut last_terminated = false;
        let mut paragraphs = 0usize;
        while brk < sc.breaks.len() {
            let (off, mandatory) = sc.breaks[brk];
            brk += 1;
            let off = off as usize;
            if !mandatory || off <= para_start {
                continue;
            }
            let raw = &text[para_start..off];
            let body_len = trim_terminator(raw);
            last_terminated = body_len < raw.len();
            sc.paragraph(
                text,
                para_start..para_start + body_len,
                slots,
                container,
                advance,
                first_baseline,
                out,
                &mut widest,
            );
            paragraphs += 1;
            para_start = off;
        }
        if paragraphs == 0 || last_terminated {
            sc.paragraph(
                text,
                text.len()..text.len(),
                slots,
                container,
                advance,
                first_baseline,
                out,
                &mut widest,
            );
        }

        out.width = widest;
        out.height = out.lines.len() as f32 * advance;
        let reference = if container.is_finite() {
            container
        } else {
            widest
        };
        if style.align != Align::Left {
            for line in &out.lines {
                let free = (reference - line.width).max(0.0);
                let shift = match style.align {
                    Align::Left => 0.0,
                    Align::Center => free * 0.5,
                    Align::Right => free,
                };
                for g in &mut out.glyphs[line.glyphs.clone()] {
                    g.x += shift;
                }
            }
        }
    }
}

impl Scratch {
    #[allow(clippy::too_many_arguments)]
    fn paragraph(
        &mut self,
        text: &str,
        para: Range<usize>,
        slots: &[Option<Slot<'_>>],
        container: f32,
        advance: f32,
        first_baseline: f32,
        out: &mut Layout,
        widest: &mut f32,
    ) {
        self.shaped.clear();
        self.shape_paragraph(text, para.clone(), slots);

        // Prefix sums of advances.
        self.prefix.clear();
        let mut acc = 0.0f32;
        self.prefix.push(0.0);
        for g in &self.shaped {
            acc += g.advance;
            self.prefix.push(acc);
        }
        let n = self.shaped.len();

        // First break opportunity strictly inside the paragraph.
        let mut bi = self
            .breaks
            .partition_point(|&(o, _)| (o as usize) <= para.start);
        let mut ls = 0usize; // line start (glyph index)
        let mut line_text_start = para.start;
        let mut last_ok: Option<usize> = None;

        loop {
            // Next opportunity as a glyph index; the paragraph end is the last one.
            let (cand, is_end) =
                if bi < self.breaks.len() && (self.breaks[bi].0 as usize) < para.end {
                    let o = self.breaks[bi].0;
                    (self.shaped.partition_point(|g| g.cluster < o), false)
                } else {
                    (n, true)
                };
            if cand <= ls && !is_end {
                bi += 1;
                continue;
            }
            let fits = cand <= ls || self.trimmed_width(ls, cand) <= container;
            if fits {
                last_ok = Some(cand);
                if is_end {
                    self.emit_line(
                        ls..cand,
                        line_text_start..para.end,
                        advance,
                        first_baseline,
                        out,
                        widest,
                    );
                    return;
                }
                bi += 1;
                continue;
            }
            // Overfull: break at the last fitting opportunity, else inside the word.
            let end = match last_ok {
                Some(e) if e > ls => e,
                _ => self.forced_break(ls, container),
            };
            let next_start = self
                .shaped
                .get(end)
                .map_or(para.end, |g| g.cluster as usize);
            self.emit_line(
                ls..end,
                line_text_start..next_start,
                advance,
                first_baseline,
                out,
                widest,
            );
            ls = end;
            line_text_start = next_start;
            last_ok = None;
            if ls >= n && is_end {
                return;
            }
        }
    }

    /// Width of glyphs `a..b` without trailing whitespace.
    fn trimmed_width(&self, a: usize, mut b: usize) -> f32 {
        while b > a && self.shaped[b - 1].space {
            b -= 1;
        }
        self.prefix[b] - self.prefix[a]
    }

    /// Largest cluster boundary `j > ls` with a fitting width; at least one cluster.
    fn forced_break(&self, ls: usize, container: f32) -> usize {
        let n = self.shaped.len();
        let mut best = None;
        for j in ls + 1..=n {
            let boundary = j == n || self.shaped[j].cluster != self.shaped[j - 1].cluster;
            if !boundary {
                continue;
            }
            if best.is_some() && self.trimmed_width(ls, j) > container {
                break;
            }
            best = Some(j);
        }
        best.unwrap_or(n)
    }

    fn emit_line(
        &mut self,
        glyphs: Range<usize>,
        text: Range<usize>,
        advance: f32,
        first_baseline: f32,
        out: &mut Layout,
        widest: &mut f32,
    ) {
        let baseline = first_baseline + out.lines.len() as f32 * advance;
        let start = out.glyphs.len();
        let width = self.trimmed_width(glyphs.start, glyphs.end);
        let base = self.prefix[glyphs.start];
        for (i, g) in self.shaped[glyphs.clone()].iter().enumerate() {
            let pen = self.prefix[glyphs.start + i] - base;
            out.glyphs.push(PositionedGlyph {
                font: g.font,
                weight: g.weight,
                glyph_id: g.glyph_id,
                x: pen + g.x_off,
                y: baseline - g.y_off,
                cluster: g.cluster,
            });
        }
        *widest = widest.max(width);
        out.lines.push(Line {
            glyphs: start..out.glyphs.len(),
            text,
            width,
            baseline,
        });
    }

    /// Itemizes a paragraph into font runs and shapes them into `self.shaped`.
    fn shape_paragraph(&mut self, text: &str, para: Range<usize>, slots: &[Option<Slot<'_>>]) {
        let body = &text[para.clone()];
        let mut cur = 0usize;
        let mut run_start = para.start;
        let mut chars = body.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let unit_start = para.start + i;
            let mut unit_end = unit_start + c.len_utf8();
            while let Some(&(j, m)) = chars.peek() {
                if is_combining(m) {
                    unit_end = para.start + j + m.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let idx = if is_neutral(c) {
                cur
            } else {
                let unit = &text[unit_start..unit_end];
                pick_slot(slots, unit)
            };
            if idx != cur {
                if unit_start > run_start {
                    self.shape_run(text, run_start..unit_start, &slots[cur]);
                }
                run_start = unit_start;
                cur = idx;
            }
        }
        if para.end > run_start {
            self.shape_run(text, run_start..para.end, &slots[cur]);
        }
    }

    fn shape_run(&mut self, text: &str, run: Range<usize>, slot: &Option<Slot<'_>>) {
        let Some(slot) = slot else { return };
        let buf = &mut self.buffer;
        buf.clear();
        for (i, c) in text[run.clone()].char_indices() {
            buf.push(c as u32, (run.start + i) as u32);
        }
        buf.set_direction(Direction::LeftToRight);
        buf.guess_segment_properties();
        let script = buf.script();
        let font = slot.resolved.font;
        let weight = slot.resolved.weight;
        let pi = match self
            .plans
            .iter()
            .position(|(f, w, s, _)| *f == font && *w == weight && *s == script)
        {
            Some(p) => p,
            None => {
                let plan = ShapePlan::new(slot.face, Direction::LeftToRight, script, None, &[]);
                self.plans.push((font, weight, script, plan));
                self.plans.len() - 1
            }
        };
        let options = ShapeOptions::new().plan(Some(&self.plans[pi].3));
        if harfrust::shape(&slot.shaper, buf, options).is_err() {
            return;
        }
        for (info, pos) in buf.glyph_infos().iter().zip(buf.glyph_positions()) {
            let cluster = info.cluster;
            let space = text[cluster as usize..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
            self.shaped.push(Shaped {
                font,
                weight,
                glyph_id: info.glyph_id as u16,
                cluster,
                advance: pos.x_advance as f32 * slot.scale,
                x_off: pos.x_offset as f32 * slot.scale,
                y_off: pos.y_offset as f32 * slot.scale,
                space,
            });
        }
    }
}

/// Length of `para` without its trailing mandatory-break character(s).
fn trim_terminator(para: &str) -> usize {
    let mut chars = para.chars().rev();
    let Some(last) = chars.next() else { return 0 };
    if !matches!(
        break_property(last as u32),
        BreakClass::Mandatory
            | BreakClass::CarriageReturn
            | BreakClass::LineFeed
            | BreakClass::NextLine
    ) {
        return para.len();
    }
    let mut len = para.len() - last.len_utf8();
    if last == '\n' && para[..len].ends_with('\r') {
        len -= 1;
    }
    len
}

fn is_combining(c: char) -> bool {
    matches!(
        break_property(c as u32),
        BreakClass::CombiningMark | BreakClass::ZeroWidthJoiner
    ) || matches!(c, '\u{FE00}'..='\u{FE0F}')
}

/// Characters that never trigger a font switch.
fn is_neutral(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{2060}')
}

/// First slot covering every non-ignorable char of `unit`; slot 0 otherwise.
fn pick_slot(slots: &[Option<Slot<'_>>], unit: &str) -> usize {
    'slot: for (i, slot) in slots.iter().enumerate() {
        let Some(slot) = slot else { continue };
        for c in unit.chars() {
            if is_neutral(c) || is_combining(c) && matches!(c, '\u{200D}' | '\u{FE00}'..='\u{FE0F}')
            {
                continue;
            }
            if slot.charmap.map_unicode(c).is_none() {
                continue 'slot;
            }
        }
        return i;
    }
    0
}
