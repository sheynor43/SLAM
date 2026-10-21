//! Game time: engine timestamps (input events, the tick) to gameplay milliseconds
//! (ADR-0032, ADR-0035).

use slam_audio::ClockSnapshot;

/// User offsets, as in lazer's `FramedBeatmapClock`: positive values make game time
/// later relative to the audio, so objects come earlier.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Offsets {
    /// Global audio offset, real-time milliseconds (scaled by the clock rate).
    pub global_ms: f64,
    /// Per-beatmap offset, track milliseconds.
    pub beatmap_ms: f64,
}

impl Offsets {
    /// What game time is ahead of track time at clock rate `rate`.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/FramedBeatmapClock.cs
    // (`TotalAppliedOffset`; the platform offset is not needed, ADR-0035).
    pub fn total_ms(&self, rate: f64) -> f64 {
        self.global_ms * rate + self.beatmap_ms
    }
}

/// Game time in milliseconds at engine time `time_ns`: the only conversion of input
/// timestamps and tick times.
pub fn game_time(clock: &ClockSnapshot, time_ns: u64, offsets: &Offsets) -> f64 {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/FramedBeatmapClock.cs
    // (the offset clocks add in this order: global, then beatmap).
    clock.track_time_at(time_ns) + offsets.global_ms * clock.rate() + offsets.beatmap_ms
}

/// Track time to seek to so that game time becomes `game_ms` at clock rate `rate`.
pub fn track_time_for(game_ms: f64, rate: f64, offsets: &Offsets) -> f64 {
    game_ms - offsets.total_ms(rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_audio::{
        GameClock, MusicAnchor, MusicSnapshot, PlayError, PositionSnapshot, TrackAdjustment,
        TrackControl,
    };

    struct NoTrack;

    impl TrackControl for NoTrack {
        fn play(&mut self) -> Result<(), PlayError> {
            Ok(())
        }
        fn play_at(&mut self, _: u64) -> Result<(), PlayError> {
            Ok(())
        }
        fn pause(&mut self) -> Result<(), PlayError> {
            Ok(())
        }
        fn seek(&mut self, _: u64) -> Result<(), PlayError> {
            Ok(())
        }
        fn set_rate(&mut self, _: f64) -> Result<(), PlayError> {
            Ok(())
        }

        fn adjust(&mut self, _: &[TrackAdjustment]) -> Result<(), PlayError> {
            Ok(())
        }
        fn generation(&self) -> u32 {
            0
        }
    }

    /// Snapshot with the track playing at `speed` from 0, frame 0 heard at 0 ns.
    fn clock(rate: f64, speed: f64) -> ClockSnapshot {
        let anchor = MusicAnchor {
            device_frame: 0,
            track_frame: 0.0,
            speed,
            accel: 0.0,
        };
        let music = MusicSnapshot {
            anchor,
            previous: anchor,
            generation: 0,
            ended: false,
        };
        let report = PositionSnapshot {
            frame: 0,
            timestamp_ns: 0,
            latency_ns: 0,
            sample_rate: 48_000,
        };
        let mut clock = GameClock::new(&NoTrack);
        clock.update(0, Some(report), Some(music));
        clock.set_rate(0, rate, rate, &mut NoTrack).unwrap();
        clock.update(0, None, None).unwrap()
    }

    #[test]
    fn no_offsets_is_track_time() {
        let c = clock(1.0, 1.0);
        assert_eq!(game_time(&c, 250_000_000, &Offsets::default()), 250.0);
    }

    #[test]
    fn global_offset_is_real_time_and_beatmap_offset_track_time() {
        let offsets = Offsets {
            global_ms: 20.0,
            beatmap_ms: -5.0,
        };
        // Track at 1.5x: 100 ms of output is 150 ms of track.
        let c = clock(1.5, 1.5);
        assert_eq!(game_time(&c, 100_000_000, &offsets), 150.0 + 30.0 - 5.0);
        assert_eq!(game_time(&clock(1.0, 1.0), 100_000_000, &offsets), 115.0);
    }

    #[test]
    fn seeking_back_through_the_offsets_round_trips() {
        let offsets = Offsets {
            global_ms: 12.5,
            beatmap_ms: 7.0,
        };
        let track = track_time_for(1_000.0, 1.5, &offsets);
        assert_eq!(track, 1_000.0 - 18.75 - 7.0);
        assert_eq!(track + offsets.total_ms(1.5), 1_000.0);
    }
}
