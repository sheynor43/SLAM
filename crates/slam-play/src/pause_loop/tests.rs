use super::*;
use slam_audio::PlayError;

const MS: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Command {
    Play { voice: u64, gain: f32 },
    Gain { voice: u64, gain: f32, ramp_ms: f64 },
    Stop { voice: u64 },
}

#[derive(Default)]
struct Sink {
    plays: u64,
    commands: Vec<Command>,
    full: bool,
}

impl VoiceSink for Sink {
    fn play_voice(
        &mut self,
        _sample: &Arc<Sample>,
        params: VoiceParams,
    ) -> Result<VoiceId, PlayError> {
        if self.full {
            return Err(PlayError::QueueFull);
        }
        assert!(params.looping);
        assert_eq!(params.group, VoiceGroup::Effects);
        let voice = self.plays;
        self.plays += 1;
        self.commands.push(Command::Play {
            voice,
            gain: params.gain,
        });
        Ok(VoiceId::from_play(voice))
    }

    fn stop_voice(&mut self, voice: VoiceId) -> Result<(), PlayError> {
        if self.full {
            return Err(PlayError::QueueFull);
        }
        self.commands.push(Command::Stop { voice: id(voice) });
        Ok(())
    }

    fn set_voice_balance(&mut self, _: VoiceId, _: f32) -> Result<(), PlayError> {
        unreachable!()
    }

    fn set_voice_gain(&mut self, voice: VoiceId, gain: f32, ramp_ms: f64) -> Result<(), PlayError> {
        if self.full {
            return Err(PlayError::QueueFull);
        }
        self.commands.push(Command::Gain {
            voice: id(voice),
            gain,
            ramp_ms,
        });
        Ok(())
    }

    fn set_voice_rate(&mut self, _: VoiceId, _: f64) -> Result<(), PlayError> {
        unreachable!()
    }
}

fn id(voice: VoiceId) -> u64 {
    (0..64)
        .find(|&n| VoiceId::from_play(n) == voice)
        .expect("a voice the sink started")
}

fn sample() -> Option<Arc<Sample>> {
    Some(Arc::new(
        Sample::from_interleaved(vec![0.0; 2], 44_100, 2).unwrap(),
    ))
}

/// The last gain command sent.
fn last_gain(sink: &Sink) -> f32 {
    sink.commands
        .iter()
        .rev()
        .find_map(|c| match *c {
            Command::Gain { gain, .. } => Some(gain),
            _ => None,
        })
        .expect("a gain command")
}

/// Updates every millisecond from `from_ms` to `to_ms` (inclusive) after `start_ns`.
fn run(pause: &mut PauseLoop, sink: &mut Sink, start_ns: u64, from_ms: u64, to_ms: u64) {
    for ms in from_ms..=to_ms {
        pause.update(start_ns + ms * MS, sink);
    }
}

#[test]
fn showing_starts_the_loop_silent_and_fades_it_in_along_in_quint() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink::default();
    let start = 1_000 * MS;
    pause.show(start, &mut sink);
    assert_eq!(
        sink.commands[0],
        Command::Play {
            voice: 0,
            gain: 0.0
        }
    );
    // The first piece ramps to the curve's value one step on.
    let first = (STEP_NS as f64 / FADE_NS as f64).powi(5) as f32;
    assert_eq!(
        sink.commands[1],
        Command::Gain {
            voice: 0,
            gain: first,
            ramp_ms: 10.0
        }
    );
    run(&mut pause, &mut sink, start, 1, 99);
    // Halfway along InQuint: (1/2)^5.
    assert!((last_gain(&sink) - 0.5f32.powi(5)).abs() < 1e-6);
    run(&mut pause, &mut sink, start, 100, 300);
    assert_eq!(last_gain(&sink), 1.0);
    // One command per step, then the gain stands.
    let gains = sink
        .commands
        .iter()
        .filter(|c| matches!(c, Command::Gain { .. }))
        .count();
    assert_eq!(gains as u64, FADE_NS / STEP_NS + 1);
    assert!(pause.is_playing());
}

#[test]
fn hiding_fades_out_along_out_quad_then_stops() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    run(&mut pause, &mut sink, 0, 1, 300);
    let hide = 300 * MS;
    pause.hide(hide);
    run(&mut pause, &mut sink, hide, 0, 99);
    // Half way along Out from 1 to 0: 1 - 0.5 * (2 - 0.5).
    assert!((last_gain(&sink) - 0.25).abs() < 1e-6);
    assert!(pause.is_playing());
    run(&mut pause, &mut sink, hide, 100, 199);
    assert!(pause.is_playing());
    pause.update(hide + FADE_NS, &mut sink);
    assert!(!pause.is_playing());
    assert_eq!(sink.commands.last(), Some(&Command::Stop { voice: 0 }));
    // Nothing more after the stop.
    let sent = sink.commands.len();
    run(&mut pause, &mut sink, hide, 201, 400);
    assert_eq!(sink.commands.len(), sent);
}

