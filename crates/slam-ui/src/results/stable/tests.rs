use super::*;
use crate::results::{ResultsMods, ResultsStat};

fn sprite(w: f32, h: f32, density: f32) -> StableSprite {
    StableSprite {
        texture: TextureId(1),
        uv: UvRect::FULL,
        size: [w, h],
        density,
    }
}

#[allow(clippy::field_reassign_with_default, clippy::needless_range_loop)]
fn skin() -> StableSkin {
    let mut images: StableImages<Option<StableSprite>> = StableImages::default();
    images.panel = Some(sprite(622.0, 505.0, 1.0));
    images.title = Some(sprite(300.0, 75.0, 1.0));
    images.max_combo = Some(sprite(162.0, 50.0, 1.0));
    images.accuracy = Some(sprite(192.0, 58.0, 1.0));
    images.perfect = Some(sprite(478.0, 150.0, 1.0));
    for g in &mut images.grades {
        *g = Some(sprite(369.0, 422.0, 1.0));
    }
    for h in &mut images.hits {
        *h = Some(sprite(128.0, 128.0, 1.0));
    }
    for font in [&mut images.score_font, &mut images.combo_font] {
        for d in 0..10 {
            font[d] = Some(sprite(40.0, 50.0, 1.0));
        }
    }
    images.score_font[Glyph::Dot.index()] = Some(sprite(10.0, 50.0, 1.0));
    images.score_font[Glyph::Percent.index()] = Some(sprite(60.0, 50.0, 1.0));
    images.combo_font[Glyph::X.index()] = Some(sprite(30.0, 50.0, 1.0));
    for m in &mut images.mods {
        *m = Some(sprite(68.0, 66.0, 1.0));
    }
    StableSkin {
        modern: false,
        score_overlap: 2.0,
        combo_overlap: 0.0,
        images,
    }
}

fn data() -> ResultsData {
    let mut stats = [ResultsStat::default(); StatKind::COUNT];
    stats[StatKind::Great.index()].value = 7;
    ResultsData {
        id: 1,
        title: String::new(),
        score: 123,
        accuracy: 0.9567,
        rank: Rank::S,
        stats,
        combo: 120,
        max_combo: 120,
        mods: ResultsMods::default(),
        average_hit_error: None,
        average_hit_error_text: None,
        unstable_rate: None,
        extra: crate::results::ResultsExtra::default(),
    }
}

type Placed = (Part, [f32; 2], [f32; 2]);

fn placed(skin: &StableSkin, data: &ResultsData, w: f32, h: f32) -> (Vec<Placed>, StableInfo) {
    let mut out = Vec::new();
    let info = lay_out(skin, data, w, h, 0.0, |item| {
        if let Item::Sprite {
            part, centre, size, ..
        } = item
        {
            out.push((part, centre, size));
        }
    });
    (out, info)
}

