use skrifa::{FontRef, MetadataProvider};
use slam_text::{
    DISTANCE_RANGE, EM_PX, FontId, FontRegistry, GlyphBitmap, GlyphError, GlyphKey, PADDING,
    UI_FAMILY, generate_glyph,
};

fn key(reg: &FontRegistry, weight: u16, c: char) -> GlyphKey {
    let face = reg.resolve(reg.family(UI_FAMILY).unwrap(), weight).unwrap();
    let (data, index) = reg.face_data(face.font).unwrap();
    let font = FontRef::from_index(data, index).unwrap();
    let gid = font.charmap().map(c).unwrap();
    GlyphKey {
        font: face.font,
        weight,
        glyph_id: gid.to_u32() as u16,
    }
}

fn median(b: &GlyphBitmap, x: u32, y: u32) -> u8 {
    let w = b.image.size.width;
    let i = ((y * w + x) * 4) as usize;
    let mut c = [
        b.image.pixels[i],
        b.image.pixels[i + 1],
        b.image.pixels[i + 2],
    ];
    c.sort_unstable();
    c[1]
}

fn bright_count(b: &GlyphBitmap) -> usize {
    let Extent { width, height } = b.image.size;
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| median(b, x, y) > 127)
        .count()
}

use slam_render::Extent;

#[test]
fn ring_glyph_has_hole_and_correct_bounds() {
    let reg = FontRegistry::with_builtin();
    let k = key(&reg, 400, 'O');
    let b = generate_glyph(&reg, k).unwrap().unwrap();
    let Extent { width, height } = b.image.size;
    assert_eq!(b.image.pixels.len(), (width * height * 4) as usize);
    assert!(b.image.pixels.chunks(4).all(|p| p[3] == 255));

    // Centre is the hole, the left ring stroke is inside, corners are outside.
    assert!(median(&b, width / 2, height / 2) < 128);
    let ring = (0..width).any(|x| median(&b, x, height / 2) > 127);
    assert!(ring);
    for (x, y) in [
        (0, 0),
        (width - 1, 0),
        (0, height - 1),
        (width - 1, height - 1),
    ] {
        assert!(median(&b, x, y) < 128);
    }

    // Bounds from the unscaled outline.
    let (data, index) = reg.face_data(k.font).unwrap();
    let font = FontRef::from_index(data, index).unwrap();
    let loc = font.axes().location([("wght", 400.0)]);
    let upem = font
        .metrics(skrifa::instance::Size::unscaled(), &loc)
        .units_per_em as f32;
    let g = font.glyph_metrics(skrifa::instance::Size::unscaled(), &loc);
    let bb = g
        .bounds(skrifa::GlyphId::new(u32::from(k.glyph_id)))
        .unwrap();
    let s = EM_PX / upem;
    let pad = PADDING as f32;
    let w = (bb.x_max - bb.x_min) * s + 2.0 * pad;
    let h = (bb.y_max - bb.y_min) * s + 2.0 * pad;
    assert!((width as f32 - w).abs() <= 1.5, "{width} vs {w}");
    assert!((height as f32 - h).abs() <= 1.5, "{height} vs {h}");
    assert!((b.offset[0] - (bb.x_min * s - pad)).abs() <= 1.5);
    assert!((b.offset[1] + (bb.y_max * s + pad)).abs() <= 1.5);
    assert!(b.offset[1] < 0.0);
    assert_eq!(b.offset[0].fract(), 0.0);
    assert_eq!(b.offset[1].fract(), 0.0);
    assert!(DISTANCE_RANGE / 2.0 <= f64::from(PADDING));
}

#[test]
fn space_has_no_bitmap() {
    let reg = FontRegistry::with_builtin();
    assert!(generate_glyph(&reg, key(&reg, 400, ' ')).unwrap().is_none());
}

#[test]
fn weight_changes_stem_width() {
    let reg = FontRegistry::with_builtin();
    let light = generate_glyph(&reg, key(&reg, 200, 'l')).unwrap().unwrap();
    let heavy = generate_glyph(&reg, key(&reg, 900, 'l')).unwrap().unwrap();
    assert_ne!(light.image, heavy.image);
    assert!(heavy.image.size.width > light.image.size.width);
    assert!(bright_count(&heavy) > bright_count(&light));
}

#[test]
fn generation_is_deterministic() {
    let reg = FontRegistry::with_builtin();
    let k = key(&reg, 400, 'g');
    assert_eq!(
        generate_glyph(&reg, k).unwrap(),
        generate_glyph(&reg, k).unwrap()
    );
}

#[test]
fn unknown_font_is_an_error() {
    let reg = FontRegistry::with_builtin();
    let k = GlyphKey {
        font: FontId(9999),
        weight: 400,
        glyph_id: 1,
    };
    assert!(matches!(
        generate_glyph(&reg, k),
        Err(GlyphError::UnknownFont)
    ));
}
