//! Shared helpers for the replay simulator tests.

#![allow(dead_code)]

use slam_formats::osr::{self, FrameData, GeneratedFrame, frames_from_timeline};
use slam_formats::osu::{Vec2, decode_str};
use slam_osu::Beatmap;
use slam_osu::judgement::{HitResult, Judgement, JudgementLog, ObjectRef};
use slam_osu::replay::{Actions, Rules, StepInput};

/// First object at 3000 ms with AR 9 (preempt 600), so gameplay starts at 1000 ms.
pub const MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:5
CircleSize:4
OverallDifficulty:8
ApproachRate:9
SliderMultiplier:1
SliderTickRate:1

[TimingPoints]
0,500,4,2,0,100,1,0

[HitObjects]
100,100,3000,1,0
200,150,3300,1,0
300,200,3600,1,0
";

pub fn beatmap() -> Beatmap {
    Beatmap::from_file(decode_str(MAP).unwrap().beatmap).unwrap()
}

/// A decoded-looking `.osr` with these absolute-time frames, stored as deltas.
pub fn replay(frames: &[GeneratedFrame], beatmap_format_version: i32) -> osr::Replay {
    osr::Replay {
        frames: FrameData::Frames(frames_from_timeline(frames, beatmap_format_version)),
        ..Default::default()
    }
}

/// A deterministic pseudo-random replay that mashes both buttons around the map's objects.
pub fn noisy_frames(seed: u64) -> Vec<GeneratedFrame> {
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };
    let mut frames = Vec::new();
    let mut time = 0.0;
    while time < 4000.0 {
        frames.push(GeneratedFrame {
            time,
            x: (next() % 512) as f32 + 0.25,
            y: (next() % 384) as f32 + 0.5,
            buttons: (next() % 32) as i32,
        });
        time += f64::from(next() % 40);
    }
    frames
}

/// Minimal test rules: each left or right press within 100 ms of the next object judges it,
/// `Great` inside its radius and `Meh` outside; objects not pressed in time are missed. Never
/// allocates after construction.
pub struct PressRules {
    objects: Vec<(f64, Vec2, f32)>,
    next: usize,
}

impl PressRules {
    pub fn new(beatmap: &Beatmap) -> PressRules {
        PressRules {
            objects: beatmap
                .hit_objects
                .iter()
                .map(|o| {
                    (
                        o.start_time,
                        o.stacked_position(),
                        o.defaults.radius() as f32,
                    )
                })
                .collect(),
            next: 0,
        }
    }

    fn judge(&mut self, time: f64, result: HitResult, log: &mut JudgementLog) {
        log.apply(Judgement {
            object: ObjectRef {
                index: self.next as u32,
                nested: None,
            },
            time,
            result,
            max_result: HitResult::Great,
        });
        self.next += 1;
    }
}

impl Rules for PressRules {
    fn max_judgements(&self) -> usize {
        self.objects.len()
    }

    fn update(&mut self, step: &StepInput<'_>, log: &mut JudgementLog) {
        for e in step.events {
            if !e.pressed || e.action == Actions::SMOKE {
                continue;
            }
            if let Some(&(start, position, radius)) = self.objects.get(self.next)
                && (e.time - start).abs() <= 100.0
            {
                let result = if e.position.distance(position) <= radius {
                    HitResult::Great
                } else {
                    HitResult::Meh
                };
                self.judge(e.time, result, log);
            }
        }
        if let Some(&(start, ..)) = self.objects.get(self.next)
            && step.time > start + 100.0
        {
            self.judge(step.time, HitResult::Miss, log);
        }
    }

    fn is_complete(&self) -> bool {
        self.next == self.objects.len()
    }
}
