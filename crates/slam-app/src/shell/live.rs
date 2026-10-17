//! Hands changed settings to the threads that use them (ADR-0048). Every update tick
//! compares the settings in force with what each receiver was last given and sends the
//! differences; a send that fails (a full ring) is retried on the next tick.

use slam_audio::{MixerHandle, MusicPlayer, VoiceGroup};
use slam_engine::UpdateRateControl;

use crate::loader::PlayOptions;
use crate::settings::{Audio, Settings};

/// A value last handed to another thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Applied<T>(T);

impl<T: Copy + PartialEq> Applied<T> {
    /// A value the receiver already has.
    pub fn new(value: T) -> Applied<T> {
        Applied(value)
    }

    /// Sends `wanted` if it is not the applied value, and takes it as applied once `send`
    /// succeeds. Returns whether the receiver has `wanted` now.
    pub fn sync<E>(&mut self, wanted: T, send: impl FnOnce(T) -> Result<(), E>) -> bool {
        if self.0 == wanted {
            return true;
        }
        let sent = send(wanted).is_ok();
        if sent {
            self.0 = wanted;
        }
        sent
    }

    #[cfg(test)]
    pub fn get(&self) -> T {
        self.0
    }
}

/// The audio gains the settings call for, linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volumes {
    /// The mixer's master gain; it leaves the music alone.
    pub master: f32,
    /// The track's volume, which takes the master volume itself.
    pub music: f32,
    pub hitsounds: f32,
    pub effects: f32,
}

impl Volumes {
    /// A non-finite volume counts as silence, as the mixer takes it; this also keeps
    /// [`Applied`] from resending a NaN every tick.
    pub fn of(settings: &Audio) -> Volumes {
        let gain = |v: f64| if v.is_finite() { v as f32 } else { 0.0 };
        let master = gain(settings.master_volume);
        Volumes {
            master,
            music: master * gain(settings.music_volume),
            hitsounds: gain(settings.hitsound_volume),
            effects: gain(settings.effects_volume),
        }
    }
}

/// The gains the mixer and the music player were last given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppliedVolumes {
    master: Applied<f32>,
    music: Applied<f32>,
    hitsounds: Applied<f32>,
    effects: Applied<f32>,
}

impl AppliedVolumes {
    /// Gains the output was opened with.
    pub fn new(volumes: Volumes) -> AppliedVolumes {
        AppliedVolumes {
            master: Applied::new(volumes.master),
            music: Applied::new(volumes.music),
            hitsounds: Applied::new(volumes.hitsounds),
            effects: Applied::new(volumes.effects),
        }
    }

    /// Sends the gains that changed. Does not allocate or block.
    pub fn sync(&mut self, wanted: Volumes, mixer: &mut MixerHandle, player: &mut MusicPlayer) {
        self.master
            .sync(wanted.master, |g| mixer.set_master_gain(g));
        self.music.sync(wanted.music, |g| player.set_volume(g));
        self.hitsounds.sync(wanted.hitsounds, |g| {
            mixer.set_group_gain(VoiceGroup::Hitsounds, g)
        });
        self.effects.sync(wanted.effects, |g| {
            mixer.set_group_gain(VoiceGroup::Effects, g)
        });
    }
}

/// The update rate the engine was last given.
#[derive(Debug)]
pub struct AppliedUpdateRate {
    control: UpdateRateControl,
    hz: Applied<f64>,
}

impl AppliedUpdateRate {
    /// `control` runs at `hz` already.
    pub fn new(control: UpdateRateControl, hz: f64) -> AppliedUpdateRate {
        AppliedUpdateRate {
            control,
            hz: Applied::new(hz),
        }
    }

    /// Requests the rate if it changed; the engine applies it after this tick. Does not
    /// allocate or block.
    pub fn sync(&mut self, settings: &Settings) {
        let control = &self.control;
        // An invalid rate is never sent and never taken as applied; the settings are
        // validated when read and edited, so it does not happen.
        self.hz
            .sync(settings.frame.update_hz, |hz| control.set_hz(hz));
    }
}

