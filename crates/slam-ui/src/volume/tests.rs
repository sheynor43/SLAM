use super::*;

const MS: u64 = 1_000_000;
/// Far enough apart that the acceleration resets between changes.
const APART: u64 = 200 * MS;

fn levels() -> Levels {
    [0.5; METERS]
}

fn shown(levels: &mut Levels) -> VolumeMixer {
    let mut m = VolumeMixer::new();
    assert!(!m.adjust(levels, 1.0, false, MS));
    m
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn the_first_adjust_only_shows_the_overlay_on_the_master_meter() {
    let mut l = levels();
    let mut m = VolumeMixer::new();
    assert!(!m.visible());
    assert!(!m.adjust(&mut l, 1.0, false, MS));
    assert!(m.visible());
    assert_eq!(m.selected(), Meter::Master);
    assert_eq!(l, levels());
}

#[test]
fn a_notch_steps_one_percent_and_quick_notches_accelerate() {
    let mut l = levels();
    let mut m = shown(&mut l);
    let master = Meter::Master.index();
    assert!(m.adjust(&mut l, 1.0, false, APART));
    assert!(close(l[master], 0.51));
    // 1, 1.8, 3.24, 5.832 → capped at 5: steps of 0.01, 0.02, 0.03, 0.05, 0.05.
    let mut now = 2 * APART;
    let mut expected = 0.51;
    for step in [0.01, 0.02, 0.03, 0.05, 0.05] {
        m.adjust(&mut l, 1.0, false, now);
        expected += step;
        assert!(close(l[master], expected), "{} vs {expected}", l[master]);
        now += 50 * MS;
    }
    // A pause resets it.
    m.adjust(&mut l, -1.0, false, now + APART);
    assert!(close(l[master], expected - 0.01));
}

#[test]
fn several_notches_at_once_step_further() {
    let mut l = levels();
    let mut m = shown(&mut l);
    m.adjust(&mut l, -3.0, false, APART);
    assert!(close(l[Meter::Master.index()], 0.47));
}

#[test]
fn precise_scrolling_gathers_fractions() {
    let mut l = levels();
    let mut m = shown(&mut l);
    let master = Meter::Master.index();
    assert!(!m.adjust(&mut l, 0.4, true, APART));
    assert!(!m.adjust(&mut l, 0.4, true, 2 * APART));
    assert!(close(l[master], 0.5));
    assert!(m.adjust(&mut l, 0.4, true, 3 * APART));
    assert!(close(l[master], 0.51));
}

#[test]
fn levels_stay_between_0_and_1() {
    let mut l = [0.99, 0.01, 1.0, 0.0];
    let mut m = shown(&mut l);
    let master = Meter::Master.index();
    assert!(!m.adjust(&mut l, 5.0, false, APART));
    assert_eq!(l[master], 1.0);
    for _ in 0..200 {
        m.adjust(&mut l, -5.0, false, 3 * APART);
    }
    assert_eq!(l[master], 0.0);
    assert!(!m.adjust(&mut l, -1.0, false, 4 * APART));
    assert_eq!(l[master], 0.0);
}

#[test]
fn hotkeys_cycle_the_meters_only_while_shown() {
    let mut l = levels();
    let mut m = VolumeMixer::new();
    assert!(!m.step_meter(true, MS));
    m.show(MS);
    assert!(m.step_meter(true, MS));
    assert_eq!(m.selected(), Meter::Music);
    assert!(m.step_meter(true, MS));
    assert_eq!(m.selected(), Meter::Hitsounds);
    assert!(m.step_meter(false, MS));
    assert_eq!(m.selected(), Meter::Music);
    m.adjust(&mut l, 1.0, false, APART);
    assert!(close(l[Meter::Music.index()], 0.51));
    assert!(close(l[Meter::Master.index()], 0.5));
}

#[test]
fn the_pointer_selects_a_meter_and_the_wheel_over_it_changes_it() {
    let theme = Theme::default();
    let layout = VolumeLayout::new(Edge::Left, MeterStyle::Arc, 1366.0, 768.0, &theme);
    let mut l = levels();
    let mut m = VolumeMixer::new();
    let effects = layout.meters[Meter::Effects.index()].card.centre();
    // Hidden, the wheel over its place does nothing.
    assert!(!m.wheel_over(&mut l, 1.0, false, effects[0], effects[1], &layout, MS));
    m.show(MS);
    m.pointer(effects[0], effects[1], &layout, MS);
    assert_eq!(m.selected(), Meter::Effects);
    let hitsounds = layout.meters[Meter::Hitsounds.index()].card.centre();
    assert!(m.wheel_over(
        &mut l,
        1.0,
        false,
        hitsounds[0],
        hitsounds[1],
        &layout,
        APART
    ));
    assert_eq!(m.selected(), Meter::Hitsounds);
    assert!(close(l[Meter::Hitsounds.index()], 0.51));
    // Away from the cards the wheel is not taken.
    assert!(!m.wheel_over(&mut l, 1.0, false, 900.0, 400.0, &layout, APART));
}

#[test]
fn it_hides_after_a_second_but_not_under_the_pointer() {
    let theme = Theme::default();
    let layout = VolumeLayout::new(Edge::Left, MeterStyle::Arc, 1366.0, 768.0, &theme);
    let mut m = VolumeMixer::new();
    m.show(MS);
    m.tick(MS + 999 * MS);
    assert!(m.visible());
    let master = layout.meters[Meter::Master.index()].card.centre();
    m.pointer(master[0], master[1], &layout, 500 * MS);
    m.tick(10_000 * MS);
    assert!(m.visible());
    // Leaving starts the wait again.
    m.pointer(900.0, 400.0, &layout, 10_000 * MS);
    m.tick(10_500 * MS);
    assert!(m.visible());
    m.tick(11_000 * MS);
    assert!(!m.visible());
}

#[test]
fn a_change_is_saved_once_when_the_overlay_hides() {
    let mut l = levels();
    let mut m = shown(&mut l);
    m.adjust(&mut l, 1.0, false, APART);
    m.adjust(&mut l, 1.0, false, 2 * APART);
    assert!(!m.take_save());
    m.tick(2 * APART + HIDE_AFTER_NS);
    assert!(!m.visible());
    assert!(m.take_save());
    assert!(!m.take_save());
    // Showing and hiding without a change saves nothing.
    m.show(10_000 * MS);
    m.tick(20_000 * MS);
    assert!(!m.take_save());
}

#[test]
fn unsaved_changes_are_found_before_the_overlay_hides() {
    let mut l = levels();
    let mut m = shown(&mut l);
    assert!(!m.take_unsaved());
    m.adjust(&mut l, 1.0, false, APART);
    assert!(m.take_unsaved());
    m.tick(APART + HIDE_AFTER_NS);
    assert!(!m.take_save());
}

#[test]
fn mute_shows_the_overlay_and_keeps_the_level() {
    let mut l = levels();
    let mut m = VolumeMixer::new();
    m.toggle_mute(MS);
    assert!(m.muted());
    assert!(m.visible());
    assert_eq!(l, levels());
    m.toggle_mute(2 * MS);
    assert!(!m.muted());
    assert!(!m.adjust(&mut l, 0.0, false, APART));
}

#[test]
fn a_change_sends_sparks_stronger_at_the_ends() {
    let mut l = [0.5, 0.5, 0.99, 0.5];
    let mut m = shown(&mut l);
    m.adjust(&mut l, 1.0, false, APART);
    let frame = m.frame(Edge::Left, MeterStyle::Arc, &l);
    let burst = frame.bursts.iter().find(|b| b.at_ns != 0).copied().unwrap();
    assert_eq!(burst.meter, Meter::Master);
    assert_eq!(burst.level, 1.0);
    assert_eq!(burst.strength, 1.0);
    // The ring keeps the last few.
    for k in 0..40 {
        m.adjust(&mut l, -1.0, false, 2 * APART + k * APART);
    }
    let frame = m.frame(Edge::Left, MeterStyle::Arc, &l);
    assert!(frame.bursts.iter().all(|b| b.at_ns != 0));
}

#[test]
fn the_layout_stands_at_its_edge() {
    let theme = Theme::default();
    let (w, h) = (1920.0, 1080.0);
    for style in MeterStyle::ALL {
        for edge in Edge::ALL {
            let layout = VolumeLayout::new(edge, style, w, h, &theme);
            for (i, parts) in layout.meters.iter().enumerate() {
                let card = parts.card;
                assert!(card.x >= 0.0 && card.y >= 0.0, "{style:?} {edge:?} {i}");
                assert!(card.x + card.width <= w && card.y + card.height <= h);
                assert!(layout.bounds.contains(card.centre()[0], card.centre()[1]));
                // The gauge and the texts are inside the card.
                for r in [parts.gauge, parts.value, parts.label] {
                    assert!(
                        r.x >= card.x - 0.01 && r.y >= card.y - 0.01,
                        "{style:?} {i}"
                    );
                    assert!(r.x + r.width <= card.x + card.width + 0.01, "{style:?} {i}");
                    assert!(
                        r.y + r.height <= card.y + card.height + 0.01,
                        "{style:?} {i}"
                    );
                }
            }
            let first = layout.meters[0].card;
            let second = layout.meters[1].card;
            match edge {
                Edge::Left => assert!(first.x < 50.0 && second.y > first.y),
                Edge::Right => assert!(first.x + first.width > w - 50.0 && second.y > first.y),
                Edge::Top => assert!(first.y < 50.0 && second.x > first.x),
                Edge::Bottom => assert!(first.y + first.height > h - 50.0 && second.x > first.x),
            }
            // The master meter is the largest.
            let master = layout.meters[Meter::Master.index()].card;
            assert!(master.width > first.width);
        }
    }
}

#[test]
fn style_names_round_trip() {
    for style in MeterStyle::ALL {
        assert_eq!(MeterStyle::from_name(style.name()), Some(style));
    }
    assert_eq!(MeterStyle::from_name("dots"), None);
}

#[test]
fn edge_names_round_trip() {
    for edge in Edge::ALL {
        assert_eq!(Edge::from_name(edge.name()), Some(edge));
    }
    assert_eq!(Edge::from_name("middle"), None);
}

#[test]
fn the_view_draws_nothing_once_hidden_and_stays_in_its_budget() {
    use slam_render::atlas::UvRect;
    use slam_render::hal::Origin;
    use slam_render::textures::TextureId;
    use slam_text::{GlyphAtlasConfig, glyph_atlas};
    let registry = Arc::new(FontRegistry::with_builtin());
    let labels = VolumeLabels {
        meters: ["HITSOUNDS", "EFFECTS", "MASTER", "MUSIC"].map(String::from),
        max: "MAX".into(),
        muted: "MUTED".into(),
    };
    let limits = slam_render::Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 12,
        max_texture_slots: 16,
    };
    let mut glyphs = glyph_atlas(Arc::clone(&registry), &GlyphAtlasConfig::new(limits)).unwrap();
    let mut view = VolumeView::new(registry, Theme::default(), &labels);
    let white = AtlasRegion {
        texture: TextureId(0),
        origin: Origin::new(0, 0),
        size: slam_render::Extent::new(1, 1),
        uv: UvRect::FULL,
    };
    let mut shapes = ShapeBatch::with_capacity(MAX_VOLUME_SHAPES);
    let mut text = SpriteBatch::with_capacity(MAX_VOLUME_GLYPHS);
    let mut l = [1.0, 0.0, 0.99, 0.5];
    let mut m = shown(&mut l);
    // Many bursts at once, all alive.
    for k in 0..MAX_BURSTS as u64 {
        m.adjust(
            &mut l,
            if k % 2 == 0 { 1.0 } else { -1.0 },
            false,
            APART + k * MS,
        );
    }
    let now = APART + 20 * MS;
    let frame = m.frame(Edge::Bottom, MeterStyle::Arc, &l);
    for style in MeterStyle::ALL {
        shapes.clear();
        text.clear();
        let frame = VolumeFrame { style, ..frame };
        view.push(
            &mut shapes,
            &mut text,
            &mut glyphs,
            &frame,
            1920.0,
            1080.0,
            now,
            white,
        );
        assert!(
            shapes.len() <= MAX_VOLUME_SHAPES,
            "{style:?} {}",
            shapes.len()
        );
    }
    shapes.clear();
    text.clear();
    view.push(
        &mut shapes,
        &mut text,
        &mut glyphs,
        &frame,
        1920.0,
        1080.0,
        now,
        white,
    );
    assert!(shapes.len() > METERS * 4);
    assert!(shapes.len() <= MAX_VOLUME_SHAPES, "{}", shapes.len());
    let hidden = VolumeFrame {
        visible: false,
        since_ns: now,
        ..frame
    };
    shapes.clear();
    text.clear();
    view.push(
        &mut shapes,
        &mut text,
        &mut glyphs,
        &hidden,
        1920.0,
        1080.0,
        now + 1000 * MS,
        white,
    );
    assert_eq!(shapes.len(), 0);
    assert_eq!(text.len(), 0);
}

