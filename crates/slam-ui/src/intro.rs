//! The startup intro (ADR-0186, timing and sound of ADR-0114): the four pieces of the
//! logo's mark fly in on black one by one in a shuffled order, blinking, gather into the
//! mark, and on the impact the disc lights up under them; the logo then settles into the
//! main menu's logo while the black fades out. A pure function of time: [`piece_pose`],
//! [`logo_pose`] and [`backdrop`] for the picture, [`cues`] for the sound, so that both
//! agree. [`Intro`] is the update thread's state; [`IntroView`] draws a snapshot.
//!
//! The lengths are constants of this module rather than theme tokens, as the curtain's:
//! the intro is a branded animation, not a transition (ADR-0114).

mod view;

pub use view::IntroView;

/// Length of the whole intro.
pub const LENGTH_NS: u64 = 2_600_000_000;
/// The impact: the mark is whole and the disc lights up.
pub const IMPACT_NS: u64 = 2_000_000_000;
/// Fade-out after a skip.
pub const SKIP_NS: u64 = 200_000_000;

/// Every piece is lit from here on and they gather, and the riser starts, in seconds.
pub const GATHER_S: f32 = 1.4;
/// [`IMPACT_NS`] in seconds.
pub const IMPACT_S: f32 = 2.0;
/// [`LENGTH_NS`] in seconds.
pub const END_S: f32 = 2.6;
/// The black starts fading out.
const BACKDROP_FADE_S: f32 = 2.2;
/// The menu starts to show under the fading black, and its toolbar slides in (ADR-0178).
pub const REVEAL_NS: u64 = 2_200_000_000;

/// When the first letter appears: the glyphs are generated meanwhile.
const FIRST_APPEAR_S: f32 = 0.15;
/// Every letter appears within this long after the first.
const APPEAR_SPAN_S: f32 = 0.9;
/// Longest gap between two letters appearing.
const MAX_APPEAR_STEP_S: f32 = 0.22;
/// When a letter is lit after it appears, in seconds: on from the first time of each pair
/// to the second. Each switch on is a blip.
const FLICKER: [(f32, f32); 3] = [(0.0, 0.05), (0.10, 0.14), (0.22, f32::INFINITY)];
/// A lit letter goes out once more, for [`GLITCH_S`], this long (plus up to
/// [`GLITCH_JITTER_S`]) after it appears, unless that runs into the gathering.
const GLITCH_AFTER_S: f32 = 0.42;
const GLITCH_JITTER_S: f32 = 0.1;
const GLITCH_S: f32 = 0.05;

/// Pieces of the mark (frame, play, facet, accent), as many as the cues' letters.
pub const PIECES: usize = 4;
/// Size of the logo until the impact, relative to the menu's (lazer's intro: 1.2).
pub const BIG_SCALE: f32 = 1.25;
/// Where each piece flies in from (degrees, y down) and how much it is turned while apart.
const PIECE_FROM: [f32; PIECES] = [200.0, 340.0, 100.0, 300.0];
const PIECE_TURN: [f32; PIECES] = [-0.35, 0.3, 0.45, -0.6];
/// How far the pieces fly from, and where they wait apart, as parts of the logo's size.
const FLY_FROM: f32 = 1.6;
const APART: f32 = 0.28;
/// How long a piece flies in.
const FLY_S: f32 = 0.3;
/// The white flash on the impact: strength and length.
const IMPACT_FLASH: f32 = 0.6;
const IMPACT_FLASH_S: f32 = 0.5;
/// Seed of the fixed choreography.
const SEED: u32 = 0x5_1A_4D;

/// The intro's state on the update thread: it starts on the first tick and may be
/// skipped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Intro {
    start_ns: Option<u64>,
    skip_ns: Option<u64>,
}

impl Intro {
    pub const fn new() -> Intro {
        Intro {
            start_ns: None,
            skip_ns: None,
        }
    }

    /// Starts the clock at `now_ns`; later calls do nothing.
    pub fn start(&mut self, now_ns: u64) {
        self.start_ns.get_or_insert(now_ns);
    }

