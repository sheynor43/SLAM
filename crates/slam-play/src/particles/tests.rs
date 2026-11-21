use super::*;
use slam_render::atlas::UvRect;
use slam_render::textures::TextureId;

fn layout() -> PlayfieldLayout {
    // 1024×768: a playfield scale of 1.6.
    PlayfieldLayout::new(1024.0, 768.0)
}

fn star_sprite() -> SkinSprite {
    SkinSprite {
        texture: TextureId(7),
        uv: UvRect::FULL,
        size: Vec2::new(32.0, 32.0),
    }
}

fn frame(time: f64) -> ParticleFrame {
    ParticleFrame {
        time,
        cursor: Some(Vec2::new(256.0, 192.0)),
        break_time: false,
        kiai_tracking: false,
        left: false,
        right: false,
        restarts: 0,
    }
}

/// Runs `particles` from `from` to `to` ms a frame each ms with `make`'s frames.
fn run(particles: &mut CursorParticles, from: i32, to: i32, make: impl Fn(f64) -> ParticleFrame) {
    for t in from..=to {
        particles.update(&make(f64::from(t)), &layout());
    }
}

#[test]
fn a_break_spews_twenty_a_second_and_they_fade_within_a_second() {
    let mut p = CursorParticles::default();
    run(&mut p, 0, 250, |t| ParticleFrame {
        break_time: true,
        ..frame(t)
    });
    // One at the start, then one every 50 ms (each lives at least 300 ms).
    assert_eq!(p.alive(250.0), 6);
    // Out of the break nothing new spawns; everything is gone a second later.
    run(&mut p, 251, 1400, frame);
    assert_eq!(p.alive(1400.0), 0);
}

#[test]
fn kiai_spews_sixty_a_second_only_while_tracking() {
    let mut p = CursorParticles::default();
    run(&mut p, 0, 200, frame);
    assert_eq!(p.alive(200.0), 0);
    run(&mut p, 201, 300, |t| ParticleFrame {
        kiai_tracking: true,
        ..frame(t)
    });
    // Spawning starts at once, then every 16.67 ms: 201, 217.7, … 284.3.
    assert_eq!(p.alive(300.0), 6);
}

#[test]
fn no_cursor_no_particles() {
    let mut p = CursorParticles::default();
    run(&mut p, 0, 300, |t| ParticleFrame {
        break_time: true,
        cursor: None,
        ..frame(t)
    });
    assert_eq!(p.alive(300.0), 0);
}

#[test]
fn the_buttons_held_pick_the_side_the_break_particles_fly_to() {
    let centre_x = layout().to_window(Vec2::new(256.0, 192.0)).x;
    let xs = |left: bool, right: bool| {
        let mut p = CursorParticles::default();
        run(&mut p, 0, 400, |t| ParticleFrame {
            break_time: true,
            left,
            right,
            ..frame(t)
        });
        let mut batch = SpriteBatch::new();
        let sprite = star_sprite();
        p.push(
            &mut batch,
            &layout(),
            400.0,
            &Star::new(&sprite, DEFAULT_STAR_COLOUR),
        );
        let xs: Vec<f32> = batch
            .vertices()
            .chunks(4)
            .map(|q| q.iter().map(|v| v.position[0]).sum::<f32>() / 4.0 - centre_x)
            .collect();
        assert!(!xs.is_empty());
        xs
    };
    assert!(xs(true, false).iter().all(|&x| x <= 1e-3));
    assert!(xs(false, true).iter().all(|&x| x >= -1e-3));
    // Neither: they only fall, on the cursor's column.
    assert!(xs(false, false).iter().all(|&x| x.abs() < 1e-3));
    let both = xs(true, true);
    assert!(both.iter().any(|&x| x < -1.0) && both.iter().any(|&x| x > 1.0));
}

#[test]
fn particles_are_additive_star_coloured_and_sized_by_the_magic_factor() {
    let mut p = CursorParticles::default();
    run(&mut p, 0, 0, |t| ParticleFrame {
        break_time: true,
        ..frame(t)
    });
    let mut batch = SpriteBatch::new();
    let sprite = star_sprite();
    let colour = Color {
        r: 0.5,
        g: 0.25,
        b: 1.0,
        a: 1.0,
    };
    p.push(&mut batch, &layout(), 0.0, &Star::new(&sprite, colour));
    let [b] = batch.batches() else {
        panic!("{:?}", batch.batches())
    };
    assert_eq!(b.blend, Blend::Additive);
    let quad = &batch.vertices()[..4];
    // At its birth: scale 1, 32 / 1.6 osu!pixels across at the playfield's 1.6.
    let (min, max) = quad.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
        (lo.min(v.position[0]), hi.max(v.position[0]))
    });
    let side = 32.0 / STABLE_MAGIC_SCALE_FACTOR * 1.6;
    // Rotated by its start angle, the quad's horizontal extent is between its side and its
    // diagonal.
    assert!(max - min >= side - 1e-3 && max - min <= side * 2f32.sqrt() + 1e-3);
    assert_eq!(quad[0].colour[3], 1.0);
}

