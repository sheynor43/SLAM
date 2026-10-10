//! Update-thread side of the click test: every press queues the hitsound and records
//! when it happened, when it was queued and, from the mixer's reports, which buffer
//! played it (ADR-0025).

use std::fmt;
use std::sync::Arc;

use slam_audio::{MixerHandle, PlayError, Sample, VoiceStart};
use slam_engine::clock::now_ns;
use slam_engine::{TickInfo, Update};
use slam_input::{InputEvent, InputKind};

use crate::bench::Percentiles;

/// One press that reached the mixer queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Click {
    /// Input event timestamp, engine clock.
    pub event_ns: u64,
    /// Right before `play`, engine clock: the audio thread may take the command as
    /// soon as `play` publishes it.
    pub queued_ns: u64,
    /// Number of the `play` command, matched against [`VoiceStart::play`].
    pub play: u64,
}

/// Presses and voice starts recorded on the update thread.
#[derive(Debug)]
pub struct Clicks {
    clicks: Vec<Click>,
    starts: Vec<VoiceStart>,
    /// Presses the mixer queue rejected (full queue: the audio thread is stalled).
    pub rejected: u64,
    /// Presses past the recording capacity.
    pub unrecorded_clicks: u64,
    /// Voice starts past the recording capacity.
    pub unrecorded_starts: u64,
}

impl Clicks {
    /// Room for `capacity` presses, allocated here.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            clicks: Vec::with_capacity(capacity),
            starts: Vec::with_capacity(capacity),
            rejected: 0,
            unrecorded_clicks: 0,
            unrecorded_starts: 0,
        }
    }

    fn push_click(&mut self, click: Click) {
        if self.clicks.len() < self.clicks.capacity() {
            self.clicks.push(click);
        } else {
            self.unrecorded_clicks += 1;
        }
    }

    fn push_start(&mut self, start: VoiceStart) {
        if self.starts.len() < self.starts.capacity() {
            self.starts.push(start);
        } else {
            self.unrecorded_starts += 1;
        }
    }

    pub fn clicks(&self) -> &[Click] {
        &self.clicks
    }

    pub fn starts(&self) -> &[VoiceStart] {
        &self.starts
    }

    /// Joins presses with their voice starts. Presses whose start was not reported
    /// (lost report, or the stream stopped first) are left out.
    pub fn matched(&self) -> Vec<(Click, VoiceStart)> {
        // Both lists are in play order; a lost report leaves a gap in `starts`.
        let mut starts = self.starts.iter().peekable();
        let mut out = Vec::with_capacity(self.clicks.len());
        for click in &self.clicks {
            while starts.next_if(|s| s.play < click.play).is_some() {}
            if let Some(start) = starts.next_if(|s| s.play == click.play) {
                out.push((*click, *start));
            }
        }
        out
    }

    /// Latency statistics over the matched presses.
    pub fn report(&self) -> ClickReport {
        let matched = self.matched();
        let mut input_to_queue: Vec<u32> = self
            .clicks
            .iter()
            .map(|c| span(c.event_ns, c.queued_ns))
            .collect();
        // Only buffers with backend timing: the first buffers of a stream have none.
        let timed: Vec<_> = matched
            .iter()
            .filter(|(_, s)| s.output_ns().is_some())
            .collect();
        let output = |s: &VoiceStart| s.output_ns().unwrap_or(0);
        let mut queue_to_output: Vec<u32> = timed
            .iter()
            .map(|(c, s)| span(c.queued_ns, output(s)))
            .collect();
        let mut device: Vec<u32> = timed.iter().map(|(_, s)| clamp(s.latency_ns)).collect();
        let mut input_to_output: Vec<u32> = timed
            .iter()
            .map(|(c, s)| span(c.event_ns, output(s)))
            .collect();
        // The backend times a buffer at the start of its graph cycle, before the
        // callback takes the commands: a command queued in between plays in that
        // buffer although its timing predates the command.
        let queued_in_cycle = timed
            .iter()
            .filter(|(c, s)| s.timestamp_ns < c.queued_ns)
            .count();
        ClickReport {
            clicks: self.clicks.len(),
            matched: matched.len(),
            untimed: matched.len() - timed.len(),
            queued_in_cycle,
            rejected: self.rejected,
            unrecorded_clicks: self.unrecorded_clicks,
            unrecorded_starts: self.unrecorded_starts,
            input_to_queue: Percentiles::of(&mut input_to_queue),
            queue_to_output: Percentiles::of(&mut queue_to_output),
            device_latency: Percentiles::of(&mut device),
            input_to_output: Percentiles::of(&mut input_to_output),
        }
    }

    /// One CSV row per matched press, times relative to the first press, in µs.
    pub fn write_csv<W: std::io::Write>(&self, mut out: W) -> std::io::Result<()> {
        writeln!(
            out,
            "play,event_us,input_to_queue_us,queue_to_output_us,device_latency_us,frame"
        )?;
        let Some(first) = self.clicks.first() else {
            return Ok(());
        };
        let us = |ns: u64| ns as f64 / 1000.0;
        // Without backend timing there is no output estimate: the column stays empty.
        for (c, s) in self.matched() {
            let queue_to_output = s.output_ns().map_or_else(String::new, |o| {
                format!("{:.1}", us(o.saturating_sub(c.queued_ns)))
            });
            writeln!(
                out,
                "{},{:.1},{:.1},{},{:.1},{}",
                c.play,
                us(c.event_ns.saturating_sub(first.event_ns)),
                us(c.queued_ns.saturating_sub(c.event_ns)),
                queue_to_output,
                us(s.latency_ns),
                s.frame
            )?;
        }
        Ok(())
    }
}