    pub fn is_started(&self) -> bool {
        self.start_ns.is_some()
    }

    /// Fades the intro out over [`SKIP_NS`] from `now_ns`. Does nothing before the start,
    /// once skipped or once over.
    pub fn skip(&mut self, now_ns: u64) {
        if self.start_ns.is_some() && self.skip_ns.is_none() && !self.is_done(now_ns) {
            self.skip_ns = Some(now_ns);
        }
    }

    pub fn is_skipped(&self) -> bool {
        self.skip_ns.is_some()
    }

    /// When it was skipped.
    pub fn skipped_at(&self) -> Option<u64> {
        self.skip_ns
    }

    /// Whether nothing of the intro shows any more.
    pub fn is_done(&self, now_ns: u64) -> bool {
        let Some(start) = self.start_ns else {
            return false;
        };
        let skipped = self
            .skip_ns
            .is_some_and(|skip| now_ns >= skip.saturating_add(SKIP_NS));
        skipped || now_ns >= start.saturating_add(LENGTH_NS)
    }

    /// When the menu starts to show under the intro: its black starts fading out, or the
    /// intro was skipped. `None` before the start.
    pub fn reveal_at(&self) -> Option<u64> {
        let start = self.start_ns?;
        Some(
            self.skip_ns
                .map_or(start.saturating_add(REVEAL_NS), |skip| {
                    skip.min(start.saturating_add(REVEAL_NS))
                }),
        )
    }

    /// Whether the menu's music may start: from the impact, or once skipped.
    pub fn music_due(&self, now_ns: u64) -> bool {
        self.start_ns
            .is_some_and(|start| self.skip_ns.is_some() || now_ns >= start + IMPACT_NS)
    }

    /// What the draw thread shows at `now_ns`; `None` once over.
    pub fn snapshot(&self, now_ns: u64) -> Option<IntroSnapshot> {
        if self.is_done(now_ns) {
            return None;
        }
        let elapsed_ns = self.start_ns.map_or(0, |s| now_ns.saturating_sub(s));
        let exit = self
            .skip_ns
            .map_or(0.0, |skip| crate::anim::progress(skip, now_ns, SKIP_NS));
        Some(IntroSnapshot {
            elapsed_ns,
            exit,
            skipped: self.skip_ns.is_some(),
        })
    }
}

/// The intro at one moment, for the draw thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntroSnapshot {
    /// Since the start.
    pub elapsed_ns: u64,
    /// How far the fade-out after a skip is, 0 ..= 1.
    pub exit: f32,
    /// Skipped: the menu shows its own logo under the fading intro.
    pub skipped: bool,
}

impl IntroSnapshot {
    /// Whether the menu leaves its logo to the intro.
    pub fn hides_menu_logo(&self) -> bool {
        !self.skipped
    }
}

/// Where one piece of the mark is and how it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PiecePose {
    /// Away from its place in the mark, as a part of the logo's size.
    pub offset: [f32; 2],
    /// Turn in radians.
    pub rotation: f32,
    /// Opacity, 0 ..= 1: the piece blinks before the gathering.
    pub alpha: f32,
}

/// The pose of piece `index` at `t` seconds since the start: dark until it appears, then
/// flying in from its side to wait apart, blinking; from the gathering on it slides into
/// its place by the impact.
pub fn piece_pose(index: usize, t: f32) -> PiecePose {
    let index = index.min(PIECES - 1);
    let appear = appear_s(index, PIECES);
    let fly = crate::anim::ease_out_expo(((t - appear) / FLY_S).clamp(0.0, 1.0));
    let gather = ease_in_out_cubic((t - GATHER_S) / (IMPACT_S - GATHER_S));
    let distance = (FLY_FROM + (APART - FLY_FROM) * fly) * (1.0 - gather);
    let (sin, cos) = PIECE_FROM[index].to_radians().sin_cos();
    PiecePose {
        offset: [cos * distance, sin * distance],
        rotation: PIECE_TURN[index] * (1.0 - fly * 0.5) * (1.0 - gather),
        alpha: if is_lit(index, PIECES, t) { 1.0 } else { 0.0 },
    }
}

