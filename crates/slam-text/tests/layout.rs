use slam_testkit::{count_allocs, install_counting_allocator};
use slam_text::{
    Align, FontError, FontRegistry, HEADING_FAMILY, Layout, LayoutContext, TextStyle, UI_FAMILY,
    match_weight,
};

install_counting_allocator!();

fn setup() -> (FontRegistry, TextStyle) {
    let reg = FontRegistry::with_builtin();
    let ui = reg.family(UI_FAMILY).unwrap();
    (reg, TextStyle::new(ui, 400, 20.0))
}

fn lay(reg: &FontRegistry, text: &str, style: &TextStyle) -> Layout {
    LayoutContext::new().layout(reg, text, style)
}

fn advance(reg: &FontRegistry, style: &TextStyle, c: char) -> f32 {
    let r = reg.resolve(style.family, style.weight).unwrap();
    let font = reg.instance(r).unwrap();
    let gid = font.charmap().map_unicode(c).unwrap();
    let upem = font.units_per_em();
    let shaper = harfrust::ShaperFont::new(&font).with_scale(i32::from(upem));
    shaper.glyph_h_advance(gid) as f32 * style.size / f32::from(upem)
}

fn line_texts<'a>(l: &Layout, text: &'a str) -> Vec<&'a str> {
    l.lines().iter().map(|ln| &text[ln.text.clone()]).collect()
}

#[test]
fn builtin_families_resolve() {
    let reg = FontRegistry::with_builtin();
    assert!(reg.family(UI_FAMILY).is_some());
    assert!(reg.family("exo 2").is_some());
    assert_eq!(reg.family(HEADING_FAMILY), reg.family("EXO 2"));
    assert!(reg.family("Nope").is_none());
    assert_eq!(reg.fallback(), &[reg.family(UI_FAMILY).unwrap()]);
    assert!(FontRegistry::new().family(UI_FAMILY).is_none());
}

#[test]
fn latin_and_cyrillic_have_glyphs() {
    let (reg, style) = setup();
    let l = lay(&reg, "Hello Привет", &style);
    assert_eq!(l.glyphs().len(), 12);
    assert!(l.glyphs().iter().all(|g| g.glyph_id != 0));
    let ui = reg.resolve(style.family, 400).unwrap().font;
    assert!(l.glyphs().iter().all(|g| g.font == ui));
}

#[test]
fn variable_weight_changes_advances_and_clamps() {
    let (reg, mut style) = setup();
    let w400 = lay(&reg, "Hello world", &style).width();
    style.weight = 700;
    let l700 = lay(&reg, "Hello world", &style);
    assert!(l700.width() > w400 + 1.0, "{} vs {w400}", l700.width());
    assert_eq!(l700.glyphs()[0].weight, 700);
    // Nunito Sans wght axis is 200..=1000, Exo 2 is 100..=900.
    assert_eq!(reg.resolve(style.family, 50).unwrap().weight, 200);
    assert_eq!(reg.resolve(style.family, 2000).unwrap().weight, 1000);
    let exo = reg.family(HEADING_FAMILY).unwrap();
    assert_eq!(reg.resolve(exo, 50).unwrap().weight, 100);
    assert_eq!(reg.resolve(exo, 1000).unwrap().weight, 900);
}

#[test]
fn width_is_sum_of_advances_and_kerning_applies() {
    let (reg, style) = setup();
    let l = lay(&reg, "llll", &style);
    let expected = 4.0 * advance(&reg, &style, 'l');
    assert!((l.width() - expected).abs() < 1e-3);
    assert!((l.lines()[0].width - expected).abs() < 1e-3);

    let av = lay(&reg, "AV", &style).width();
    let sum = advance(&reg, &style, 'A') + advance(&reg, &style, 'V');
    assert!(av < sum - 0.1, "AV {av} should be kerned below {sum}");
}

#[test]
fn vertical_metrics() {
    let (reg, mut style) = setup();
    let one = lay(&reg, "a", &style);
    let two = lay(&reg, "a\nb", &style);
    let adv = one.height();
    assert!(adv > style.size * 0.9 && adv < style.size * 2.0);
    assert!((two.height() - 2.0 * adv).abs() < 1e-3);
    assert!((two.lines()[1].baseline - two.lines()[0].baseline - adv).abs() < 1e-3);
    assert!(one.lines()[0].baseline > 0.0 && one.lines()[0].baseline < adv);
    assert_eq!(one.glyphs()[0].y, one.lines()[0].baseline);

    style.line_height = 2.0;
    let tall = lay(&reg, "a", &style);
    assert!((tall.height() - 2.0 * adv).abs() < 1e-3);
    // Half-leading: baseline moves down by half of the extra leading.
    let shift = tall.lines()[0].baseline - one.lines()[0].baseline;
    assert!((shift - adv * 0.5).abs() < 1e-3);
}

