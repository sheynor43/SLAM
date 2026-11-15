//! A live input step, and the rules' update on it, do not allocate.

mod common;

use common::{Linear, beatmap, layout, ns, play};
use slam_osu::ModSet;
use slam_osu::judgement::JudgementLog;
use slam_osu::replay::Rules;
use slam_osu::rules::OsuRules;
use slam_play::input::{MAX_LIVE_STEP_EVENTS, MAX_PENDING_EVENTS};
use slam_play::{Bindings, LiveInput};
use slam_testkit::assert_no_alloc;

slam_testkit::install_counting_allocator!();

#[test]
fn live_steps_do_not_allocate() {
    let map = beatmap();
    let play = play(&map, 3);
    let mut live = LiveInput::new(Bindings::default(), layout());
    let mut rules = OsuRules::new(&map, &ModSet::default());
    let mut log = JudgementLog::with_capacity(rules.max_judgements());

    // Ticks every 4 ms, so a tick takes several events.
    let end = play.events.last().unwrap().time_ns;
    let mut next = 0;
    assert_no_alloc(|| {
        let mut now = 0;
        while now <= end + ns(4.0) {
            let start = next;
            while next < play.events.len() && play.events[next].time_ns <= now {
                next += 1;
            }
            let step = live.step(&play.events[start..next], now, &Linear);
            rules.update(&step, &mut log);
            now += ns(4.0);
        }
    });
    assert!(rules.is_complete());
    assert!(!log.events().is_empty());
}

#[test]
fn deferring_and_dropping_do_not_allocate() {
    let map = beatmap();
    let play = play(&map, 5);
    let mut live = LiveInput::new(Bindings::default(), layout());
    let buttons: Vec<_> = play
        .events
        .iter()
        .filter(|e| !matches!(e.kind, slam_input::InputKind::MouseMove { .. }))
        .copied()
        .collect();
    assert!(buttons.len() > MAX_LIVE_STEP_EVENTS);

    // Everything in one tick: some events wait, then a second huge tick overflows the queue.
    assert_no_alloc(|| {
        live.step(&buttons, ns(1e6), &Linear);
        for _ in 0..(MAX_PENDING_EVENTS / buttons.len() + 2) {
            live.step(&buttons, ns(1e6), &Linear);
        }
    });
    assert!(live.dropped() > 0);
}

#[test]
fn service_elements_do_not_allocate() {
    use slam_play::service::{
        COUNTDOWN_STEPS, MAX_SERVICE_SPRITES, ServiceTextures, ServiceTracker, ServiceView,
    };
    let osu = "osu file format v14\n\n[General]\nCountdown: 1\n\n[Events]\n2,1500,10500\n\n\
               [TimingPoints]\n0,500,4,1,0,100,1,0\n\n[HitObjects]\n256,192,4000,1,0\n\
               256,192,14000,1,0\n256,192,15000,1,0\n";
    let map = slam_osu::beatmap::Beatmap::decode(osu.as_bytes()).unwrap();
    let mut tracker = ServiceTracker::new(&map, 0.0);
    let view = ServiceView::new(1920.0, 1080.0);
    let sprite = slam_play::circles::SkinSprite {
        texture: slam_render::textures::TextureId(1),
        uv: slam_render::atlas::UvRect {
            u0: 0.0,
            v0: 0.0,
            u1: 1.0,
            v1: 1.0,
        },
        size: slam_osu::Vec2::new(100.0, 50.0),
    };
    let textures = ServiceTextures {
        countdown: [Some(sprite); COUNTDOWN_STEPS],
        warning_arrow: Some(sprite),
        section_pass: Some(sprite),
        section_fail: Some(sprite),
    };
    let mut batch = slam_render::sprite::SpriteBatch::with_capacity(MAX_SERVICE_SPRITES);
    let mut drawn = 0;
    assert_no_alloc(|| {
        for ms in (0..16_000).step_by(3) {
            let (state, _) = tracker.update(f64::from(ms), 0.7, false, None);
            batch.clear();
            view.push(&mut batch, &state, f64::from(ms) + 1.5, &textures);
            drawn |= batch.len();
        }
    });
    assert!(drawn > 0);
}

#[test]
fn the_break_countdown_does_not_allocate() {
    use slam_osu::breaks::Period;
    use slam_play::break_countdown::{BreakCountdownView, MAX_BREAK_GLYPHS, MAX_BREAK_SHAPES};
    use slam_play::service::{BreakDisplay, ServiceState};
    use std::sync::Arc;

    let registry = Arc::new(slam_text::FontRegistry::with_builtin());
    let limits = slam_render::Limits {
        max_texture_size: 4096,
        max_uniform_block_size: 16384,
        max_uniform_buffer_slots: 8,
        max_texture_slots: 8,
    };
    let mut glyphs = slam_text::glyph_atlas(
        Arc::clone(&registry),
        &slam_text::GlyphAtlasConfig::new(limits),
    )
    .unwrap();
    let mut view = BreakCountdownView::new(Arc::clone(&registry));
    view.request_glyphs(&mut glyphs, 1920.0, 1080.0);
    let white = slam_render::atlas::AtlasRegion {
        texture: slam_render::textures::TextureId(1),
        origin: slam_render::Origin { x: 0, y: 0 },
        size: slam_render::Extent {
            width: 4,
            height: 4,
        },
        uv: slam_render::atlas::UvRect::FULL,
    };
    let mut shapes = slam_render::shape::ShapeBatch::with_capacity(MAX_BREAK_SHAPES);
    let mut text = slam_render::sprite::SpriteBatch::with_capacity(MAX_BREAK_GLYPHS);
    let period = Period {
        start: 1_000.0,
        end: 20_000.0,
    };
    for display in [BreakDisplay::Lazer, BreakDisplay::Slam] {
        let state = ServiceState {
            break_period: Some(period),
            break_display: display,
            ..ServiceState::default()
        };
        // The first frame lays out the first number.
        view.update(1920.0, 1080.0, &state, 1_000.0);
        let mut drawn = 0;
        assert_no_alloc(|| {
            for ms in (1_000..20_300).step_by(5) {
                let time = f64::from(ms);
                shapes.clear();
                text.clear();
                view.update(1920.0, 1080.0, &state, time);
                view.push_shapes(&mut shapes, &state, time, white, Default::default(), true);
                view.push_text(&mut text, &state, time, &mut glyphs, Default::default());
                drawn |= shapes.len();
            }
        });
        assert!(drawn > 0);
    }
}