#[test]
fn levels_are_exact_hundredths() {
    let mut l = [0.5; METERS];
    let mut m = shown(&mut l);
    let master = Meter::Master.index();
    for k in 0..20 {
        m.adjust(&mut l, 1.0, false, APART * (k + 1));
        let hundredths = (l[master] * 100.0).round();
        assert_eq!(l[master], hundredths / 100.0, "{}", l[master]);
    }
    assert_eq!(l[master], 0.7);
}

#[test]
fn each_meter_accelerates_on_its_own() {
    let mut l = levels();
    let mut m = shown(&mut l);
    // Two quick notches on the master: the next one there would step 3 %.
    m.adjust(&mut l, 1.0, false, APART);
    m.adjust(&mut l, 1.0, false, APART + 50 * MS);
    // Another meter right after starts at 1 %, as each lazer meter has its own.
    m.step_meter(true, APART + 60 * MS);
    m.adjust(&mut l, 1.0, false, APART + 70 * MS);
    assert!(close(l[Meter::Music.index()], 0.51));
}

#[test]
fn a_resting_pointer_keeps_the_overlay_up_without_selecting() {
    let theme = Theme::default();
    let layout = VolumeLayout::new(Edge::Left, MeterStyle::Arc, 1366.0, 768.0, &theme);
    let mut m = VolumeMixer::new();
    m.show(MS);
    m.step_meter(true, MS);
    let [x, y] = layout.meters[Meter::Effects.index()].card.centre();
    // Before the pointer ever moved, its place is unknown: no hover.
    m.hover(x, y, &layout, 2 * MS);
    m.tick(1500 * MS);
    assert!(!m.visible());
    m.show(2000 * MS);
    m.step_meter(true, 2000 * MS);
    m.pointer(900.0, 400.0, &layout, 2000 * MS);
    m.hover(x, y, &layout, 2000 * MS);
    assert_eq!(m.selected(), Meter::Music);
    m.tick(7000 * MS);
    assert!(m.visible());
    m.hover(900.0, 400.0, &layout, 7000 * MS);
    m.tick(7500 * MS);
    assert!(m.visible());
    m.tick(8000 * MS);
    assert!(!m.visible());
}