#[test]
fn wraps_at_spaces_and_trims_trailing_space() {
    let (reg, mut style) = setup();
    let text = "aaaa aaaa aaaa";
    let word = lay(&reg, "aaaa", &style).width();
    let space = advance(&reg, &style, ' ');
    style.max_width = Some(word * 2.0 + space + 1.0);
    let l = lay(&reg, text, &style);
    assert_eq!(line_texts(&l, text), ["aaaa aaaa ", "aaaa"]);
    let first = &l.lines()[0];
    assert!((first.width - (word * 2.0 + space)).abs() < 0.5);
    // Trailing space is part of the glyphs but not of the width.
    assert_eq!(first.glyphs.len(), 10);
    assert!(l.width() <= style.max_width.unwrap());

    style.max_width = Some(word + 0.5);
    let l = lay(&reg, text, &style);
    assert_eq!(l.lines().len(), 3);
    assert!(l.lines().iter().all(|ln| ln.width <= word + 0.5));
}

#[test]
fn long_word_breaks_by_clusters() {
    let (reg, mut style) = setup();
    let text = "abcdefghijkl";
    let full = lay(&reg, text, &style).width();
    style.max_width = Some(full / 3.0);
    let l = lay(&reg, text, &style);
    assert!(l.lines().len() >= 3);
    let joined: String = line_texts(&l, text).concat();
    assert_eq!(joined, text);
    assert!(l.lines().iter().all(|ln| ln.width <= full / 3.0 + 1e-3));
    assert_eq!(l.glyphs().len(), text.len());
}

#[test]
fn zero_and_tiny_max_width_terminate() {
    let (reg, mut style) = setup();
    for w in [0.0, 0.001, 1.0] {
        style.max_width = Some(w);
        let l = lay(&reg, "abc def", &style);
        let glyphs: usize = l.lines().iter().map(|ln| ln.glyphs.len()).sum();
        assert_eq!(glyphs, l.glyphs().len());
        assert_eq!(l.glyphs().len(), 7);
        assert!(l.lines().len() >= 3 && l.lines().len() <= 7);
    }
}

#[test]
fn newlines_empty_and_trailing() {
    let (reg, style) = setup();
    let empty = lay(&reg, "", &style);
    assert_eq!(empty.lines().len(), 1);
    assert!(empty.glyphs().is_empty());
    assert!(empty.height() > 0.0);
    assert_eq!(empty.width(), 0.0);

    for text in ["a\nb", "a\r\nb", "a\u{2028}b"] {
        let l = lay(&reg, text, &style);
        assert_eq!(l.lines().len(), 2, "{text:?}");
        assert_eq!(l.glyphs().len(), 2, "{text:?}");
        assert_eq!(line_texts(&l, text), ["a", "b"]);
    }

    let t = lay(&reg, "a\n", &style);
    assert_eq!(t.lines().len(), 2);
    assert_eq!(t.glyphs().len(), 1);
    assert!(t.lines()[1].glyphs.is_empty());

    let blank = lay(&reg, "a\n\nb", &style);
    assert_eq!(blank.lines().len(), 3);
    assert!(blank.lines()[1].glyphs.is_empty());
    assert_eq!(lay(&reg, "\r\n", &style).lines().len(), 2);
}

#[test]
fn alignment_offsets() {
    let (reg, mut style) = setup();
    let text = "aaaa\naa";
    style.max_width = Some(200.0);
    let left = lay(&reg, text, &style);
    style.align = Align::Center;
    let center = lay(&reg, text, &style);
    style.align = Align::Right;
    let right = lay(&reg, text, &style);
    for (i, ln) in left.lines().iter().enumerate() {
        let first = ln.glyphs.start;
        assert_eq!(left.glyphs()[first].x, 0.0);
        let free = 200.0 - ln.width;
        assert!((center.glyphs()[first].x - free / 2.0).abs() < 1e-3);
        assert!((right.glyphs()[first].x - free).abs() < 1e-3);
        assert_eq!(left.lines()[i].width, center.lines()[i].width);
    }
    // Without max_width the widest line is the reference.
    style.max_width = None;
    style.align = Align::Right;
    let r = lay(&reg, text, &style);
    let widest = r.lines()[0].width;
    assert!((r.glyphs()[r.lines()[0].glyphs.start].x).abs() < 1e-3);
    let short = &r.lines()[1];
    assert!((r.glyphs()[short.glyphs.start].x - (widest - short.width)).abs() < 1e-3);
}