/// `to - from` in nanoseconds, 0 if negative, saturated to `u32`.
fn span(from: u64, to: u64) -> u32 {
    clamp(to.saturating_sub(from))
}

fn clamp(ns: u64) -> u32 {
    ns.min(u64::from(u32::MAX)) as u32
}

/// Result of a click test.
#[derive(Clone, Debug, PartialEq)]
pub struct ClickReport {
    pub clicks: usize,
    /// Presses with a voice-start report.
    pub matched: usize,
    /// Matched presses whose buffer had no backend timing (stream start).
    pub untimed: usize,
    /// Timed presses queued after the backend timed the buffer they played in.
    pub queued_in_cycle: usize,
    pub rejected: u64,
    pub unrecorded_clicks: u64,
    pub unrecorded_starts: u64,
    /// Input event timestamp → command queued; over all recorded presses. The
    /// timestamp is when SDL got the event from the window system, so compositor and
    /// driver latency before it are not included.
    pub input_to_queue: Percentiles,
    /// Command queued → estimated first sample of the voice out of the device; over
    /// timed presses, like the two below.
    pub queue_to_output: Percentiles,
    /// Latency the backend reports from its timing of the buffer to the buffer
    /// leaving the device.
    pub device_latency: Percentiles,
    /// Input event timestamp → estimated first sample out of the device.
    pub input_to_output: Percentiles,
}

impl fmt::Display for ClickReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "presses: {} (voice start reported: {}, without backend timing: {}, \
             queued after the cycle was timed: {}, rejected by full queue: {}, \
             unrecorded presses/starts: {}/{})",
            self.clicks,
            self.matched,
            self.untimed,
            self.queued_in_cycle,
            self.rejected,
            self.unrecorded_clicks,
            self.unrecorded_starts
        )?;
        writeln!(
            f,
            "µs; input → queued over all presses, the rest over timed ones; \
             input is the SDL timestamp:"
        )?;
        writeln!(f, "  input → queued:              {}", self.input_to_queue)?;
        writeln!(f, "  queued → output (est.):      {}", self.queue_to_output)?;
        writeln!(f, "  reported device latency:     {}", self.device_latency)?;
        writeln!(f, "  input → output (est.):       {}", self.input_to_output)
    }
}

/// [`Update`] that plays `sample` on every key or mouse button press.
pub struct Clicker {
    mixer: MixerHandle,
    sample: Arc<Sample>,
    pub clicks: Clicks,
}

impl Clicker {
    /// Records up to `capacity` presses.
    pub fn new(mixer: MixerHandle, sample: Arc<Sample>, capacity: usize) -> Self {
        Self {
            mixer,
            sample,
            clicks: Clicks::with_capacity(capacity),
        }
    }

    /// Final voice-start reports, after the stream has stopped.
    pub fn finish(&mut self) {
        let clicks = &mut self.clicks;
        self.mixer.drain_started(|s| clicks.push_start(s));
    }

    pub fn mixer(&self) -> &MixerHandle {
        &self.mixer
    }
}

impl Update for Clicker {
    /// Presses so far, for the draw thread to flash on.
    type Snapshot = u64;