/// The whole logo at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogoPose {
    /// Size relative to the menu's logo.
    pub scale: f32,
    /// Opacity of the disc under the mark.
    pub disc: f32,
    /// White over the logo.
    pub flash: f32,
}

/// The logo at `t` seconds: big until the impact, when the disc lights up with a flash,
/// then settling to the menu's size by the end.
pub fn logo_pose(t: f32) -> LogoPose {
    let settle = ease_in_out_cubic((t - IMPACT_S) / (END_S - IMPACT_S));
    let since = t - IMPACT_S;
    LogoPose {
        scale: BIG_SCALE + (1.0 - BIG_SCALE) * settle,
        disc: smoothstep(IMPACT_S - 0.05, IMPACT_S + 0.08, t),
        flash: if since >= 0.0 {
            IMPACT_FLASH * (1.0 - since / IMPACT_FLASH_S).max(0.0)
        } else {
            0.0
        },
    }
}

/// Opacity of the black over the menu at `t` seconds.
pub fn backdrop(t: f32) -> f32 {
    1.0 - smoothstep(BACKDROP_FADE_S, END_S, t)
}

/// What the sound plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CueKind {
    /// A letter lights up; `rank` is its place in the order the letters appear, `step`
    /// counts its switches on.
    Blip { rank: u32, step: u32 },
    /// A rising sweep lasting `length_s`, up to the impact.
    Sweep { length_s: f32 },
    /// The letters stand in a row.
    Impact,
}

/// One sound at `at_s` seconds since the start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    pub at_s: f32,
    pub kind: CueKind,
}

/// The sounds of the intro of a `count`-letter title, in time order, into `out` (cleared
/// first).
pub fn cues(count: usize, out: &mut Vec<Cue>) {
    out.clear();
    for index in 0..count {
        let rank = rank(index, count) as u32;
        let appear = appear_s(index, count);
        let mut step = 0;
        for (on, _) in FLICKER {
            out.push(Cue {
                at_s: appear + on,
                kind: CueKind::Blip { rank, step },
            });
            step += 1;
        }
        if let Some((_, end)) = glitch(index, count) {
            out.push(Cue {
                at_s: end,
                kind: CueKind::Blip { rank, step },
            });
        }
    }
    out.push(Cue {
        at_s: GATHER_S,
        kind: CueKind::Sweep {
            length_s: IMPACT_S - GATHER_S,
        },
    });
    out.push(Cue {
        at_s: IMPACT_S,
        kind: CueKind::Impact,
    });
    out.sort_by(|a, b| a.at_s.total_cmp(&b.at_s));
}

/// Whether letter `index` is lit at `t`.
fn is_lit(index: usize, count: usize, t: f32) -> bool {
    let appear = appear_s(index, count);
    if t < appear {
        return false;
    }
    if t >= GATHER_S {
        return true;
    }
    if glitch(index, count).is_some_and(|(from, to)| (from..to).contains(&t)) {
        return false;
    }
    let since = t - appear;
    FLICKER.iter().any(|&(on, off)| since >= on && since < off)
}

/// When letter `index` goes out once more, if before the gathering.
fn glitch(index: usize, count: usize) -> Option<(f32, f32)> {
    let from = appear_s(index, count)
        + GLITCH_AFTER_S
        + GLITCH_JITTER_S * unit01(hash(SEED ^ 0x9E37 ^ index as u32));
    let to = from + GLITCH_S;
    (to <= GATHER_S).then_some((from, to))
}

/// When letter `index` first lights up.
fn appear_s(index: usize, count: usize) -> f32 {
    let step = (APPEAR_SPAN_S / count.saturating_sub(1).max(1) as f32).min(MAX_APPEAR_STEP_S);
    FIRST_APPEAR_S + rank(index, count) as f32 * step
}

