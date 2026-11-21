//! The restart's black cover (ADR-0164): the screen darkens while quick retry is held, and a
//! restarted play shows from under black, as lazer's `HoldToConfirmOverlay` and
//! `PlayerLoader` do.

use crate::session::PlaySnapshot;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PlayerLoader.cs (contentIn)
/// How long a restarted play stays fully black, ns.
pub const BLACK_NS: u64 = 50_000_000;
/// How long the black then takes to fade out (OutQuint), ns: as long as lazer's content
/// takes to fade in (`FadeInFromZero(500, OutQuint)`); lazer's own black layer takes 5 s,
/// which feels too slow without its loader screen.
pub const FADE_NS: u64 = 500_000_000;

/// The black over a play restarted `since_ns` ago.
pub fn after_restart(since_ns: u64) -> f32 {
    let Some(fading) = since_ns.checked_sub(BLACK_NS) else {
        return 1.0;
    };
    let t = (fading as f64 / FADE_NS as f64).min(1.0);
    let u = t - 1.0;
    // 1 − OutQuint.
    -(u * u * u * u * u) as f32
}

/// The black over the play of `snapshot` at engine time `now_ns`: the hold of quick retry
/// (`HoldToConfirmOverlay`, its alpha the hold's progress) or the fade after a restart,
/// whichever is darker; 0 with the animation off.
pub fn cover_alpha(snapshot: &PlaySnapshot, now_ns: u64) -> f32 {
    if !snapshot.restart_animation {
        return 0.0;
    }
    let after = snapshot
        .restart_ns
        .map_or(0.0, |at| after_restart(now_ns.saturating_sub(at)));
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/HoldToConfirmOverlay.cs (load)
    snapshot.retry_progress.clamp(0.0, 1.0).max(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn a_restarted_play_stays_black_for_50_ms_then_fades_out_quint() {
        assert_eq!(after_restart(0), 1.0);
        assert_eq!(after_restart(50 * MS), 1.0);
        // OutQuint: most of the black is gone within the first fifth of the half second.
        let a = after_restart((50 + 100) * MS);
        assert!((a - 0.8f32.powi(5)).abs() < 1e-4, "{a}");
        assert_eq!(after_restart((50 + 500) * MS), 0.0);
        assert_eq!(after_restart(60_000 * MS), 0.0);
    }

    #[test]
    fn the_cover_is_the_darker_of_the_hold_and_the_fade_and_off_without_the_animation() {
        let mut s = PlaySnapshot {
            retry_progress: 0.4,
            restart_animation: true,
            ..PlaySnapshot::default()
        };
        assert!((cover_alpha(&s, 0) - 0.4).abs() < 1e-6);
        s.restart_ns = Some(1000 * MS);
        assert_eq!(cover_alpha(&s, 1000 * MS), 1.0);
        s.restart_animation = false;
        assert_eq!(cover_alpha(&s, 1000 * MS), 0.0);
    }
}
