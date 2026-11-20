//! The pause and fail menus' looping `pause-loop` sound (ADR-0157): lazer's
//! `GameplayMenuOverlay` plays it while either menu shows, fading in and out with the menu.
//!
//! The mixer moves a voice's gain linearly; the eased fades are followed in steps of
//! [`STEP_NS`], each a linear ramp to the curve's value at the step's end.

use std::sync::Arc;

use slam_audio::{Sample, VoiceGroup, VoiceId, VoiceParams};

use crate::circles::Easing;
use crate::gameplay_sounds::VoiceSink;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (TRANSITION_DURATION)
/// How long the loop fades in and out, in nanoseconds.
const FADE_NS: u64 = 200_000_000;

/// Length of one linear piece of a fade, in nanoseconds.
pub const STEP_NS: u64 = 10_000_000;

const NS_PER_MS: f64 = 1_000_000.0;

/// Stops that did not queue kept to be tried again: one for the loop fading out and one
/// for the loop a show replaced in the same full queue.
const PENDING_STOPS: usize = 2;

/// Start of the loop: silent, under the effects volume, as lazer's `Volume = { Value = 0 }`.
const START: VoiceParams = VoiceParams {
    gain: 0.0,
    balance: 0.0,
    rate: 1.0,
    looping: true,
    group: VoiceGroup::Effects,
};

/// The gain moving from one value to another over [`FADE_NS`], as a framework `VolumeTo`
/// started from the value at the time.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fade {
    from: f32,
    to: f32,
    start_ns: u64,
    easing: Easing,
}

impl Fade {
    const SILENT: Fade = Fade {
        from: 0.0,
        to: 0.0,
        start_ns: 0,
        easing: Easing::None,
    };

    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Transforms/Transform.cs (Apply)
    fn value_at(&self, now_ns: u64) -> f32 {
        let elapsed = now_ns.saturating_sub(self.start_ns);
        if elapsed >= FADE_NS {
            return self.to;
        }
        let t = self.easing.apply(elapsed as f64 / FADE_NS as f64);
        self.from + (self.to - self.from) * t as f32
    }

    fn end_ns(&self) -> u64 {
        self.start_ns.saturating_add(FADE_NS)
    }
}

/// The `pause-loop` voice of a play. Call [`PauseLoop::update`] every update tick; nothing
/// here allocates.
///
/// Not `Clone`: it owns the looping voice it started.
#[derive(Debug)]
pub struct PauseLoop {
    /// The skin's `pause-loop`; silent without one.
    sample: Option<Arc<Sample>>,
    /// The loop playing, or [`VoiceId::NONE`].
    voice: VoiceId,
    fade: Fade,
    /// Whether a menu shows: the loop fades to full volume rather than out.
    shown: bool,
    /// When the next piece of the fade is due; `u64::MAX` when the gain stands.
    next_step_ns: u64,
    /// Stops that did not queue ([`VoiceId::NONE`] when free), tried again before every
    /// command here.
    pending_stops: [VoiceId; PENDING_STOPS],
    /// The loop should play but has no voice (its start did not queue, or the sample
    /// changed): the next update starts it at the volume `fade` gives.
    restart: bool,
    /// Mixer commands that failed.
    failed: u64,
}