#[test]
fn falling_particles_follow_lazers_formulas() {
    let p = Particle {
        start_time: 0.0,
        start_position: Vec2::new(10.0, 20.0),
        velocity: Vec2::new(100.0, -50.0),
        duration: 500.0,
        start_angle: -1.0,
        end_angle: 1.0,
        end_scale: 2.0,
    };
    // Halfway through its life (250 of 500 ms).
    assert!((p.alpha(250.0) - 0.5).abs() < 1e-6);
    assert!((p.scale(250.0) - 1.5).abs() < 1e-6);
    assert!(p.angle(250.0).abs() < 1e-6);
    // Gravity 240 × 500 / 1000 × 0.5 = 60 added to the vertical velocity; times 250 / 1000.
    let at = p.position(250.0);
    assert!((at.x - (10.0 + 100.0 * 0.25)).abs() < 1e-4);
    assert!((at.y - (20.0 + (-50.0 + 60.0) * 0.25)).abs() < 1e-4);
    // Past its life it is transparent.
    assert_eq!(p.alpha(600.0), 0.0);
}

#[test]
fn a_seek_back_hides_particles_from_the_future_and_a_restart_clears_them() {
    let mut p = CursorParticles::default();
    run(&mut p, 1000, 1200, |t| ParticleFrame {
        break_time: true,
        ..frame(t)
    });
    assert!(p.alive(1200.0) > 0);
    assert_eq!(p.alive(900.0), 0);
    p.update(
        &ParticleFrame {
            restarts: 1,
            ..frame(1200.0)
        },
        &layout(),
    );
    assert_eq!(p.alive(1200.0), 0);
}

#[test]
fn the_cursor_velocity_is_measured_over_more_than_15_ms() {
    let mut s = Spewer::<4>::new(60);
    s.set_active(true);
    s.cursor_moved(Vec2::new(0.0, 0.0), 0.0);
    for i in 1..=16 {
        s.cursor_moved(Vec2::new(i as f32 * 2.0, 0.0), 1.0);
    }
    // 32 pixels over 16 ms.
    assert_eq!(s.velocity, Vec2::new(2.0, 0.0));
}

#[test]
fn rng_stays_in_range() {
    let mut rng = Rng(1);
    for _ in 0..10_000 {
        let v = rng.next();
        assert!((0.0..1.0).contains(&v));
        let w = rng.between(-460.0, 0.0);
        assert!((-460.0..=0.0).contains(&w));
    }
}

#[test]
fn a_positive_angle_turns_the_particle_counter_clockwise_on_screen() {
    let mut p = CursorParticles::default();
    run(&mut p, 0, 0, |t| ParticleFrame {
        break_time: true,
        ..frame(t)
    });
    // Force a known angle on the only particle.
    p.break_spewer.particles[0].start_angle = 0.5;
    p.break_spewer.particles[0].end_angle = 0.5;
    let mut batch = SpriteBatch::new();
    let sprite = star_sprite();
    p.push(
        &mut batch,
        &layout(),
        0.0,
        &Star::new(&sprite, DEFAULT_STAR_COLOUR),
    );
    // Lazer's quad: (1, 0) of the centre goes to (cos, −sin), up on screen.
    assert_eq!(batch.batches().len(), 1);
    let quad = &batch.vertices()[..4];
    let centre = [
        quad.iter().map(|v| v.position[0]).sum::<f32>() / 4.0,
        quad.iter().map(|v| v.position[1]).sum::<f32>() / 4.0,
    ];
    // Its corners sit at ±45° and ±135°; turned 0.5 rad (28.6°) counter-clockwise, the
    // rightmost one is at −16.4°: below the centre on screen (clockwise, it would be above).
    let right = quad
        .iter()
        .max_by(|a, b| a.position[0].total_cmp(&b.position[0]))
        .unwrap();
    assert!(right.position[1] > centre[1], "{right:?} {centre:?}");
}