/// What the next play takes from the settings, prepared on the loader thread.
pub fn play_options(settings: &Settings) -> PlayOptions {
    PlayOptions {
        dim: settings.gameplay.dim,
        beatmap_colours: settings.gameplay.beatmap_colours,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_value_is_not_sent() {
        let mut applied = Applied::new(0.5_f32);
        let mut sent = 0;
        assert!(applied.sync(0.5, |_| {
            sent += 1;
            Ok::<(), ()>(())
        }));
        assert_eq!(sent, 0);
    }

    #[test]
    fn a_change_is_sent_once() {
        let mut applied = Applied::new(0.5_f32);
        let mut sent = Vec::new();
        for _ in 0..3 {
            assert!(applied.sync(0.25, |v| {
                sent.push(v);
                Ok::<(), ()>(())
            }));
        }
        assert_eq!(sent, [0.25]);
        assert_eq!(applied.get(), 0.25);
    }

    #[test]
    fn a_failed_send_is_retried_until_it_succeeds() {
        let mut applied = Applied::new(1.0_f32);
        assert!(!applied.sync(0.3, |_| Err(())));
        assert_eq!(applied.get(), 1.0);
        assert!(!applied.sync(0.3, |_| Err(())));
        let mut sent = Vec::new();
        assert!(applied.sync(0.3, |v| {
            sent.push(v);
            Ok::<(), ()>(())
        }));
        assert_eq!(sent, [0.3]);
        assert_eq!(applied.get(), 0.3);
    }

    #[test]
    fn a_change_back_before_a_retry_sends_nothing() {
        let mut applied = Applied::new(1.0_f32);
        assert!(!applied.sync(0.3, |_| Err(())));
        let mut sent = 0;
        assert!(applied.sync(1.0, |_| {
            sent += 1;
            Ok::<(), ()>(())
        }));
        assert_eq!(sent, 0);
    }

    #[test]
    fn the_music_takes_the_master_volume_and_the_groups_do_not() {
        let audio = Audio {
            master_volume: 0.5,
            music_volume: 0.5,
            hitsound_volume: 0.75,
            effects_volume: 0.25,
            ..Audio::default()
        };
        assert_eq!(
            Volumes::of(&audio),
            Volumes {
                master: 0.5,
                music: 0.25,
                hitsounds: 0.75,
                effects: 0.25,
            }
        );
    }

    #[test]
    fn a_non_finite_volume_is_silence() {
        let audio = Audio {
            master_volume: f64::NAN,
            hitsound_volume: f64::INFINITY,
            ..Audio::default()
        };
        let volumes = Volumes::of(&audio);
        assert_eq!(
            (volumes.master, volumes.music, volumes.hitsounds),
            (0.0, 0.0, 0.0)
        );
    }

    #[test]
    fn the_update_rate_is_requested_only_on_a_change() {
        let control = UpdateRateControl::new();
        control.set_hz(1000.0).unwrap();
        let mut applied = AppliedUpdateRate::new(control.clone(), 1000.0);
        let mut settings = Settings::default();
        settings.frame.update_hz = 1000.0;
        // Nothing is written while the setting stays the same: a rate set elsewhere stays.
        control.set_hz(500.0).unwrap();
        applied.sync(&settings);
        assert_eq!(control.hz(), 500.0);
        settings.frame.update_hz = 4000.0;
        applied.sync(&settings);
        assert_eq!(control.hz(), 4000.0);
    }

    #[test]
    fn the_next_play_takes_the_dim_and_beatmap_colours() {
        let mut settings = Settings::default();
        settings.gameplay.dim = 0.3;
        settings.gameplay.beatmap_colours = false;
        assert_eq!(
            play_options(&settings),
            PlayOptions {
                dim: 0.3,
                beatmap_colours: false,
            }
        );
    }
}