    fn tick(&mut self, _: &TickInfo, events: &[InputEvent], presses: &mut u64) {
        for event in events {
            let pressed = matches!(
                event.kind,
                InputKind::Key { down: true, .. } | InputKind::MouseButton { down: true, .. }
            );
            if !pressed {
                continue;
            }
            let queued_ns = now_ns();
            match self.mixer.play(&self.sample, 1.0) {
                Ok(()) => {
                    self.clicks.push_click(Click {
                        event_ns: event.time_ns,
                        queued_ns,
                        play: self.mixer.queued_plays() - 1,
                    });
                }
                Err(PlayError::QueueFull) => self.clicks.rejected += 1,
                // `run` converts the sample to the mixer format before starting.
                Err(PlayError::FormatMismatch) => unreachable!("sample format checked"),
            }
        }
        let clicks = &mut self.clicks;
        self.mixer.drain_started(|s| clicks.push_start(s));
        *presses = self.mixer.queued_plays();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(play: u64, event_ns: u64, queued_ns: u64) -> Click {
        Click {
            event_ns,
            queued_ns,
            play,
        }
    }

    fn start(play: u64, timestamp_ns: u64, latency_ns: u64) -> VoiceStart {
        VoiceStart {
            play,
            frame: play * 128,
            timestamp_ns,
            latency_ns,
        }
    }

    fn clicks(clicks: &[Click], starts: &[VoiceStart]) -> Clicks {
        let mut c = Clicks::with_capacity(8);
        for &click in clicks {
            c.push_click(click);
        }
        for &s in starts {
            c.push_start(s);
        }
        c
    }

    #[test]
    fn matches_presses_with_starts_across_gaps() {
        // Report for play 1 lost; play 3 not reported yet; a start (2) without a
        // recorded press is skipped.
        let c = clicks(
            &[click(0, 0, 1), click(1, 2, 3), click(3, 4, 5)],
            &[start(0, 10, 0), start(2, 11, 0)],
        );
        let plays: Vec<u64> = c.matched().iter().map(|(c, _)| c.play).collect();
        assert_eq!(plays, [0]);
    }

    #[test]
    fn report_splits_the_path() {
        let c = clicks(
            &[click(0, 1_000, 21_000), click(1, 5_000, 6_000)],
            // Play 1 started before the backend had timing.
            &[start(0, 2_000_000, 10_000_000), start(1, 0, 0)],
        );
        let r = c.report();
        assert_eq!((r.clicks, r.matched, r.untimed), (2, 2, 1));
        assert_eq!(r.input_to_queue.samples, 2);
        assert_eq!(r.input_to_queue.max_ns, 20_000);
        assert_eq!(r.queue_to_output.max_ns, 12_000_000 - 21_000);
        assert_eq!(r.device_latency.max_ns, 10_000_000);
        assert_eq!(r.input_to_output.max_ns, 12_000_000 - 1_000);
        assert_eq!(r.input_to_output.samples, 1);
        assert_eq!(r.queued_in_cycle, 0);
    }

    #[test]
    fn queued_after_cycle_timing_still_adds_up() {
        // Timed at 1 ms, queued at 1.2 ms, played in that buffer.
        let c = clicks(
            &[click(0, 1_100_000, 1_200_000)],
            &[start(0, 1_000_000, 3_000_000)],
        );
        let r = c.report();
        assert_eq!(r.queued_in_cycle, 1);
        assert_eq!(r.queue_to_output.max_ns, 2_800_000);
        assert_eq!(
            r.input_to_queue.max_ns + r.queue_to_output.max_ns,
            r.input_to_output.max_ns
        );
    }

    #[test]
    fn spans_saturate() {
        let c = clicks(&[click(0, 0, 5_000_000_000)], &[start(0, 1, u64::MAX)]);
        let r = c.report();
        assert_eq!(r.input_to_queue.max_ns, u32::MAX);
        assert_eq!(r.device_latency.max_ns, u32::MAX);
        assert_eq!(r.input_to_output.max_ns, u32::MAX);
    }

    #[test]
    fn recording_stops_at_capacity() {
        let mut c = Clicks::with_capacity(1);
        c.push_click(click(0, 0, 0));
        c.push_click(click(1, 0, 0));
        c.push_start(start(0, 1, 0));
        c.push_start(start(1, 1, 0));
        assert_eq!((c.clicks().len(), c.starts().len()), (1, 1));
        assert_eq!((c.unrecorded_clicks, c.unrecorded_starts), (1, 1));
    }

    #[test]
    fn csv_has_a_row_per_matched_press() {
        let c = clicks(
            &[click(0, 1_000, 3_000), click(1, 2_001_000, 2_002_000)],
            // Play 1 started before the backend had timing: no output estimate.
            &[start(0, 5_000, 7_000), start(1, 0, 0)],
        );
        let mut out = Vec::new();
        c.write_csv(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], "0,0.0,2.0,9.0,7.0,0");
        assert_eq!(lines[2], "1,2000.0,1.0,,0.0,128");
    }
}