fn of(items: &[Placed], part: Part) -> Vec<Placed> {
    items.iter().copied().filter(|p| p.0 == part).collect()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn grades_pick_their_image_and_f_shows_d() {
    let cases = [
        (Rank::XH, "ranking-XH"),
        (Rank::SH, "ranking-SH"),
        (Rank::X, "ranking-X"),
        (Rank::S, "ranking-S"),
        (Rank::A, "ranking-A"),
        (Rank::B, "ranking-B"),
        (Rank::C, "ranking-C"),
        (Rank::D, "ranking-D"),
        (Rank::F, "ranking-D"),
    ];
    for (rank, name) in cases {
        assert_eq!(grade_name(rank), name);
    }
    // Every letter has its own slot.
    let mut seen = [false; GRADES];
    for rank in [
        Rank::XH,
        Rank::SH,
        Rank::X,
        Rank::S,
        Rank::A,
        Rank::B,
        Rank::C,
        Rank::D,
    ] {
        seen[grade_index(rank)] = true;
    }
    assert!(seen.iter().all(|s| *s));
}

#[test]
fn metrics_fit_the_design_as_4_by_3() {
    let m = Metrics::new(640.0, 480.0);
    assert!(near(m.ui(100.0), 100.0));
    assert!(near(m.image_scale([622.0, 505.0], 317.0), 317.0 / 505.0));
    // 16:9: the 4:3 area is 1440x1080.
    let wide = Metrics::new(1920.0, 1080.0);
    assert!(near(wide.ui(100.0), 225.0));
    // A narrow window is fitted by its width.
    let tall = Metrics::new(640.0, 1000.0);
    assert!(near(tall.ui(100.0), 100.0));
}

#[test]
fn layout_at_640_by_480() {
    let skin = skin();
    let (items, info) = placed(&skin, &data(), 640.0, 480.0);
    // Title: scale max(75/300, 75/75) = 1, so 300x75; the bar is 0.785 of its height.
    assert!(near(info.bar_height, 75.0 * 0.785));
    assert!(near(info.title_left, 640.0 - 300.0 - 20.0));
    let panel = of(&items, Part::Panel)[0];
    let scale = 317.0 / 505.0;
    assert!(near(panel.2[0], 622.0 * scale) && near(panel.2[1], 505.0 * scale));
    // The panel's top-left is (0, bar - 1).
    assert!(near(panel.1[0] - panel.2[0] / 2.0, 0.0));
    assert!(near(panel.1[1] - panel.2[1] / 2.0, info.bar_height - 1.0));
    // The letter's centre is 120 from the right edge (the container is 2 px wider).
    let grade = of(&items, Part::Grade)[0];
    assert!(near(grade.1[0], 642.0 - 120.0) && near(grade.1[1], 170.0));
    // The first hit icon sits at (40, 99) on the panel; 128 px at 42 osu px.
    let hit = of(&items, Part::Hit(0))[0];
    assert!(near(hit.1[0], 40.0) && near(hit.1[1], info.bar_height - 1.0 + 99.0));
    assert!(near(hit.2[0], 42.0 / 128.0 * 128.0));
    // The miss icon is in the second column, third row.
    let miss = of(&items, Part::Hit(5))[0];
    assert!(near(miss.1[0], 240.0) && near(miss.1[1], info.bar_height - 1.0 + 99.0 + 120.0));
    // Mods are absent without any, the perfect banner is there for the full combo.
    assert!(items.iter().all(|p| !matches!(p.0, Part::Mod(_))));
    assert_eq!(of(&items, Part::Perfect).len(), 1);
}

#[test]
fn layout_at_16_by_9_uses_the_clipped_area_for_sizes_and_the_window_for_the_edge() {
    let skin = skin();
    let (items, info) = placed(&skin, &data(), 1920.0, 1080.0);
    assert!(near(info.unit, 2.25));
    let grade = of(&items, Part::Grade)[0];
    // The right edge is the window's, the offset is scaled by the 4:3 area.
    assert!(near(grade.1[0], 1922.0 - 120.0 * 2.25));
    assert!(near(grade.1[1], 170.0 * 2.25));
    let hit = of(&items, Part::Hit(0))[0];
    assert!(near(hit.1[0], 40.0 * 2.25));
    // The panel starts at the window's left, not the area's.
    let panel = of(&items, Part::Panel)[0];
    assert!(near(panel.1[0] - panel.2[0] / 2.0, 0.0));
}

#[test]
fn score_digits_advance_by_the_width_of_zero_less_the_overlap() {
    let skin = skin();
    let (items, info) = placed(&skin, &data(), 640.0, 480.0);
    let digits = of(&items, Part::Score(Glyph::Digit(1)));
    // "123" then the accuracy: the score's `1` is the first glyph.
    let one = digits[0];
    // Scale: max(20/40, 20/50) = 0.5, times 1.05 for a version 1 skin.
    let scale = 0.5 * 1.05;
    assert!(near(one.2[0], 40.0 * scale));
    assert!(near(one.1[0], 111.0 + 40.0 * scale / 2.0));
    let two = of(&items, Part::Score(Glyph::Digit(2)))[0];
    let advance = 40.0 * scale - 2.0 * scale;
    assert!(near(two.1[0] - one.1[0], advance));
    // The centre is half a digit under the line's top.
    assert!(near(
        one.1[1],
        info.bar_height - 1.0 + 25.0 * scale + 11.0 + -1.0
    ));
}

#[test]
fn accuracy_reads_int_dot_two_digits_and_percent() {
    let skin = skin();
    let (items, _) = placed(&skin, &data(), 640.0, 480.0);
    let tail: Vec<Part> = items
        .iter()
        .map(|p| p.0)
        .filter(|p| matches!(p, Part::Score(_)))
        .skip(3)
        .collect();
    use Glyph::*;
    // 95.67: "95", dot, "67", percent.
    assert_eq!(
        tail,
        [
            Part::Score(Digit(9)),
            Part::Score(Digit(5)),
            Part::Score(Dot),
            Part::Score(Digit(6)),
            Part::Score(Digit(7)),
            Part::Score(Percent),
        ]
    );
    assert_eq!(accuracy_parts(99.999), (99, 99));
    assert_eq!(accuracy_parts(5.0), (5, 0));
    // A single digit gets a leading zero, as stable draws it.
    let mut one = data();
    one.accuracy = 0.05;
    let (items, _) = placed(&skin, &one, 640.0, 480.0);
    let digits = of(&items, Part::Score(Glyph::Digit(0)));
    assert!(digits.len() >= 3);
}

#[test]
fn missing_symbols_and_images_are_skipped() {
    let mut skin = skin();
    skin.images.combo_font[Glyph::X.index()] = None;
    skin.images.score_font[Glyph::Dot.index()] = None;
    skin.images.score_font[Glyph::Percent.index()] = None;
    skin.images.max_combo = None;
    skin.images.hits[3] = None;
    let (items, _) = placed(&skin, &data(), 640.0, 480.0);
    assert!(of(&items, Part::Combo(Glyph::X)).is_empty());
    assert!(of(&items, Part::Score(Glyph::Dot)).is_empty());
    assert!(of(&items, Part::Score(Glyph::Percent)).is_empty());
    assert!(of(&items, Part::MaxComboLabel).is_empty());
    assert!(of(&items, Part::Hit(3)).is_empty());
    // The digits are still there.
    assert!(!of(&items, Part::Combo(Glyph::Digit(7))).is_empty());
    // Without the score font's zero nothing numeric is drawn, nothing panics.
    skin.images.score_font[0] = None;
    let (items, _) = placed(&skin, &data(), 640.0, 480.0);
    assert!(items.iter().all(|p| !matches!(p.0, Part::Combo(_))));
}

#[test]
fn no_panel_places_no_panel() {
    let mut skin = skin();
    skin.images.panel = None;
    let (items, _) = placed(&skin, &data(), 640.0, 480.0);
    assert!(of(&items, Part::Panel).is_empty());
}

#[test]
fn perfect_only_on_a_full_combo() {
    let skin = skin();
    let mut broken = data();
    broken.combo = 100;
    let (items, _) = placed(&skin, &broken, 640.0, 480.0);
    assert!(of(&items, Part::Perfect).is_empty());
    assert!(!broken.full_combo());
    assert!(data().full_combo());
}

#[test]
fn mods_draw_right_to_left_in_stables_order() {
    let skin = skin();
    let mut with = data();
    with.mods = ResultsMods::new(["DT", "HD", "SD", "PF", "NC"]);
    let icons: Vec<usize> = mod_icons(&with).collect();
    let names: Vec<&str> = icons.iter().map(|i| MODS[*i].0).collect();
    // PF hides SD, NC hides DT.
    assert_eq!(
        names,
        [
            "selection-mod-perfect",
            "selection-mod-hidden",
            "selection-mod-nightcore"
        ]
    );
    let (items, _) = placed(&skin, &with, 640.0, 480.0);
    let xs: Vec<f32> = items
        .iter()
        .filter(|p| matches!(p.0, Part::Mod(_)))
        .map(|p| p.1[0])
        .collect();
    assert_eq!(xs.len(), 3);
    assert!(xs[0] > xs[1] && xs[1] > xs[2]);
    // Icons are 20 osu pixels apart.
    assert!(near(xs[0] - xs[1], 20.0));
}

#[test]
fn modern_skins_move_elements() {
    let mut skin = skin();
    skin.modern = true;
    let (items, _) = placed(&skin, &data(), 640.0, 480.0);
    let grade = of(&items, Part::Grade)[0];
    assert!(near(grade.1[1], 200.0));
}

#[test]
fn double_density_halves_the_drawn_size_of_hit_icons() {
    let mut skin = skin();
    skin.images.hits[0] = Some(sprite(256.0, 256.0, 2.0));
    let (items, _) = placed(&skin, &data(), 640.0, 480.0);
    let hit = of(&items, Part::Hit(0))[0];
    let normal = of(&items, Part::Hit(1))[0];
    assert!(near(hit.2[0], normal.2[0]));
}

#[test]
fn images_map_and_iterate_every_slot() {
    let images: StableImages<u32> = StableImages::default();
    assert_eq!(
        images.iter().count(),
        5 + 3 + GRADES + HITS + 2 * GLYPHS + MOD_COUNT
    );
    let mut n = 0;
    let numbered = images.map(|_| {
        n += 1;
        n
    });
    assert_eq!(numbered.iter().count(), n as usize);
    assert!(numbered.iter().all(|v| *v > 0));
    assert!(MOD_COUNT as u32 + (12 + 11) + 5 + 3 + (GRADES + HITS) as u32 <= 64);
}