/// Place of letter `index` in the shuffled order the letters appear in.
fn rank(index: usize, count: usize) -> usize {
    let key = |i: usize| (hash(SEED ^ i as u32), i);
    let own = key(index);
    (0..count).filter(|&i| key(i) < own).count()
}

/// A well-mixed 32-bit hash (lowbias32).
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x
}

/// `h` mapped to `0 ..= 1`.
fn unit01(h: u32) -> f32 {
    (h >> 8) as f32 / ((1 << 24) - 1) as f32
}

/// 0 below `from`, 1 above `to`, smooth (Hermite) in between.
fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Cubic ease-in-out; `t` is clamped to `0 ..= 1`.
fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - u * u * u / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn letters_appear_in_a_shuffled_order_each_once() {
        for count in 1..=8 {
            let mut ranks: Vec<usize> = (0..count).map(|i| rank(i, count)).collect();
            ranks.sort_unstable();
            assert_eq!(ranks, (0..count).collect::<Vec<_>>(), "{count}");
        }
        // Not left to right for the four letters of "SLAM".
        let order: Vec<usize> = (0..4).map(|i| rank(i, 4)).collect();
        assert_ne!(order, vec![0, 1, 2, 3]);
        assert_ne!(order, vec![3, 2, 1, 0]);
    }

    #[test]
    fn letters_are_dark_before_appearing_and_blink_after() {
        for i in 0..4 {
            let appear = appear_s(i, 4);
            assert!(appear >= FIRST_APPEAR_S);
            assert!(appear <= FIRST_APPEAR_S + APPEAR_SPAN_S);
            assert!(!is_lit(i, 4, appear - 0.001));
            assert!(is_lit(i, 4, appear + 0.01));
            // The first gap of the flicker.
            assert!(!is_lit(i, 4, appear + 0.07));
            assert!(is_lit(i, 4, appear + 0.12));
        }
        // Black at the start, every letter lit from the gathering on.
        assert!((0..4).all(|i| !is_lit(i, 4, 0.0)));
        for t in [GATHER_S, IMPACT_S, END_S] {
            assert!((0..4).all(|i| is_lit(i, 4, t)));
        }
    }

    #[test]
    fn pieces_fly_in_wait_apart_and_gather_by_the_impact() {
        for i in 0..PIECES {
            let appear = appear_s(i, PIECES);
            let far = piece_pose(i, appear);
            let apart = piece_pose(i, appear + FLY_S);
            let length = |p: PiecePose| p.offset[0].hypot(p.offset[1]);
            assert!((length(far) - FLY_FROM).abs() < 1e-3, "{i}");
            assert!((length(apart) - APART).abs() < 0.01, "{i}");
            let whole = piece_pose(i, IMPACT_S);
            assert!(length(whole) < 1e-6 && whole.rotation.abs() < 1e-6);
            assert_eq!(whole.alpha, 1.0);
            assert_eq!(piece_pose(i, 0.0).alpha, 0.0);
        }
    }

    #[test]
    fn the_logo_lights_up_on_the_impact_and_settles_to_the_menu() {
        assert_eq!(logo_pose(1.0).disc, 0.0);
        assert_eq!(logo_pose(1.0).scale, BIG_SCALE);
        let impact = logo_pose(IMPACT_S + 0.1);
        assert_eq!(impact.disc, 1.0);
        assert!(impact.flash > 0.0);
        let end = logo_pose(END_S);
        assert_eq!((end.scale, end.flash), (1.0, 0.0));
    }

    #[test]
    fn backdrop_is_black_until_it_fades_out_at_the_end() {
        assert_eq!(backdrop(0.0), 1.0);
        assert_eq!(backdrop(IMPACT_S), 1.0);
        assert!(backdrop(2.4) > 0.0 && backdrop(2.4) < 1.0);
        assert_eq!(backdrop(END_S), 0.0);
    }

    #[test]
    fn cues_follow_the_blinks_then_sweep_and_impact() {
        let mut out = Vec::new();
        cues(4, &mut out);
        assert!(out.windows(2).all(|w| w[0].at_s <= w[1].at_s));
        // Each blip is a letter lighting up.
        for cue in &out {
            if let CueKind::Blip { rank: r, .. } = cue.kind {
                let index = (0..4).find(|&i| rank(i, 4) as u32 == r).unwrap();
                assert!(!is_lit(index, 4, cue.at_s - 0.001), "{cue:?}");
                assert!(is_lit(index, 4, cue.at_s + 0.001), "{cue:?}");
            }
        }
        let blips = out
            .iter()
            .filter(|c| matches!(c.kind, CueKind::Blip { .. }))
            .count();
        assert!(blips >= 4 * FLICKER.len());
        assert_eq!(
            out[out.len() - 2],
            Cue {
                at_s: GATHER_S,
                kind: CueKind::Sweep {
                    length_s: IMPACT_S - GATHER_S
                }
            }
        );
        assert_eq!(
            out.last(),
            Some(&Cue {
                at_s: IMPACT_S,
                kind: CueKind::Impact
            })
        );
        // The same every time.
        let mut again = Vec::new();
        cues(4, &mut again);
        assert_eq!(out, again);
    }

    #[test]
    fn intro_runs_from_the_first_tick_to_its_length() {
        let mut intro = Intro::new();
        assert!(!intro.is_done(5 * LENGTH_NS));
        assert!(!intro.music_due(5 * LENGTH_NS));
        intro.start(1_000 * MS);
        intro.start(1_500 * MS);
        let snap = intro.snapshot(1_500 * MS).unwrap();
        assert_eq!(snap.elapsed_ns, 500 * MS);
        assert_eq!(snap.exit, 0.0);
        assert!(snap.hides_menu_logo());
        assert!(!intro.music_due(1_000 * MS + IMPACT_NS - 1));
        assert!(intro.music_due(1_000 * MS + IMPACT_NS));
        assert!(!intro.is_done(1_000 * MS + LENGTH_NS - 1));
        assert!(intro.is_done(1_000 * MS + LENGTH_NS));
        assert_eq!(intro.snapshot(1_000 * MS + LENGTH_NS), None);
    }

    #[test]
    fn skip_fades_out_and_hands_the_logo_back() {
        let mut intro = Intro::new();
        intro.skip(0);
        assert!(!intro.is_skipped());
        intro.start(0);
        intro.skip(300 * MS);
        assert!(intro.music_due(300 * MS));
        let snap = intro.snapshot(400 * MS).unwrap();
        assert!((snap.exit - 0.5).abs() < 1e-6);
        assert!(!snap.hides_menu_logo());
        // A second skip does not restart the fade.
        intro.skip(450 * MS);
        assert!(intro.is_done(500 * MS));
        // Skipping a finished intro does nothing.
        let mut over = Intro::new();
        over.start(0);
        over.skip(LENGTH_NS);
        assert!(!over.is_skipped());
    }

    #[test]
    fn the_menu_is_revealed_when_the_black_starts_to_fade_or_at_a_skip() {
        let mut intro = Intro::new();
        assert_eq!(intro.reveal_at(), None);
        intro.start(1_000);
        assert_eq!(intro.reveal_at(), Some(1_000 + REVEAL_NS));
        // The black starts fading exactly then.
        assert!(backdrop(REVEAL_NS as f32 / 1e9 - 0.001) >= 1.0 - 1e-6);
        assert!(backdrop(REVEAL_NS as f32 / 1e9 + 0.05) < 1.0);
        // A skip reveals at once, and a late one does not push it later.
        intro.skip(1_000 + 300 * MS);
        assert_eq!(intro.reveal_at(), Some(1_000 + 300 * MS));
        let mut late = Intro::new();
        late.start(0);
        late.skip(REVEAL_NS + 50 * MS);
        assert_eq!(late.reveal_at(), Some(REVEAL_NS));
    }
}