#[test]
fn fallback_uses_second_family_and_notdef_from_primary() {
    let mut reg = FontRegistry::with_builtin();
    let ui = reg.family(UI_FAMILY).unwrap();
    let exo = reg.family(HEADING_FAMILY).unwrap();
    let ui_font = reg.resolve(ui, 400).unwrap().font;
    let exo_font = reg.resolve(exo, 400).unwrap().font;
    // U+0181 is only in Nunito Sans, U+04A6 only in Exo 2.
    reg.set_fallback(&[ui]);
    let style = TextStyle::new(exo, 400, 20.0);
    let l = lay(&reg, "a\u{181}b", &style);
    let fonts: Vec<_> = l.glyphs().iter().map(|g| g.font).collect();
    assert_eq!(fonts, [exo_font, ui_font, exo_font]);
    assert!(l.glyphs().iter().all(|g| g.glyph_id != 0));

    reg.set_fallback(&[exo]);
    let style = TextStyle::new(ui, 400, 20.0);
    let l = lay(&reg, "a\u{4A6}b", &style);
    assert_eq!(l.glyphs()[1].font, exo_font);

    // Missing everywhere: notdef from the primary font.
    let l = lay(&reg, "a\u{4E2D}b", &style);
    assert_eq!(l.glyphs()[1].font, ui_font);
    assert_eq!(l.glyphs()[1].glyph_id, 0);

    // Spaces stay in the current run.
    reg.set_fallback(&[ui]);
    let style = TextStyle::new(exo, 400, 20.0);
    let l = lay(&reg, "\u{181} \u{181}", &style);
    assert!(l.glyphs().iter().all(|g| g.font == ui_font));
}

#[test]
fn garbage_bytes_are_errors() {
    let mut reg = FontRegistry::new();
    assert_eq!(reg.add_font(Vec::new()), Err(FontError::InvalidFont));
    assert_eq!(reg.add_font(vec![0u8; 100]), Err(FontError::InvalidFont));
    let mut x = 12345u32;
    let junk: Vec<u8> = (0..4096)
        .map(|_| {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 24) as u8
        })
        .collect();
    assert!(reg.add_font(junk).is_err());
    // Truncated real font.
    let full = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/fonts/exo2/Exo2-VariableFont.ttf"
    ))
    .unwrap();
    assert!(reg.add_font(full[..200].to_vec()).is_err());
    assert!(reg.family(HEADING_FAMILY).is_none());
    // A valid font added at runtime is found by its name table family.
    let ids = reg.add_font(full).unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(reg.family_name(reg.family("exo 2").unwrap()), Some("Exo 2"));
}

#[test]
fn weight_matching_rules() {
    assert_eq!(match_weight(400, &[300, 500, 700]), Some(1));
    assert_eq!(match_weight(500, &[300, 400, 700]), Some(1));
    assert_eq!(match_weight(300, &[400, 700]), Some(0));
    assert_eq!(match_weight(600, &[300, 400, 500]), Some(2));
    assert_eq!(match_weight(400, &[]), None);
}

#[test]
fn warm_relayout_does_not_allocate() {
    let (reg, mut style) = setup();
    let mut ctx = LayoutContext::new();
    let mut out = Layout::new();
    style.max_width = Some(120.0);
    let long = "The quick brown fox jumps over the lazy dog. Привет, мир!\nSecond line AV";
    // The first use of each (font, script) pair builds a shape plan; warm up
    // the scripts used below (Latin, Cyrillic, digits).
    for text in [long, "12:34 1234"] {
        ctx.layout_into(&reg, text, &style, &mut out);
        ctx.layout_into(&reg, text, &style, &mut out);
    }

    let (_, stats) = count_allocs(|| ctx.layout_into(&reg, long, &style, &mut out));
    assert_eq!(stats.total(), 0, "same text: {stats}");
    let (_, stats) = count_allocs(|| ctx.layout_into(&reg, "The quick brown", &style, &mut out));
    assert_eq!(stats.total(), 0, "shorter text: {stats}");
    let (_, stats) = count_allocs(|| ctx.layout_into(&reg, "12:34 1234", &style, &mut out));
    assert_eq!(stats.total(), 0, "digits: {stats}");
}