impl PauseLoop {
    /// The loop of `sample` ([`hitsounds::effect`](crate::hitsounds::effect) of
    /// `Gameplay/pause-loop`); silent without one.
    pub fn new(sample: Option<Arc<Sample>>) -> Self {
        PauseLoop {
            sample,
            voice: VoiceId::NONE,
            fade: Fade::SILENT,
            shown: false,
            next_step_ns: u64::MAX,
            pending_stops: [VoiceId::NONE; PENDING_STOPS],
            restart: false,
            failed: 0,
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (PopIn, startPauseLoop)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/SkinnableSound.cs (Play)
    /// A menu shows at `now_ns`: the loop starts over from its first frame at the volume it
    /// had and fades in.
    pub fn show(&mut self, now_ns: u64, sink: &mut impl VoiceSink) {
        self.retry_stops(sink);
        // A loop starting again keeps the volume it would have had, as lazer's sound keeps
        // its `Volume` over a skin change.
        let gain = if std::mem::take(&mut self.restart) {
            self.fade.value_at(now_ns)
        } else {
            self.gain_at(now_ns)
        };
        self.shown = true;
        self.fade = Fade {
            from: gain,
            to: 1.0,
            start_ns: now_ns,
            easing: Easing::InQuint,
        };
        let old = std::mem::replace(&mut self.voice, VoiceId::NONE);
        self.stop_voice(old, sink);
        let Some(sample) = &self.sample else { return };
        match sink.play_voice(sample, VoiceParams { gain, ..START }) {
            Ok(voice) => {
                self.voice = voice;
                self.next_step_ns = now_ns;
                self.step(now_ns, sink);
            }
            Err(_) => {
                self.failed += 1;
                self.restart = true;
            }
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (PopOut, stopPauseLoop)
    /// The menu hides at `now_ns`: the loop fades out and then stops.
    pub fn hide(&mut self, now_ns: u64) {
        self.restart = false;
        if !self.shown {
            return;
        }
        self.shown = false;
        self.fade = Fade {
            from: self.gain_at(now_ns),
            to: 0.0,
            start_ns: now_ns,
            easing: Easing::Out,
        };
        if self.voice != VoiceId::NONE {
            self.next_step_ns = now_ns;
        }
    }

    /// Moves the fade on to `now_ns`, stopping the loop once it faded out, and retries a stop
    /// that did not queue.
    pub fn update(&mut self, now_ns: u64, sink: &mut impl VoiceSink) {
        self.retry_stops(sink);
        if self.restart {
            self.show(now_ns, sink);
            return;
        }
        if self.voice == VoiceId::NONE || now_ns < self.next_step_ns {
            return;
        }
        if !self.shown && now_ns >= self.fade.end_ns() {
            // Lazer's `Finally(_ => pauseLoop.Stop())`.
            let voice = std::mem::replace(&mut self.voice, VoiceId::NONE);
            self.stop_voice(voice, sink);
            self.next_step_ns = u64::MAX;
            return;
        }
        self.step(now_ns, sink);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (StopAllSamples)
    /// Stops the loop at once, as when the player screen goes. Returns whether the stop
    /// queued; if not, call it again (or keep updating) until it does.
    pub fn stop(&mut self, sink: &mut impl VoiceSink) -> bool {
        self.shown = false;
        self.restart = false;
        self.fade = Fade::SILENT;
        self.next_step_ns = u64::MAX;
        let voice = std::mem::replace(&mut self.voice, VoiceId::NONE);
        self.stop_voice(voice, sink);
        self.pending_stops.iter().all(|&v| v == VoiceId::NONE)
    }

    /// Takes the sample of `other` (the same loop from another skin, ADR-0096) and gives it
    /// this one to free off the update thread. The loop stops; if a menu shows, the next
    /// update starts it again with the new sample at the volume it had. Returns whether the
    /// stop queued, as [`PauseLoop::stop`]. Does not allocate.
    pub fn replace_sample(&mut self, other: &mut PauseLoop, sink: &mut impl VoiceSink) -> bool {
        let (shown, fade) = (self.shown, self.fade);
        let stopped = self.stop(sink);
        std::mem::swap(&mut self.sample, &mut other.sample);
        if shown {
            self.fade = fade;
            self.restart = true;
        }
        stopped
    }

    /// Whether the loop's voice plays (fading out included).
    pub fn is_playing(&self) -> bool {
        self.voice != VoiceId::NONE
    }

    /// Mixer commands that failed so far.
    pub fn failed_commands(&self) -> u64 {
        self.failed
    }

    /// The gain the loop has at `now_ns`: 0 when it does not play.
    fn gain_at(&self, now_ns: u64) -> f32 {
        if self.voice == VoiceId::NONE {
            0.0
        } else {
            self.fade.value_at(now_ns)
        }
    }

    /// Sends the next piece of the fade: a ramp to the curve's value one step on (or to its
    /// end). A ramp that does not queue is tried again next update.
    fn step(&mut self, now_ns: u64, sink: &mut impl VoiceSink) {
        let end_ns = self.fade.end_ns();
        if now_ns >= end_ns {
            // Only a shown loop gets here: a hidden one stops in `update` once faded out.
            match sink.set_voice_gain(self.voice, self.fade.to, 0.0) {
                Ok(()) => self.next_step_ns = u64::MAX,
                Err(_) => self.failed += 1,
            }
            return;
        }
        let until_ns = now_ns.saturating_add(STEP_NS).min(end_ns);
        let gain = self.fade.value_at(until_ns);
        let ramp_ms = (until_ns - now_ns) as f64 / NS_PER_MS;
        match sink.set_voice_gain(self.voice, gain, ramp_ms) {
            Ok(()) => self.next_step_ns = until_ns,
            Err(_) => {
                self.failed += 1;
                self.next_step_ns = now_ns;
            }
        }
    }

    /// Stops `voice`, if any, after the stops kept from before; a stop that does not queue
    /// is kept to be tried again.
    fn stop_voice(&mut self, voice: VoiceId, sink: &mut impl VoiceSink) {
        self.retry_stops(sink);
        if voice == VoiceId::NONE || sink.stop_voice(voice).is_ok() {
            return;
        }
        self.failed += 1;
        // With both slots taken the queue stayed full over two shows. At most one voice
        // starts per show, and a start into a full queue fails too, so this needs a start
        // to queue between failing stops; the shell's `stop_all` as the play closes ends it.
        if let Some(slot) = self.pending_stops.iter_mut().find(|v| **v == VoiceId::NONE) {
            *slot = voice;
        }
    }

    fn retry_stops(&mut self, sink: &mut impl VoiceSink) {
        for slot in &mut self.pending_stops {
            if *slot != VoiceId::NONE && sink.stop_voice(*slot).is_ok() {
                *slot = VoiceId::NONE;
            }
        }
    }
}

#[cfg(test)]
mod tests;
