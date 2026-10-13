//! A scripted player: window input events for a map, and the same play as replay frames.

#![allow(dead_code)]

pub mod skin;

use slam_input::{InputEvent, InputKind, MouseButton};
use slam_osu::replay::{Actions, InputFrame, ReplayInput, gameplay_start_time};
use slam_osu::{Beatmap, Vec2};
use slam_play::{GameTime, Input, PlayfieldLayout};

/// Circles, sliders with ticks and repeats, and a spinner.
pub const MAP: &str = "osu file format v14

[Difficulty]
HPDrainRate:6
CircleSize:4
OverallDifficulty:8
ApproachRate:9
SliderMultiplier:1.4
SliderTickRate:2

[TimingPoints]
0,300,4,2,0,100,1,0

[HitObjects]
100,100,1000,1,0
120,120,1300,2,0,P|200:60|300:120,2,250
300,200,2400,1,0
300,200,2600,2,0,L|100:200,1,150
256,192,3200,12,0,4700
256,192,5000,2,0,B|300:100|400:300,3,120
400,300,6400,5,0
";

pub fn beatmap() -> Beatmap {
    Beatmap::decode(MAP.as_bytes()).unwrap()
}

/// 640×480 maps window points to osu!pixels exactly: one point per osu!pixel, origin (64, 56).
pub fn layout() -> PlayfieldLayout {
    PlayfieldLayout::new(640.0, 480.0)
}

/// Engine nanoseconds are game nanoseconds.
pub struct Linear;

impl GameTime for Linear {
    fn game_time(&self, time_ns: u64) -> f64 {
        time_ns as f64 / 1_000_000.0
    }
}

pub fn ns(ms: f64) -> u64 {
    (ms * 1_000_000.0) as u64
}

const INPUTS: [(Input, Actions); 5] = [
    (Input::Key(29), Actions::LEFT),
    (Input::Key(27), Actions::RIGHT),
    (Input::Key(6), Actions::SMOKE),
    (Input::Mouse(MouseButton::Left), Actions::LEFT),
    (Input::Mouse(MouseButton::Right), Actions::RIGHT),
];

/// A scripted play: window events, and the replay frames the same play records.
pub struct Play {
    pub events: Vec<InputEvent>,
    pub frames: ReplayInput,
}

/// A deterministic player that roughly follows the objects with a jittery cursor, spins the
/// spinner and mashes the default bindings with holds of random length. The cursor moves every
/// millisecond and at most one input changes per millisecond, after the move, so that replay
/// frames on the same milliseconds describe the same input.
pub fn play(beatmap: &Beatmap, seed: u64) -> Play {
    let mut state = seed;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as u32
    };

    let layout = layout();
    // Engine time starts at 0: no input before it.
    let start = (gameplay_start_time(beatmap).ceil() as i64).max(0);
    let end = beatmap.hit_objects.last().unwrap().end_time() as i64 + 500;
    let mut held = [false; INPUTS.len()];
    let mut next_change = start + 1;
    let mut events = Vec::new();
    let mut frames = Vec::new();

    for t in 0..=end {
        let time = t as f64;
        let target = target(beatmap, time);
        // Whole osu!pixels, so the window position converts back exactly.
        let position = Vec2::new(
            target.x.round() + (next() % 41) as f32 - 20.0,
            target.y.round() + (next() % 41) as f32 - 20.0,
        );
        let window = layout.to_window(position);
        events.push(InputEvent {
            time_ns: ns(time),
            kind: InputKind::MouseMove {
                x: window.x,
                y: window.y,
            },
        });

        if t == next_change {
            let i = next() as usize % INPUTS.len();
            held[i] = !held[i];
            let down = held[i];
            let kind = match INPUTS[i].0 {
                Input::Key(scancode) => InputKind::Key { scancode, down },
                Input::Mouse(button) => InputKind::MouseButton { button, down },
            };
            events.push(InputEvent {
                time_ns: ns(time),
                kind,
            });
            next_change += 1 + i64::from(next() % if down { 120 } else { 40 });
        }

        let actions = INPUTS
            .iter()
            .zip(held)
            .filter(|&(_, h)| h)
            .fold(Actions::NONE, |acc, (&(_, a), _)| acc | a);
        frames.push(InputFrame {
            time,
            position,
            actions,
        });
    }

    Play {
        events,
        frames: ReplayInput::new(frames),
    }
}

/// Where the player aims at `time`: the current object (sliders straight from head to tail,
/// spinners in circles), else the next one.
fn target(beatmap: &Beatmap, time: f64) -> Vec2 {
    let Some(object) = beatmap
        .hit_objects
        .iter()
        .find(|o| o.end_time() + 50.0 >= time)
    else {
        return Vec2::new(256.0, 192.0);
    };
    if time < object.start_time {
        return object.stacked_position();
    }
    if object.is_spinner() {
        let angle = (time / 25.0) as f32;
        return Vec2::new(256.0 + 80.0 * angle.cos(), 192.0 + 80.0 * angle.sin());
    }
    let duration = object.end_time() - object.start_time;
    let progress = if duration > 0.0 {
        ((time - object.start_time) / duration).min(1.0) as f32
    } else {
        1.0
    };
    let from = object.stacked_position();
    let to = object.stacked_end_position();
    Vec2::new(
        from.x + (to.x - from.x) * progress,
        from.y + (to.y - from.y) * progress,
    )
}