#[test]
fn showing_again_while_fading_out_restarts_from_the_volume_it_had() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    run(&mut pause, &mut sink, 0, 1, 300);
    pause.hide(300 * MS);
    run(&mut pause, &mut sink, 300 * MS, 0, 100);
    let gain = pause.gain_at(400 * MS);
    pause.show(400 * MS, &mut sink);
    let n = sink.commands.len();
    // Lazer's `SkinnableSound.Play` stops and plays again.
    assert_eq!(sink.commands[n - 3], Command::Stop { voice: 0 });
    assert_eq!(sink.commands[n - 2], Command::Play { voice: 1, gain });
    run(&mut pause, &mut sink, 400 * MS, 1, 200);
    assert_eq!(last_gain(&sink), 1.0);
    assert!(pause.is_playing());
}

#[test]
fn stop_ends_the_loop_at_once() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    run(&mut pause, &mut sink, 0, 1, 50);
    assert!(pause.stop(&mut sink));
    assert!(!pause.is_playing());
    assert_eq!(sink.commands.last(), Some(&Command::Stop { voice: 0 }));
    // A hide after the stop sends nothing.
    pause.hide(60 * MS);
    let sent = sink.commands.len();
    run(&mut pause, &mut sink, 0, 60, 400);
    assert_eq!(sink.commands.len(), sent);
}

#[test]
fn no_sample_is_silent() {
    let mut pause = PauseLoop::new(None);
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    run(&mut pause, &mut sink, 0, 1, 300);
    pause.hide(300 * MS);
    run(&mut pause, &mut sink, 300 * MS, 0, 300);
    assert!(pause.stop(&mut sink));
    assert!(sink.commands.is_empty());
}

#[test]
fn a_stop_that_does_not_queue_is_tried_again() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    sink.full = true;
    assert!(!pause.stop(&mut sink));
    assert_eq!(pause.failed_commands(), 1);
    sink.full = false;
    pause.update(MS, &mut sink);
    assert_eq!(sink.commands.last(), Some(&Command::Stop { voice: 0 }));
    assert!(pause.stop(&mut sink));
}

#[test]
fn a_gain_step_that_does_not_queue_is_sent_next_update() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    sink.full = true;
    pause.update(10 * MS, &mut sink);
    sink.full = false;
    let sent = sink.commands.len();
    pause.update(11 * MS, &mut sink);
    assert_eq!(sink.commands.len(), sent + 1);
}

#[test]
fn replacing_the_sample_restarts_a_shown_loop() {
    let mut pause = PauseLoop::new(sample());
    let mut other = PauseLoop::new(sample());
    let new = other.sample.clone().unwrap();
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    run(&mut pause, &mut sink, 0, 1, 300);
    assert!(pause.replace_sample(&mut other, &mut sink));
    assert!(Arc::ptr_eq(pause.sample.as_ref().unwrap(), &new));
    assert!(!pause.is_playing());
    pause.update(301 * MS, &mut sink);
    assert!(pause.is_playing());
    assert!(sink.commands.contains(&Command::Stop { voice: 0 }));
    assert!(matches!(
        sink.commands
            .iter()
            .rev()
            .find(|c| matches!(c, Command::Play { .. })),
        Some(Command::Play { voice: 1, .. })
    ));
    // Hidden, the new sample waits for the next show.
    pause.hide(400 * MS);
    run(&mut pause, &mut sink, 400 * MS, 0, 300);
    let plays = sink.plays;
    assert!(pause.replace_sample(&mut other, &mut sink));
    run(&mut pause, &mut sink, 800 * MS, 0, 10);
    assert_eq!(sink.plays, plays);
}

#[test]
fn a_start_that_does_not_queue_is_tried_again() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink {
        full: true,
        ..Sink::default()
    };
    pause.show(0, &mut sink);
    assert!(!pause.is_playing());
    assert_eq!(pause.failed_commands(), 1);
    sink.full = false;
    pause.update(MS, &mut sink);
    assert!(pause.is_playing());
    run(&mut pause, &mut sink, 0, 2, 400);
    assert_eq!(last_gain(&sink), 1.0);
}

#[test]
fn two_stops_that_do_not_queue_are_both_tried_again() {
    let mut pause = PauseLoop::new(sample());
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    // The first loop fades out; its stop fails.
    pause.hide(10 * MS);
    sink.full = true;
    pause.update(10 * MS + FADE_NS, &mut sink);
    sink.full = false;
    // A show starts the second loop; then its stop fails as well.
    pause.show(300 * MS, &mut sink);
    sink.full = true;
    assert!(!pause.stop(&mut sink));
    sink.full = false;
    pause.update(400 * MS, &mut sink);
    let stopped: Vec<u64> = sink
        .commands
        .iter()
        .filter_map(|c| match *c {
            Command::Stop { voice } => Some(voice),
            _ => None,
        })
        .collect();
    assert!(stopped.contains(&0) && stopped.contains(&1), "{stopped:?}");
    assert!(pause.stop(&mut sink));
}

#[test]
fn a_new_sample_under_a_settled_menu_plays_at_full_volume() {
    let mut pause = PauseLoop::new(sample());
    let mut other = PauseLoop::new(sample());
    let mut sink = Sink::default();
    pause.show(0, &mut sink);
    run(&mut pause, &mut sink, 0, 1, 300);
    assert!(pause.replace_sample(&mut other, &mut sink));
    pause.update(301 * MS, &mut sink);
    assert_eq!(
        sink.commands
            .iter()
            .rev()
            .find(|c| matches!(c, Command::Play { .. })),
        Some(&Command::Play {
            voice: 1,
            gain: 1.0
        })
    );
}
