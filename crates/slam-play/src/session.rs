//! A play from start to end: the update side runs the game clock, live input, rules, health
//! and sounds and writes a [`PlaySnapshot`]; the draw side turns the snapshot into sprites.
//!
//! The update side is lazer's `Player` for one play: pausing and resuming, failing, passing
//! to the results, restarting and quitting, driven by the global actions of
//! [`crate::hotkeys`]. No breaks overlay or results screen yet ([`PlaySession::results`]),
//! only [`PlaySnapshot::is_break_time`] for the background dim; the HUD's numbers go out in
//! [`HudValues`].

use slam_audio::{ClockError, GameClock, MusicSnapshot, PositionSnapshot, TrackControl};
use slam_input::InputEvent;
use slam_osu::breaks::BreakTracker;
use slam_osu::hit_events::{HitErrorSummary, HitEvents};
use slam_osu::judgement::{HitResult, JudgementLog, Statistics};
use slam_osu::lead_in::{IntroSkip, earliest_start_time, gameplay_start_time};
use slam_osu::replay::{Actions, InputFrame, ReplayRecorder, Rules};
use slam_osu::rules::OsuRules;
use slam_osu::scoring::{ScoreProcessor, ScoreRank};
use slam_osu::{Beatmap, HealthProcessor, ModSet, Vec2};
use slam_render::atlas::AtlasRegion;
use slam_render::image::{Image, ImageLimits, decode_image};
use slam_render::sprite::SpriteBatch;
use slam_render::upload::{
    TextureHandle, TextureKind, TextureRequests, TextureState, TextureUploads,
};
use slam_render::{Color, Extent};
use slam_skin::{AnimationOptions, ImageCheck, SIXTY_FRAME_TIME, SkinChain, TextureFile};

use crate::circles::{
    CircleTextures, HitCircles, LiveCircles, OBJECT_SIZE, PieceTextures, SkinSprite,
};
use crate::cursor::{
    CursorFrame, CursorPath, CursorSkinConfig, CursorSources, CursorTextures, CursorView,
    ExpandTimes, ExpandTracker,
};
use crate::fail::{FailFrame, FailView, TrackEffects};
use crate::follow_points::{FollowPoints, MAX_FOLLOW_POINT_SIZE};
use crate::gameplay_sounds::{GameplaySounds, VoiceSink};
use crate::health_bar::{HealthTextures, MAX_FILL_FRAMES, MarkerTextures};
use crate::hit_error::{HitErrorRing, HitErrorTargets, MeterTextures, disc_image, gradient_image};
use crate::hotkeys::{GlobalAction, Hotkeys};
use crate::hud::{HudTextures, MAX_SKIP_FRAMES, SkipTextures};
use crate::input::{ClockTime, GameTime, LiveInput};
use crate::judgements::{
    JudgementRing, JudgementTextures, JudgementView, MAX_JUDGEMENT_FRAMES, PIECE_SLOTS, PieceKind,
    PieceSprites, PopupTargets, piece_sources,
};
use crate::key_overlay::{KEYS, KeyOverlayTextures, KeyValues};
use crate::legacy_hit_error::triangle_image;
use crate::legacy_text::{self, FontSprites};
use crate::menu_overlay::{
    GameplayMenu, MenuButton, MenuInfo, MenuKind, MenuSnapshot, ReplaySave, song_progress,
};
use crate::offset::{self, OffsetPanel};
use crate::playfield::PlayfieldLayout;
use crate::resume::ResumeView;
use crate::shake::ObjectShakes;
use crate::sliders::{
    AnimationSprites, BallStyle, HitSliders, LiveSliders, MAX_ANIMATION_FRAMES,
    MAX_FOLLOW_CIRCLE_AREA_SIZE, ObjectTextures, SliderBodies, SliderTextures, push_approaches,
    push_objects,
};
use crate::song_progress::ring_image;
use crate::spinners::{HitSpinners, LiveSpinners, SpinnerSpm, SpinnerTextures};
use crate::time::Offsets;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (PauseCooldownDuration)
/// Gameplay time after a pause before the next one is allowed, in milliseconds.
pub const PAUSE_COOLDOWN: f64 = 1000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (duration)
/// How long gameplay runs on after failing, in nanoseconds of real time, while the track
/// slows down to a stop ([`crate::fail`]).
pub const FAIL_ANIMATION_NS: u64 = crate::fail::FAIL_DURATION_NS;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (RESULTS_DISPLAY_DELAY)
/// How long gameplay runs on after passing before the results show, in nanoseconds of real
/// time (lazer schedules them on the player's clock, not the gameplay clock).
pub const RESULTS_DISPLAY_DELAY_NS: u64 = 1_000_000_000;

/// Gameplay time past the end of the last object the replay buffer has room for: the play
/// passes after the last object's hit window, and important frames can pile up there.
const RECORD_TAIL_MS: f64 = 30_000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/Cursor/OsuCursor.cs (SIZE)
/// Size of the click-to-resume cursor's hit area, in osu!pixels.
pub(crate) const RESUME_CURSOR_SIZE: f32 = 28.0;

/// Where a play is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    /// Waiting for the first output position report to start the clock.
    Waiting,
    Playing,
    /// Paused: the clock and gameplay stand still.
    Paused,
    /// Waiting for a click on the cursor where gameplay paused (lazer's `OsuResumeOverlay`).
    Resuming,
    /// Failed: gameplay runs on for the fail animation.
    Failing,
    /// Failed, and the clock stopped.
    Failed,
    /// Every judgement is in without failing (lazer's `HasPassed`): gameplay runs on until the
    /// results show, pausing is over.
    Passed,
    /// The results show: gameplay stopped, the track plays on.
    Results,
    /// The player quit: the play is over.
    Exited,
}

impl PlayState {
    /// Whether the OS cursor should be hidden: while lazer shows the gameplay cursor (the
    /// player's, or the resume overlay's to aim at the resume target). Paused, failed and on
    /// the results the OS cursor shows where lazer shows its menu cursor (there is no menu
    /// cursor of our own yet).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuResumeOverlay.cs (ProvidingUserCursor)
    pub fn hides_os_cursor(self) -> bool {
        matches!(
            self,
            PlayState::Waiting
                | PlayState::Playing
                | PlayState::Resuming
                | PlayState::Failing
                | PlayState::Passed
        )
    }
}

/// The numbers of the HUD: lazer's score processor as the HUD counters bind it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudValues {
    /// Total score with the mods' multiplier (`TotalScore`).
    pub score: i64,
    /// Accuracy, 0..=1 (`Accuracy`).
    pub accuracy: f64,
    /// Current combo (`Combo`).
    pub combo: u32,
    /// Judgements that are hits, ignored hits aside: each flashes the health bar's marker
    /// (`HealthDisplay.onNewJudgement`).
    pub hits: u32,
}

impl Default for HudValues {
    fn default() -> Self {
        HudValues {
            score: 0,
            accuracy: 1.0,
            combo: 0,
            hits: 0,
        }
    }
}

impl HudValues {
    /// The values of `score` now.
    pub fn of(score: &ScoreProcessor) -> HudValues {
        HudValues {
            score: score.total_score(),
            accuracy: score.accuracy(),
            combo: score.statistics().combo,
            hits: hits(score.statistics()),
        }
    }
}

/// The judgements of `statistics` that are hits, ignored hits aside.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/HUD/HealthDisplay.cs (onNewJudgement)
fn hits(statistics: &Statistics) -> u32 {
    HitResult::ALL
        .iter()
        .filter(|r| r.is_hit() && **r != HitResult::IgnoreHit)
        .map(|r| statistics.count(*r))
        .sum()
}

/// What a finished attempt's replay is made of: its input frames and final score. A passed
/// attempt is taken out of the session as it passes (lazer imports the score on
/// completion), a failed one when the fail menu's button saves it (ADR-0084).
#[derive(Debug)]
pub struct FinishedAttempt {
    recorder: ReplayRecorder,
    mods: ModSet,
    score: i64,
    score_without_mods: i64,
    accuracy: f64,
    rank: ScoreRank,
    statistics: Statistics,
    maximum_statistics: Statistics,
}

impl FinishedAttempt {
    /// An attempt slot recording into `recorder`, filled when an attempt passes or its fail
    /// is saved.
    fn spare(recorder: ReplayRecorder, mods: ModSet) -> FinishedAttempt {
        FinishedAttempt {
            recorder,
            mods,
            score: 0,
            score_without_mods: 0,
            accuracy: 1.0,
            rank: ScoreRank::F,
            statistics: Statistics::default(),
            maximum_statistics: Statistics::default(),
        }
    }

    /// The attempt's input as replay frames; `None` if the recording overflowed its buffer.
    pub fn frames(&self) -> Option<&[InputFrame]> {
        (!self.recorder.overflowed()).then(|| self.recorder.frames())
    }

    /// The mods played with (their settings; state such as Easy's lives used is not kept).
    pub fn mods(&self) -> &ModSet {
        &self.mods
    }

    /// Total score with the mods' multiplier.
    pub fn score(&self) -> i64 {
        self.score
    }

    /// Total score without the mods' multiplier.
    pub fn score_without_mods(&self) -> i64 {
        self.score_without_mods
    }

    /// Accuracy, 0..=1.
    pub fn accuracy(&self) -> f64 {
        self.accuracy
    }

    pub fn rank(&self) -> ScoreRank {
        self.rank
    }

    /// Counts per result and the highest combo.
    pub fn statistics(&self) -> &Statistics {
        &self.statistics
    }

    /// The counts and combo of a perfect play.
    pub fn maximum_statistics(&self) -> &Statistics {
        &self.maximum_statistics
    }
}

/// The final score of a passed play, for the results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayResults {
    /// Total score with the mods' multiplier.
    pub score: i64,
    /// Total score without the mods' multiplier.
    pub score_without_mods: i64,
    /// Accuracy, 0..=1.
    pub accuracy: f64,
    pub rank: ScoreRank,
    /// Counts per result and the highest combo.
    pub statistics: Statistics,
    /// The counts and combo of a perfect play, to show only the results the beatmap has.
    pub maximum_statistics: Statistics,
    /// Unstable rate and average and median hit error (ADR-0074).
    pub hit_errors: HitErrorSummary,
}

/// What the draw side needs of one update tick. Fixed size, copied into a buffer slot with
/// `clone_from` without allocations.
#[derive(Debug, Clone)]
pub struct PlaySnapshot {
    pub state: PlayState,
    /// Game time of the tick, in milliseconds.
    pub time: f64,
    /// The tick's clock, to place a frame between ticks. `None` while waiting.
    pub clock: Option<ClockTime>,
    /// Cursor position in osu!pixels.
    pub cursor: Vec2,
    /// The cursor positions gameplay took, for the trail (ADR-0042).
    pub cursor_path: CursorPath,
    /// When the cursor last expanded and contracted.
    pub cursor_expand: ExpandTimes,
    /// The judgements that show popups (ADR-0043).
    pub judgements: JudgementRing,
    /// The judgements the hit error meter shows.
    pub hit_errors: HitErrorRing,
    /// Whether the intro can be skipped now.
    pub skip_available: bool,
    /// Whether gameplay is in a break (lazer's `IsBreakTime`): the background dims less.
    pub is_break_time: bool,
    /// Health, 0..=1.
    pub health: f64,
    pub hud: HudValues,
    /// The key overlay's keys (ADR-0113).
    pub keys: KeyValues,
    /// Where to click to resume, in osu!pixels ([`PlayState::Resuming`]).
    pub resume_target: Vec2,
    /// The cursor the window has while gameplay's stands still, in osu!pixels: the resume
    /// overlay's own cursor ([`PlayState::Resuming`]).
    pub resume_cursor: Vec2,
    /// The positions of `resume_cursor`, for its trail.
    pub resume_cursor_path: CursorPath,
    /// Times the play went into [`PlayState::Resuming`]: each has a new cursor.
    pub resumes: u32,
    /// How far quick retry is held, 0..=1.
    pub retry_progress: f32,
    /// How far quick exit is held, 0..=1.
    pub exit_progress: f32,
    /// Times the play was restarted.
    pub restarts: u32,
    /// The pause or fail menu.
    pub menu: MenuSnapshot,
    /// When the play failed: the fail animation runs from then on.
    pub fail: Option<FailView>,
    /// The beatmap offset and its panel.
    pub offset_panel: OffsetPanel,
    pub circles: LiveCircles,
    pub sliders: LiveSliders,
    pub spinners: LiveSpinners,
    /// The circles and sliders shaking after blocked presses.
    pub shakes: ObjectShakes,
}

impl Default for PlaySnapshot {
    fn default() -> Self {
        PlaySnapshot {
            state: PlayState::Waiting,
            time: 0.0,
            clock: None,
            cursor: Vec2::ZERO,
            cursor_path: CursorPath::default(),
            cursor_expand: ExpandTimes::default(),
            judgements: JudgementRing::default(),
            hit_errors: HitErrorRing::default(),
            skip_available: false,
            // Lazer's `BreakTracker` starts in a break.
            is_break_time: true,
            health: 1.0,
            hud: HudValues::default(),
            keys: KeyValues::default(),
            resume_target: Vec2::ZERO,
            resume_cursor: Vec2::ZERO,
            resume_cursor_path: CursorPath::default(),
            resumes: 0,
            retry_progress: 0.0,
            exit_progress: 0.0,
            restarts: 0,
            menu: MenuSnapshot::default(),
            fail: None,
            offset_panel: OffsetPanel::default(),
            circles: LiveCircles::new(),
            sliders: LiveSliders::new(),
            spinners: LiveSpinners::new(),
            shakes: ObjectShakes::default(),
        }
    }
}

impl PlaySnapshot {
    /// Game time at engine time `now_ns` (a frame): the clock's time, not before the tick's.
    pub fn time_at(&self, now_ns: u64) -> f64 {
        self.clock
            .map_or(self.time, |clock| clock.game_time(now_ns).max(self.time))
    }
}

/// Where the session is; [`PlayState`] without the derived states.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Playing,
    Paused,
    Resuming,
    Failing { until_ns: u64 },
    Failed,
    Passed { results_at_ns: u64 },
    Results,
    Exited,
}

/// The state a restart goes back to.
#[derive(Debug)]
struct Initial {
    rules: OsuRules,
    health: HealthProcessor,
    score: ScoreProcessor,
    mods: ModSet,
    skip: IntroSkip,
}

/// The update side of a play.
#[derive(Debug)]
pub struct PlaySession {
    clock: GameClock,
    offsets: Offsets,
    start_time: f64,
    /// Lazer's `GameplayStartTime`: the offset can change for a while after it.
    gameplay_start_time: f64,
    /// Whether hotkeys may change the beatmap offset (not in a replay).
    offset_adjustable: bool,
    offset_panel: OffsetPanel,
    breaks: BreakTracker,
    skip: IntroSkip,
    /// Skip the intro as soon as it can be (a quick restart).
    auto_skip: bool,
    /// A seek the clock still has to take; retried every tick until it does.
    pending_seek: Option<f64>,
    /// Whether the clock should run; commands that fail are retried every tick.
    run: bool,
    started: bool,
    phase: Phase,
    input: LiveInput,
    hotkeys: Hotkeys,
    rules: OsuRules,
    /// The spins per minute of the alive spinners, counted after each step.
    spm: SpinnerSpm,
    /// The cursor's expansion by the left and right buttons.
    expand: ExpandTracker,
    log: JudgementLog,
    /// Which judgements show popups.
    popup_targets: PopupTargets,
    /// The judgements that showed popups, for the draw side; kept across restarts.
    popups: JudgementRing,
    /// The objects shaking after blocked presses.
    shakes: ObjectShakes,
    /// Judgements of the log looked at for popups.
    popups_seen: usize,
    /// Which judgements the hit error meter shows.
    hit_error_targets: HitErrorTargets,
    /// The judgements the meter showed, for the draw side; kept across restarts.
    hit_errors: HitErrorRing,
    /// The hit errors the play counts, for the results (ADR-0074).
    hit_events: HitEvents,
    health: HealthProcessor,
    score: ScoreProcessor,
    /// The mods' state for the health processor (lives of Easy).
    mods: ModSet,
    /// Judgements of the log the health and score processors took.
    applied: usize,
    /// Judgements made before failing: those that count.
    failed_at: Option<usize>,
    sounds: GameplaySounds,
    /// Gameplay time of the last pause.
    last_pause_time: Option<f64>,
    resume_target: Vec2,
    /// Times the play went into resuming.
    resumes: u32,
    /// Time of the last gameplay step: lazer's gameplay clock time, which stands still
    /// while paused and seeking.
    last_time: f64,
    /// A mod restarts the play on failing: at the end of the tick.
    restart_on_fail: bool,
    /// Every judgement is in without failing (lazer's `HasPassed`); stays after quitting.
    passed: bool,
    restarts: u32,
    /// The overlay keys' presses counted this attempt, and the presses of the input seen.
    key_counts: [u32; KEYS],
    key_presses_seen: [u32; KEYS],
    /// The pause or fail menu.
    menu: GameplayMenu,
    /// The menu's retry was pressed: the play restarts at the end of the tick.
    menu_restart: bool,
    /// Start of the first object and the latest end of an object, for the song progress.
    playable: (f64, f64),
    initial: Initial,
    /// Clock commands that failed (queue full); retried on the next tick.
    clock_errors: u64,
    /// What the track should sound like, and what it was last given (`None` before the
    /// first tick: the previous play may have left it failing).
    track_effects: TrackEffects,
    applied_effects: Option<TrackEffects>,
    /// The rate mods' speed (ADR-0055): the clock's rate, the track's tempo and its
    /// frequency, set once the clock takes commands.
    speed: Speed,
    speed_set: bool,
    /// When the play failed.
    fail: Option<FailView>,
    /// The play's input as replay frames, until it passes or its fail is saved.
    recorder: ReplayRecorder,
    /// The slot a finished attempt's frames and score move to, made ahead with its own
    /// recorder to swap in; `None` once taken, until the next restart makes another.
    spare_attempt: Option<Box<FinishedAttempt>>,
    /// The attempt that passed or whose fail was saved, until
    /// [`PlaySession::take_finished_attempt`].
    finished_attempt: Option<Box<FinishedAttempt>>,
    /// The fail menu's button saved this attempt's replay.
    failed_replay_saved: bool,
}

/// How fast a play runs: lazer's track adjustments from the rate mods.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Speed {
    /// The gameplay clock's rate (lazer's `GetTrueGameplayRate`).
    rate: f64,
    /// The track's tempo, pitch kept.
    tempo: f64,
    /// The track's frequency, with the pitch.
    frequency: f64,
}

impl Speed {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Mods/ModRateAdjust.cs (ApplyToTrack)
    fn of(mods: &ModSet) -> Speed {
        let change = mods.rate();
        Speed {
            rate: mods.true_gameplay_rate(),
            tempo: change.rate / change.frequency,
            frequency: change.frequency,
        }
    }
}

impl PlaySession {
    /// A play of `beatmap` (with `mods` applied) on the track `track` controls. Call it off
    /// the hot paths.
    pub fn new(
        beatmap: &Beatmap,
        mods: &ModSet,
        track: &impl TrackControl,
        input: LiveInput,
        sounds: GameplaySounds,
        offsets: Offsets,
    ) -> PlaySession {
        let rules = OsuRules::new(beatmap, mods);
        let log = JudgementLog::with_capacity(rules.max_judgements());
        let health = HealthProcessor::for_play(beatmap, mods);
        let score = ScoreProcessor::for_play(beatmap, mods);
        // A play whose rules judge fewer times than the simulated perfect play never passes.
        debug_assert_eq!(
            score.max_hits(),
            rules.max_judgements(),
            "judgement counts differ"
        );
        // No storyboards yet.
        let start_time = earliest_start_time(beatmap, None);
        let gameplay_start_time = gameplay_start_time(beatmap);
        // The skip overlay appears with the clock at the start.
        let skip = IntroSkip::new(gameplay_start_time, start_time, start_time);
        let breaks = BreakTracker::new(&beatmap.breaks, gameplay_start_time);
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Beatmaps/IBeatmap.cs (CalculatePlayableBounds)
        let playable = match beatmap.hit_objects.first() {
            Some(first) => (
                first.start_time,
                beatmap
                    .hit_objects
                    .iter()
                    .map(|object| object.end_time())
                    .fold(f64::NEG_INFINITY, f64::max),
            ),
            None => (0.0, 0.0),
        };
        let speed = Speed::of(mods);
        // Room for frames from the start to past the last object, where the play passes.
        let recorder = ReplayRecorder::new(
            playable.1 - start_time + RECORD_TAIL_MS,
            speed.rate,
            rules.max_judgements(),
        );
        PlaySession {
            clock: GameClock::new(track),
            offsets,
            start_time,
            gameplay_start_time,
            offset_adjustable: true,
            offset_panel: OffsetPanel::new(offsets.beatmap_ms),
            breaks,
            skip,
            auto_skip: false,
            pending_seek: None,
            run: false,
            started: false,
            phase: Phase::Playing,
            input,
            hotkeys: Hotkeys::default(),
            initial: Initial {
                rules: rules.clone(),
                health: health.clone(),
                score: score.clone(),
                mods: mods.clone(),
                skip,
            },
            rules,
            spm: SpinnerSpm::new(beatmap),
            expand: ExpandTracker::default(),
            log,
            popup_targets: PopupTargets::new(beatmap, mods),
            popups: JudgementRing::default(),
            shakes: ObjectShakes::default(),
            popups_seen: 0,
            hit_error_targets: HitErrorTargets::new(beatmap),
            hit_errors: HitErrorRing::default(),
            hit_events: HitEvents::new(beatmap, mods.true_gameplay_rate()),
            health,
            score,
            mods: mods.clone(),
            applied: 0,
            failed_at: None,
            sounds,
            last_pause_time: None,
            resume_target: Vec2::ZERO,
            resumes: 0,
            last_time: start_time,
            restart_on_fail: false,
            passed: false,
            restarts: 0,
            key_counts: [0; KEYS],
            key_presses_seen: [0; KEYS],
            menu: GameplayMenu::default(),
            menu_restart: false,
            playable,
            clock_errors: 0,
            track_effects: TrackEffects::Neutral,
            applied_effects: None,
            speed,
            speed_set: false,
            fail: None,
            spare_attempt: Some(Box::new(FinishedAttempt::spare(
                recorder.spare(),
                mods.clone(),
            ))),
            finished_attempt: None,
            failed_replay_saved: false,
            recorder,
        }
    }

    /// Lays the pause and fail menus out at the pause menu's scale (ADR-0122).
    pub fn set_menu_scale(&mut self, ui_scale: f32) {
        self.menu.set_ui_scale(ui_scale);
    }

    /// Draws the pause and fail menus in `style`, SLAM's in `scheme`'s colours, stable's
    /// with buttons of the display sizes `stable` (ADR-0122).
    pub fn set_menu_style(
        &mut self,
        style: crate::menu_overlay::MenuStyle,
        scheme: slam_ui::theme::ColourScheme,
        stable: [[f32; 2]; 3],
    ) {
        self.menu.set_style(style, scheme, stable);
    }

    /// Turns the menus' glow and sheen on or off (ADR-0123).
    pub fn set_menu_effects(&mut self, effects: bool) {
        self.menu.set_effects(effects);
    }

    /// Game time the clock starts at: 0, or earlier for the first object's fade-in and the
    /// beatmap's lead-in ([`earliest_start_time`]).
    pub fn start_time(&self) -> f64 {
        self.start_time
    }

    pub fn log(&self) -> &JudgementLog {
        &self.log
    }

    /// The judgements that count: all of them, or those made before failing (lazer's score
    /// processor ignores judgements after a fail).
    pub fn counted_judgements(&self) -> usize {
        self.failed_at.unwrap_or(self.log.events().len())
    }

    pub fn health(&self) -> &HealthProcessor {
        &self.health
    }

    pub fn score(&self) -> &ScoreProcessor {
        &self.score
    }

    /// The mods the play started with.
    pub fn mods(&self) -> &ModSet {
        &self.initial.mods
    }

    pub fn input(&self) -> &LiveInput {
        &self.input
    }

    /// Times the play was restarted.
    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    /// Whether the play passed: its results wait to show, show, or were left.
    pub fn passed(&self) -> bool {
        self.passed
    }

    /// The final score once the play passed: while the results wait to show, show, and
    /// after quitting them.
    pub fn results(&self) -> Option<PlayResults> {
        self.passed.then(|| PlayResults {
            score: self.score.total_score(),
            score_without_mods: self.score.total_score_without_mods(),
            accuracy: self.score.accuracy(),
            rank: self.score.rank(&self.initial.mods),
            statistics: *self.score.statistics(),
            maximum_statistics: *self.score.maximum_statistics(),
            hit_errors: self.hit_events.summary(),
        })
    }

    /// The hit error statistics of the attempt so far, passed or not. Allocates for the
    /// median.
    pub fn hit_error_summary(&self) -> HitErrorSummary {
        self.hit_events.summary()
    }

    /// The attempt that passed, or whose fail the fail menu saved, once: its replay is saved
    /// right away, so that quitting before the results or retrying loses nothing. Without
    /// allocating.
    pub fn take_finished_attempt(&mut self) -> Option<Box<FinishedAttempt>> {
        self.finished_attempt.take()
    }

    /// Whether the player quit.
    pub fn has_exited(&self) -> bool {
        self.phase == Phase::Exited
    }

    /// Clock commands that failed and were retried.
    pub fn clock_errors(&self) -> u64 {
        self.clock_errors
    }

    /// The beatmap offset, milliseconds: as the play started, or as hotkeys changed it.
    pub fn beatmap_offset(&self) -> f64 {
        self.offsets.beatmap_ms
    }

    /// Whether hotkeys may change the beatmap offset; they may unless turned off (a
    /// replay, which lazer plays without the offset control).
    pub fn set_offset_adjustable(&mut self, adjustable: bool) {
        self.offset_adjustable = adjustable;
    }

    /// Changes the playfield layout of the input (the window was resized).
    pub fn set_layout(&mut self, layout: PlayfieldLayout) {
        self.input.set_layout(layout);
    }

    /// One update tick at engine time `now_ns` with the tick's input `events` and the latest
    /// reads of the output and music positions. Overwrites all of `snapshot`.
    ///
    /// Allocation-free, except on a restart, which builds the play anew as lazer loads a new
    /// player.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        now_ns: u64,
        events: &[InputEvent],
        output: Option<PositionSnapshot>,
        music: Option<MusicSnapshot>,
        track: &mut impl TrackControl,
        sink: &mut impl VoiceSink,
        snapshot: &mut PlaySnapshot,
    ) {
        let clock = self.clock.update(now_ns, output, music);
        let clock = match clock {
            Some(clock) if self.started => clock,
            Some(_) => {
                // The first tick with a position: the clock can take commands. Input before
                // the play starts is dropped.
                self.pending_seek = Some(self.start_time);
                self.run = true;
                match self.sync_clock(now_ns, track) {
                    Ok(()) => self.started = true,
                    Err(_) => self.clock_errors += 1,
                }
                self.sync_track(track);
                return self.waiting(events, snapshot);
            }
            None => return self.waiting(events, snapshot),
        };

        let mut time = ClockTime {
            clock,
            offsets: self.offsets,
        };
        // A seek in the track is heard one output latency later; until then time is the old
        // position, which must not reach the rules (a restart would miss every object).
        let seeking = self.clock.is_seeking();

        // Global actions take effect between the events around them: the events before go
        // to gameplay (or are followed while it is suspended), those after see the new state.
        let mut skip_requested = false;
        let mut from = 0;
        for (index, event) in events.iter().enumerate() {
            // Global bindings see mouse buttons even when gameplay ignores them: lazer's
            // prioritised `GlobalActionContainer` gets them before `RulesetInputManager`.
            let action = self.hotkeys.on_event(event);
            self.menu.follow_pointer(event);
            if self.input.is_suspended() {
                let target = self.resume_target;
                let resuming = self.phase == Phase::Resuming;
                let resumed = self
                    .input
                    .follow_suspended(std::slice::from_ref(event), |actions, at| {
                        resuming
                            && (actions.contains(Actions::LEFT) || actions.contains(Actions::RIGHT))
                            && hovers(target, at)
                    })
                    .is_some();
                from = index + 1;
                if resumed {
                    self.complete_resume();
                    continue;
                }
            }
            // The menu shows only while gameplay is suspended; what it takes reaches
            // nothing else (Space selects there rather than skipping).
            let key = self.hotkeys.menu_key(event);
            let size = self.input.layout().window_size();
            let (clicked, taken) = self.menu.on_event(event, key, size);
            if let Some(button) = clicked {
                self.press_menu(button, event.time_ns, sink);
            }
            if taken {
                continue;
            }
            match action {
                Some(GlobalAction::Back | GlobalAction::PauseGameplay) => {
                    if self.in_gameplay() {
                        self.step(&events[from..index], now_ns, &time, seeking, sink);
                        from = index + 1;
                    }
                    self.back(event.time_ns, sink);
                }
                Some(GlobalAction::SkipCutscene) => skip_requested = true,
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/OsuGame.cs (OnPressed)
                Some(GlobalAction::ToggleGameplayMouseButtons) => {
                    if self.in_gameplay() {
                        self.step(&events[from..index], now_ns, &time, seeking, sink);
                        from = index + 1;
                    }
                    let disabled = self.input.mouse_buttons_disabled();
                    self.input.set_mouse_buttons_disabled(!disabled);
                }
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PlayerSettings/BeatmapOffsetControl.cs (OnPressed)
                Some(action @ (GlobalAction::IncreaseOffset | GlobalAction::DecreaseOffset)) => {
                    // Paused, resuming and after the play the gameplay clock is stopped. Without
                    // the control (a replay) the key is not taken, as lazer has nothing to
                    // take it; with it, the key is taken even when the offset cannot change.
                    if !self.offset_adjustable || !self.in_gameplay() {
                        continue;
                    }
                    // The events before play at the old offset, those after at the new.
                    self.step(&events[from..index], now_ns, &time, seeking, sink);
                    from = index + 1;
                    // Lazer checks the clock of the previous frame; this tick's time up to
                    // the press differs by less than a tick at the window's end.
                    let is_break_time = self.is_break_time(self.last_time);
                    if !offset::may_adjust(self.last_time, self.gameplay_start_time, is_break_time)
                    {
                        continue;
                    }
                    let delta = if action == GlobalAction::IncreaseOffset {
                        offset::STEP_MS
                    } else {
                        -offset::STEP_MS
                    };
                    let beatmap_ms = offset::adjusted(self.offsets.beatmap_ms, delta);
                    // A bindable notifies only when its value changes.
                    if beatmap_ms != self.offsets.beatmap_ms {
                        self.offsets.beatmap_ms = beatmap_ms;
                        time.offsets = self.offsets;
                        self.offset_panel.changed(beatmap_ms, event.time_ns);
                    }
                }
                _ => {}
            }
        }
        // Time moves with the offset at once (lazer's offset clock).
        let current = time.game_time(now_ns);
        if self.in_gameplay() {
            self.step(&events[from..], now_ns, &time, seeking, sink);
        }

        if let Phase::Failing { until_ns } = self.phase
            && now_ns >= until_ns
        {
            self.complete_fail(now_ns, sink, false);
        }
        if let Phase::Passed { results_at_ns } = self.phase
            && now_ns >= results_at_ns
        {
            self.show_results(sink);
        }

        let mut restart = std::mem::take(&mut self.restart_on_fail);
        let menu_restart = std::mem::take(&mut self.menu_restart);
        match self.hotkeys.fire(now_ns) {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (HotkeyRetryOverlay, HotkeyExitOverlay)
            Some(GlobalAction::QuickRetry) => restart = true,
            // The results screen quits on quick exit too (`ResultsScreen.OnPressed`), on the
            // press there rather than after the hold.
            Some(GlobalAction::QuickExit) => self.perform_exit(sink),
            _ => {}
        }
        if !self.in_gameplay() {
            // Stops of looping samples that did not queue are retried while gameplay stands.
            self.sounds.stop_all(sink);
        }

        if (restart || menu_restart) && self.phase != Phase::Exited {
            // Lazer's quick restarts skip the intro (`AutomaticallySkipIntro`); the menu's
            // retry does not.
            self.restart(sink, restart);
        } else if self.phase == Phase::Playing && !seeking {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (PerformIntroSkip)
            // The skip period is checked at the tick's time; lazer checks the button enabled
            // by the previous frame, which differs only on the period's last millisecond.
            if (skip_requested || self.auto_skip)
                && let Some(target) = self.skip.target(current, false)
            {
                // Lazer stops the samples by disabling their playback around the seek.
                // Nothing loops before gameplay starts; a stop that did not queue is not
                // retried.
                self.sounds.stop_all(sink);
                self.pending_seek = Some(target);
            }
            self.auto_skip = false;
        }

        // The clock moves on the next tick at the earliest: a seek sent now hides the skip.
        let seek_sent = self.pending_seek.is_some();
        if self.sync_clock(now_ns, track).is_err() {
            self.clock_errors += 1;
        }
        self.sync_track(track);

        snapshot.state = match self.phase {
            Phase::Playing => PlayState::Playing,
            Phase::Paused => PlayState::Paused,
            Phase::Resuming => PlayState::Resuming,
            Phase::Failing { .. } => PlayState::Failing,
            Phase::Failed => PlayState::Failed,
            Phase::Passed { .. } => PlayState::Passed,
            Phase::Results => PlayState::Results,
            Phase::Exited => PlayState::Exited,
        };
        snapshot.time = self.last_time;
        // While a seek is on its way the clock shows the old time: frames hold the tick's.
        // The results stop gameplay while the track plays on: frames hold the tick's time.
        let restart = restart || menu_restart;
        snapshot.clock =
            (!restart && !seek_sent && !self.clock.is_seeking() && self.phase != Phase::Results)
                .then_some(time);
        snapshot.cursor = self.input.position();
        snapshot.cursor_path.clone_from(self.input.path());
        snapshot.cursor_expand = self.expand.times();
        snapshot.skip_available = self.phase == Phase::Playing
            && !seek_sent
            && !self.clock.is_seeking()
            && self.skip.is_available(current);
        snapshot.is_break_time = self.is_break_time(self.last_time);
        snapshot.offset_panel = self.offset_panel;
        self.fill(snapshot, now_ns);
    }

    /// Stops the looping sounds (the play is closing). Returns whether all stops were
    /// queued; call again until it does.
    pub fn stop_sounds(&mut self, sink: &mut impl VoiceSink) -> bool {
        self.sounds.stop_all(sink)
    }

    /// Takes the samples of `sounds`, made for this play's beatmap and mods with another
    /// skin or other settings, and leaves it this play's old ones to free off the update
    /// thread (ADR-0096); see [`GameplaySounds::replace_samples`]. Does not allocate.
    pub fn replace_samples(
        &mut self,
        sounds: &mut GameplaySounds,
        sink: &mut impl VoiceSink,
    ) -> bool {
        self.sounds.replace_samples(sounds, sink)
    }

    /// Follows `events` another screen takes while it covers the pause or fail menu (the
    /// settings panel, ADR-0095), acting on none of them: the keys and buttons held and the
    /// pointer, as lazer's input state goes on tracking them under an overlay. Without it, a
    /// modifier or tap key released over the panel stays held once it closes (#324), and the
    /// menu's next click lands where the pointer was before the panel opened.
    /// Allocation-free.
    pub fn follow_covered(&mut self, events: &[InputEvent]) {
        let size = self.input.layout().window_size();
        for event in events {
            self.hotkeys.follow(event);
            self.menu.follow_pointer(event);
            // Hover follows the pointer, as osu-framework finds it again once the panel
            // is gone; nothing else reaches the menu.
            if matches!(event.kind, slam_input::InputKind::MouseMove { .. }) {
                let _ = self.menu.on_event(event, None, size);
            }
        }
        if self.input.is_suspended() {
            // Nothing resumes from under the panel.
            let _ = self.input.follow_suspended(events, |_, _| false);
        }
    }

    /// Whether gameplay takes input and time: playing, failing or passed.
    fn in_gameplay(&self) -> bool {
        matches!(
            self.phase,
            Phase::Playing | Phase::Failing { .. } | Phase::Passed { .. }
        )
    }

    /// One gameplay step with `events`: input, health drain, rules, sounds, then the new
    /// judgements to the health processor. While the clock is seeking, the events wait.
    fn step(
        &mut self,
        events: &[InputEvent],
        now_ns: u64,
        time: &ClockTime,
        seeking: bool,
        sink: &mut impl VoiceSink,
    ) {
        if seeking {
            self.input.hold(events);
            return;
        }
        let step = self.input.step(events, now_ns, time);
        let step_time = step.time;
        self.expand.apply(step.events);
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (LoadComponents)
        // The health processor is a frame-stable component ahead of the playfield: it drains
        // first, then takes the step's judgements as they come.
        self.health.update(step_time);
        let judged_before = self.log.events().len();
        self.rules.update(&step, &mut self.log);
        for shake in self.rules.shakes() {
            self.shakes.start(shake.index, shake.time);
        }
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/UI/DrawableRuleset.cs (SetRecordTarget)
        // Recording stops once the play completes (lazer's `SetRecordTarget(null)`); every
        // judgement records an important frame.
        if !self.passed {
            let judged = self.log.events().len() > judged_before;
            self.recorder.record_step(&step, judged);
        }
        // Lazer's spinners count their rate after their children, in the playfield's update.
        self.spm.update(&self.rules, step_time);
        self.sounds.update(step_time, &self.log, &self.rules, sink);
        for judgement in self.log.events_since(self.popups_seen) {
            if let Some(shown) = self.popup_targets.shown(judgement) {
                self.popups.push(shown);
            }
            if let Some(error) = self.hit_error_targets.error(judgement) {
                self.hit_errors.push(error);
            }
        }
        self.popups_seen = self.log.events().len();
        // Ported from McOsu db2add2: src/App/Osu/OsuBeatmap.cpp (keyPressed1, keyPressed2)
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (onBreakTimeChanged)
        // Presses count while playing, outside breaks (lazer's `IsBreakTime`, the intro
        // before the first object included) and the skippable intro (McOsu's
        // `m_bIsInSkippableSection`), to the tick: a step's presses go by the state at the
        // previous step's time, the one the break tracker was last asked about (ADR-0113).
        let (presses, _) = self.input.keys();
        let counting = self.phase == Phase::Playing
            && !self.skip.is_available(self.last_time)
            && !self.is_break_time(self.last_time);
        if counting {
            for ((count, &now), seen) in self
                .key_counts
                .iter_mut()
                .zip(&presses)
                .zip(&self.key_presses_seen)
            {
                *count = count.wrapping_add(now.wrapping_sub(*seen));
            }
        }
        self.key_presses_seen = presses;
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (LoadComponents)
        // Each judgement goes to the health processor, then to the score processor.
        for (index, judgement) in self.log.events().iter().enumerate().skip(self.applied) {
            let failed_before = self.health.has_failed();
            self.health.apply(judgement, &mut self.mods);
            let old_combo = self.score.statistics().combo;
            self.score.apply(judgement, failed_before);
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (createGameplayComponents)
            // `ComboEffects` follows the score processor's combo, at the gameplay clock's time.
            let new_combo = self.score.statistics().combo;
            self.sounds
                .combo_changed(old_combo, new_combo, step_time, sink);
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/ScoreProcessor.cs (ApplyResultInternal)
            // Judgements after the fail make no hit events.
            if !failed_before {
                self.hit_events.apply(judgement);
            }
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Rulesets/Scoring/HealthProcessor.cs (ApplyResultInternal)
            // The judgement that fails still counts; later ones are `FailedAtJudgement`.
            if self.failed_at.is_none() && self.health.has_failed() {
                self.failed_at = Some(index + 1);
            }
        }
        self.applied = self.log.events().len();
        if self.health.has_failed() {
            self.score.fail();
        }
        self.last_time = step_time;

        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (onFail, PerformFail)
        // Failing comes before anything the player does after this step (a pause).
        if self.phase == Phase::Playing && self.health.has_failed() {
            if self.mods.restart_on_fail() {
                self.restart_on_fail = true;
            } else {
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
                self.phase = Phase::Failing {
                    until_ns: now_ns.saturating_add(FAIL_ANIMATION_NS),
                };
                self.track_effects = TrackEffects::Failing;
                self.sounds.play_fail(sink);
                self.fail = Some(FailView {
                    time: step_time,
                    at_ns: now_ns,
                    cut: false,
                });
            }
        }

        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (checkScoreCompleted, progressToResults)
        // The score processor completes on its update at the step's time; a fail in the same
        // step (on the last judgement) comes first and there are no results. Without
        // rewinding, completion is never cancelled; without storyboards, nothing waits for
        // one to end. Lazer's processor updates ahead of the playfield, on the next step;
        // checked here after the step's judgements, a last judgement by a press before the
        // step's time passes one step earlier.
        if self.phase == Phase::Playing
            && !self.restart_on_fail
            && self.score.has_completed(step_time)
        {
            self.phase = Phase::Passed {
                results_at_ns: now_ns.saturating_add(RESULTS_DISPLAY_DELAY_NS),
            };
            self.passed = true;
            self.keep_attempt();
        }
    }

    /// Moves the attempt that just passed, or failed, into its slot; the session keeps the
    /// slot's empty buffer for the next attempt. Without allocating: the slot was made ahead.
    /// Whether there was a slot.
    fn keep_attempt(&mut self) -> bool {
        let Some(mut attempt) = self.spare_attempt.take() else {
            return false;
        };
        std::mem::swap(&mut attempt.recorder, &mut self.recorder);
        attempt.score = self.score.total_score();
        attempt.score_without_mods = self.score.total_score_without_mods();
        attempt.accuracy = self.score.accuracy();
        attempt.rank = self.score.rank(&self.initial.mods);
        attempt.statistics = *self.score.statistics();
        attempt.maximum_statistics = *self.score.maximum_statistics();
        // An attempt nobody took becomes the next spare, without freeing it here.
        if let Some(mut old) = self.finished_attempt.replace(attempt) {
            old.recorder.clear();
            self.spare_attempt = Some(old);
        }
        true
    }

    /// What the fail menu's save button can do now (ADR-0084).
    fn replay_save(&self) -> ReplaySave {
        if self.failed_replay_saved {
            ReplaySave::Saved
        } else if self.spare_attempt.is_none() || self.recorder.overflowed() {
            ReplaySave::Unavailable
        } else {
            ReplaySave::Available
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/SaveFailedScoreButton.cs (load)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (prepareAndImportScoreAsync)
    /// The fail menu's save button: the failed attempt (rank F) moves to its slot, once.
    /// Recording ran through the fail animation and stopped with it (lazer's clock stops in
    /// `onFailComplete`).
    fn save_failed_replay(&mut self) {
        if self.phase != Phase::Failed || self.replay_save() != ReplaySave::Available {
            return;
        }
        if self.keep_attempt() {
            self.failed_replay_saved = true;
            self.menu.set_replay_save(ReplaySave::Saved);
        }
    }

    /// Back or pause-gameplay pressed: lazer's `PerformExitWithConfirmation` from the hold-for-
    /// menu button, or the pause overlay's first button (continue) while paused.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (PerformExitWithConfirmation)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PauseOverlay.cs (BackAction, OnPressed)
    fn back(&mut self, now_ns: u64, sink: &mut impl VoiceSink) {
        match self.phase {
            // A restart on failing is on its way: nothing to pause.
            Phase::Playing if self.restart_on_fail => {}
            Phase::Playing | Phase::Resuming => self.pause(now_ns, sink),
            // The menu's back button: continue when paused, quit when failed.
            // Without a menu, a button of it was pressed this tick (retry): it goes on.
            Phase::Paused | Phase::Failed => {
                if let Some(kind) = self.menu.kind() {
                    self.press_menu(kind.back_button(), now_ns, sink);
                }
            }
            // The fail animation is cut short; the fail overlay shows.
            Phase::Failing { .. } => self.complete_fail(now_ns, sink, true),
            // Passed, pausing is over (`ValidForResume` is false): the exit goes on.
            Phase::Passed { .. } | Phase::Results => self.perform_exit(sink),
            Phase::Exited => {}
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (AddButton)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (PauseOverlay, FailOverlay)
    /// A button of the menu pressed at `now_ns`: its action, then the menu hides.
    fn press_menu(&mut self, button: MenuButton, now_ns: u64, sink: &mut impl VoiceSink) {
        // The menu shows only while paused or failed; quitting hides it.
        if !matches!(self.phase, Phase::Paused | Phase::Failed) {
            return;
        }
        match button {
            // The save button is in the fail menu's footer: the menu stays.
            MenuButton::SaveReplay => {
                self.save_failed_replay();
                return;
            }
            MenuButton::Continue => self.resume(),
            MenuButton::Retry => self.menu_restart = true,
            // The menu shows: `PerformExitWithConfirmation` exits at once.
            MenuButton::Quit => self.perform_exit(sink),
        }
        self.menu.hide(now_ns);
    }

    /// The menu's information now: lazer updates it as the menu shows.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/GameplayMenuOverlay.cs (updateInfoText)
    fn menu_info(&self) -> MenuInfo {
        MenuInfo {
            retries: self.restarts,
            progress: song_progress(self.last_time, self.playable.0, self.playable.1),
            accuracy: self.score.accuracy(),
        }
    }

    fn show_menu(&mut self, kind: MenuKind, now_ns: u64) {
        let info = self.menu_info();
        let size = self.input.layout().window_size();
        self.menu.show(kind, info, now_ns, size);
    }

    /// The player asks to quit: lazer's `PerformExit` (or the results screen's exit).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (PerformExit)
    fn perform_exit(&mut self, sink: &mut impl VoiceSink) {
        match self.phase {
            // Matching stable, an exit while the results wait shows them instead.
            Phase::Passed { .. } => self.show_results(sink),
            _ => self.exit(sink),
        }
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (progressToResults, OnSuspending)
    /// The results screen takes over: gameplay stops (the player screen no longer updates),
    /// the track plays on.
    fn show_results(&mut self, sink: &mut impl VoiceSink) {
        self.phase = Phase::Results;
        self.sounds.stop_all(sink);
        self.input.suspend();
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (Pause, PauseCooldownActive)
    fn pause(&mut self, now_ns: u64, sink: &mut impl VoiceSink) {
        let current = self.last_time;
        let resuming = self.phase == Phase::Resuming;
        // The cooldown counts while playing outside breaks.
        let cooldown = self.phase == Phase::Playing
            && !self.is_break_time(current)
            && self
                .last_pause_time
                .is_some_and(|last| current < last + PAUSE_COOLDOWN);
        if !resuming && cooldown {
            return;
        }
        self.phase = Phase::Paused;
        self.run = false;
        self.last_pause_time = Some(current);
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/PausableSkinnableSound.cs (SamplePlaybackDisabledChanged)
        // Looping samples stop; the others play out.
        self.sounds.stop_all(sink);
        self.input.suspend();
        self.show_menu(MenuKind::Pause, now_ns);
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (Resume)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuResumeOverlay.cs (PopIn)
    fn resume(&mut self) {
        // Breaks resume at once, and so does a hidden cursor (never moved).
        if self.is_break_time(self.last_time) || !self.input.cursor_moved() {
            self.complete_resume();
        } else {
            self.phase = Phase::Resuming;
            self.resumes = self.resumes.wrapping_add(1);
            // The gameplay cursor stood still while paused.
            self.resume_target = self.input.position();
        }
    }

    fn complete_resume(&mut self) {
        self.phase = Phase::Playing;
        self.run = true;
        self.input.resume();
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (onFailComplete, PerformExitWithConfirmation)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (Start)
    /// The fail animation ends, or Back cut it short (`cut`): its transforms jump to their
    /// end.
    fn complete_fail(&mut self, now_ns: u64, sink: &mut impl VoiceSink, cut: bool) {
        self.phase = Phase::Failed;
        self.run = false;
        self.track_effects = TrackEffects::Failed;
        if let Some(fail) = &mut self.fail {
            fail.cut = cut;
        }
        self.sounds.stop_all(sink);
        self.input.suspend();
        self.show_menu(MenuKind::Fail, now_ns);
        let save = self.replay_save();
        self.menu.set_replay_save(save);
    }

    fn exit(&mut self, sink: &mut impl VoiceSink) {
        if self.phase == Phase::Exited {
            return;
        }
        self.phase = Phase::Exited;
        self.run = false;
        self.sounds.stop_all(sink);
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (OnExiting)
        self.sounds.stop_fail(sink);
        self.track_effects = TrackEffects::Neutral;
        self.input.suspend();
        // The player screen goes with its menu.
        self.menu.reset();
    }

    /// Starts the play over from its start time. Allocates: the rules, health and mods are
    /// copied from their initial state.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (Restart, StartGameplay)
    fn restart(&mut self, sink: &mut impl VoiceSink, quick: bool) {
        self.rules = self.initial.rules.clone();
        self.spm.reset();
        self.expand.reset();
        self.health = self.initial.health.clone();
        self.score = self.initial.score.clone();
        self.mods = self.initial.mods.clone();
        self.log = JudgementLog::with_capacity(self.rules.max_judgements());
        self.skip = self.initial.skip;
        // A stop that did not queue is retried by the sounds' next update.
        self.sounds.reset(sink);
        // The old player's fail animation goes with it (`OnExiting`).
        self.sounds.stop_fail(sink);
        self.track_effects = TrackEffects::Neutral;
        self.fail = None;
        self.recorder.clear();
        if self.spare_attempt.is_none() {
            self.spare_attempt = Some(Box::new(FinishedAttempt::spare(
                self.recorder.spare(),
                self.initial.mods.clone(),
            )));
        }
        self.input.reset();
        self.phase = Phase::Playing;
        self.pending_seek = Some(self.start_time);
        self.run = true;
        self.auto_skip = quick;
        self.applied = 0;
        self.popups_seen = 0;
        self.popups.restart();
        self.shakes.clear();
        self.hit_errors.restart();
        self.hit_events.restart();
        self.failed_at = None;
        self.last_pause_time = None;
        self.last_time = self.start_time;
        self.restart_on_fail = false;
        self.passed = false;
        self.failed_replay_saved = false;
        self.restarts += 1;
        self.key_counts = [0; KEYS];
        self.key_presses_seen = [0; KEYS];
        // A new player shows no panel and no menu.
        self.offset_panel.reset();
        self.menu.reset();
    }

    fn is_break_time(&mut self, time: f64) -> bool {
        self.breaks
            .update(time, self.score.has_completed(time))
            .is_break_time
    }

    /// Sends the pending seek, then starts or stops the clock as wanted. A failed seek may
    /// leave it stopped; the start is sent with the seek's retry.
    fn sync_clock(&mut self, now_ns: u64, track: &mut impl TrackControl) -> Result<(), ClockError> {
        if !self.speed_set {
            // Before the first seek: it converts game time at the clock's rate.
            self.clock
                .set_rate(now_ns, self.speed.rate, self.speed.tempo, track)?;
            self.speed_set = true;
        }
        if let Some(target) = self.pending_seek {
            // Game time includes the offsets; the clock runs in track time.
            let track_time = crate::time::track_time_for(target, self.speed.rate, &self.offsets);
            self.clock.seek(now_ns, track_time, track)?;
            self.pending_seek = None;
        }
        if self.run {
            self.clock.start(now_ns, track)
        } else {
            self.clock.stop(now_ns, track)
        }
    }

    /// Gives the track the effects wanted, if it does not have them; a full queue retries on
    /// the next tick.
    fn sync_track(&mut self, track: &mut impl TrackControl) {
        if self.applied_effects == Some(self.track_effects) {
            return;
        }
        match track.adjust(&self.track_effects.adjustments(self.speed.frequency)) {
            Ok(()) => self.applied_effects = Some(self.track_effects),
            Err(_) => self.clock_errors += 1,
        }
    }

    fn waiting(&mut self, events: &[InputEvent], snapshot: &mut PlaySnapshot) {
        // The modifiers held count once the play starts; actions do nothing yet.
        for event in events {
            self.hotkeys.on_event(event);
        }
        snapshot.state = PlayState::Waiting;
        snapshot.time = self.start_time;
        snapshot.clock = None;
        snapshot.cursor = self.input.position();
        snapshot.cursor_path.clone_from(self.input.path());
        snapshot.cursor_expand = self.expand.times();
        snapshot.skip_available = false;
        snapshot.health = self.health.health();
        snapshot.hud = HudValues::of(&self.score);
        let (presses, held) = self.input.keys();
        snapshot.keys = KeyValues {
            presses,
            counts: self.key_counts,
            held,
        };
        snapshot.resume_target = Vec2::ZERO;
        snapshot.resume_cursor = self.input.window_cursor();
        snapshot
            .resume_cursor_path
            .clone_from(self.input.window_path());
        snapshot.resumes = self.resumes;
        snapshot.retry_progress = 0.0;
        snapshot.exit_progress = 0.0;
        snapshot.restarts = self.restarts;
        snapshot.menu = self.menu.snapshot();
        snapshot.fail = None;
        snapshot.offset_panel = self.offset_panel;
        snapshot.judgements.clone_from(&self.popups);
        snapshot.hit_errors.clone_from(&self.hit_errors);
        snapshot.circles.clear();
        snapshot.sliders.clear();
        snapshot.spinners.clear();
        snapshot.shakes = self.shakes;
    }

    /// Fills the fields every gameplay tick writes alike.
    fn fill(&self, snapshot: &mut PlaySnapshot, now_ns: u64) {
        snapshot.health = self.health.health();
        snapshot.hud = HudValues::of(&self.score);
        let (presses, held) = self.input.keys();
        snapshot.keys = KeyValues {
            presses,
            counts: self.key_counts,
            held,
        };
        snapshot.resume_target = self.resume_target;
        snapshot.resume_cursor = self.input.window_cursor();
        snapshot
            .resume_cursor_path
            .clone_from(self.input.window_path());
        snapshot.resumes = self.resumes;
        snapshot.retry_progress = self.hotkeys.retry_progress(now_ns);
        snapshot.exit_progress = self.hotkeys.exit_progress(now_ns);
        snapshot.restarts = self.restarts;
        snapshot.menu = self.menu.snapshot();
        snapshot.fail = self.fail;
        snapshot.judgements.clone_from(&self.popups);
        snapshot.hit_errors.clone_from(&self.hit_errors);
        snapshot.circles.capture(&self.rules);
        snapshot.sliders.capture(&self.rules);
        snapshot.spinners.capture(&self.rules, &self.spm);
        snapshot.shakes = self.shakes;
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Primitives/RectangleF.cs (Contains)
/// Whether the click-to-resume cursor at `target` is hovered by the cursor at `at`: its
/// square hit area, as the framework's rectangle (left and top edges inside).
pub(crate) fn hovers(target: Vec2, at: Vec2) -> bool {
    let half = RESUME_CURSOR_SIZE / 2.0;
    let (left, top) = (target.x - half, target.y - half);
    left <= at.x
        && at.x < left + RESUME_CURSOR_SIZE
        && top <= at.y
        && at.y < top + RESUME_CURSOR_SIZE
}

// ---- Draw side ----

/// Skin textures of a play, by element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Element {
    Circle,
    Overlay,
    Approach,
    /// `sliderstartcircle` and its overlay; requested only when the skin has it.
    HeadCircle,
    HeadOverlay,
    /// `sliderendcircle` and its overlay; requested only when the skin has it.
    TailCircle,
    TailOverlay,
    /// `sliderscorepoint`.
    ScorePoint,
    /// `reversearrow`.
    ReverseArrow,
    /// `sliderb-nd` and `sliderb-spec` of the skin that provides the ball.
    BallNd,
    BallSpec,
    /// `cursor`, `cursormiddle` of the skin with `cursor`, `cursortrail` of the skin with it.
    Cursor,
    CursorMiddle,
    CursorTrail,
    Digit(u8),
    /// A legacy spinner texture.
    Spinner(SpinnerPart),
}

/// The textures of legacy spinners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpinnerPart {
    Background,
    Circle,
    Metre,
    Glow,
    Bottom,
    Top,
    Middle2,
    Middle,
    Approach,
    Spin,
    Clear,
    Rpm,
}

impl SpinnerPart {
    fn name(self) -> &'static str {
        match self {
            SpinnerPart::Background => "spinner-background",
            SpinnerPart::Circle => "spinner-circle",
            SpinnerPart::Metre => "spinner-metre",
            SpinnerPart::Glow => "spinner-glow",
            SpinnerPart::Bottom => "spinner-bottom",
            SpinnerPart::Top => "spinner-top",
            SpinnerPart::Middle2 => "spinner-middle2",
            SpinnerPart::Middle => "spinner-middle",
            SpinnerPart::Approach => "spinner-approachcircle",
            SpinnerPart::Spin => "spinner-spin",
            SpinnerPart::Clear => "spinner-clear",
            SpinnerPart::Rpm => "spinner-rpm",
        }
    }

    fn slot(self, textures: &mut SpinnerTextures) -> &mut Option<SkinSprite> {
        match self {
            SpinnerPart::Background => &mut textures.background,
            SpinnerPart::Circle => &mut textures.circle,
            SpinnerPart::Metre => &mut textures.metre,
            SpinnerPart::Glow => &mut textures.glow,
            SpinnerPart::Bottom => &mut textures.bottom,
            SpinnerPart::Top => &mut textures.top,
            SpinnerPart::Middle2 => &mut textures.middle2,
            SpinnerPart::Middle => &mut textures.middle,
            SpinnerPart::Approach => &mut textures.approach,
            SpinnerPart::Spin => &mut textures.spin,
            SpinnerPart::Clear => &mut textures.clear,
            SpinnerPart::Rpm => &mut textures.rpm,
        }
    }
}

const ELEMENTS: [Element; 36] = [
    Element::Circle,
    Element::Overlay,
    Element::Approach,
    Element::HeadCircle,
    Element::HeadOverlay,
    Element::TailCircle,
    Element::TailOverlay,
    Element::ScorePoint,
    Element::ReverseArrow,
    Element::BallNd,
    Element::BallSpec,
    Element::Cursor,
    Element::CursorMiddle,
    Element::CursorTrail,
    Element::Digit(0),
    Element::Digit(1),
    Element::Digit(2),
    Element::Digit(3),
    Element::Digit(4),
    Element::Digit(5),
    Element::Digit(6),
    Element::Digit(7),
    Element::Digit(8),
    Element::Digit(9),
    Element::Spinner(SpinnerPart::Background),
    Element::Spinner(SpinnerPart::Circle),
    Element::Spinner(SpinnerPart::Metre),
    Element::Spinner(SpinnerPart::Glow),
    Element::Spinner(SpinnerPart::Bottom),
    Element::Spinner(SpinnerPart::Top),
    Element::Spinner(SpinnerPart::Middle2),
    Element::Spinner(SpinnerPart::Middle),
    Element::Spinner(SpinnerPart::Approach),
    Element::Spinner(SpinnerPart::Spin),
    Element::Spinner(SpinnerPart::Clear),
    Element::Spinner(SpinnerPart::Rpm),
];

/// A requested skin texture with the pixel density of its file.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Requested {
    handle: TextureHandle,
    scale: f32,
}

/// Why skin textures could not be requested.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("{path}: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("texture request rejected: {0}")]
    Rejected(#[from] slam_render::upload::Rejected),
    #[error("white texture request rejected: {0}")]
    PixelsRejected(#[from] slam_render::upload::Rejected<Image>),
}

/// The skin image check of a play (ADR-0110): an image decodes when it reads and decodes
/// within `limits`, the texture loader's ([`TextureLoaderConfig::image_limits`]). A file that
/// fails is logged here, once, and skin lookups then treat it as missing.
///
/// [`TextureLoaderConfig::image_limits`]: slam_render::upload::TextureLoaderConfig::image_limits
pub fn skin_image_check(limits: ImageLimits) -> ImageCheck {
    // Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Textures/TextureLoaderStore.cs
    ImageCheck::new(move |path| {
        let decoded = std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| decode_image(&bytes, limits).map_err(|e| e.to_string()));
        decoded
            .inspect_err(|e| {
                tracing::warn!(path = %path.display(), "skin image treated as missing: {e}");
            })
            .is_ok()
    })
}

/// The display size of the image `file` (pixels over its density), read off the disk;
/// `None` when it does not decode within the texture loader's `limits`.
fn image_size(file: &TextureFile, limits: ImageLimits) -> Option<[f32; 2]> {
    let bytes = std::fs::read(&file.path).ok()?;
    let image = decode_image(&bytes, limits).ok()?;
    Some([
        image.size.width as f32 / file.scale,
        image.size.height as f32 / file.scale,
    ])
}

/// Reads `file` and queues it for upload into the atlas.
fn request_file(
    requests: &mut TextureRequests,
    file: &TextureFile,
) -> Result<Requested, RequestError> {
    request_file_as(requests, file, TextureKind::Atlas)
}

/// Reads `file` and queues it for upload as `kind`.
fn request_file_as(
    requests: &mut TextureRequests,
    file: &TextureFile,
    kind: TextureKind,
) -> Result<Requested, RequestError> {
    let bytes = std::fs::read(&file.path).map_err(|source| RequestError::Read {
        path: file.path.clone(),
        source,
    })?;
    let handle = requests.load(bytes, kind)?;
    Ok(Requested {
        handle,
        scale: file.scale,
    })
}

/// Side of the white image the HUD's rectangles tint: the atlas border keeps its edge
/// texels white, a few pixels keep filtering inside.
const WHITE_SIDE: u32 = 4;

/// The skin textures of a play: requested once, looked up every frame.
#[derive(Debug, Clone)]
pub struct PlayTextures {
    requested: [Option<Requested>; ELEMENTS.len()],
    white: TextureHandle,
    /// The hit error meters' disc, gradient and triangle.
    disc: TextureHandle,
    gradient: TextureHandle,
    triangle: TextureHandle,
    /// Whether slider heads use `sliderstartcircle` rather than the hit circle's textures.
    head_prefix: bool,
    /// Whether slider tails use `sliderendcircle` rather than the hit circle's textures.
    tail_prefix: bool,
    /// The frames of the slider ball.
    ball: Vec<Requested>,
    /// The frames of the follow circle and their length.
    follow: Vec<Requested>,
    follow_frame_length: f64,
    /// The frames of the follow points and their length.
    follow_points: Vec<Requested>,
    follow_point_frame_length: f64,
    cursor: CursorSkinConfig,
    /// The judgement popups' frames and particles, by `judgements::slot`.
    judgements: [RequestedPiece; PIECE_SLOTS],
    lighting: Option<Requested>,
    /// The score and combo fonts and the health bar.
    hud: HudRequests,
    /// The song progress's ring.
    ring: TextureHandle,
    /// The stable pause and fail menus' images, in [`PAUSE_IMAGES`] order (ADR-0122).
    pause: [Option<Requested>; PAUSE_IMAGES.len()],
    /// The display sizes of stable's three pause buttons, read when requested: the update
    /// side lays them out without the uploads.
    pause_sizes: [Option<[f32; 2]>; 3],
}

/// The skin images of stable's pause and fail menus (ADR-0122): the backgrounds, then the
/// buttons in the order the pause menu shows them.
// Ported from McOsu db2add2: src/App/Osu/OsuSkin.cpp (load)
pub const PAUSE_IMAGES: [&str; 5] = [
    "pause-overlay",
    "fail-background",
    "pause-continue",
    "pause-retry",
    "pause-back",
];

/// The requested textures of one result's popup.
#[derive(Debug, Clone, Default)]
struct RequestedPiece {
    kind: PieceKind,
    frames: Vec<Requested>,
    frame_length: f64,
    particle: Option<Requested>,
}

/// Whether a circle piece looked up with `prefix` (`sliderstartcircle`, `sliderendcircle`) uses
/// it: when the skin that provides `hitcircle` has it, otherwise the piece falls back to
/// `hitcircle`, overlay included.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyMainCirclePiece.cs (load)
fn uses_prefix(skins: &SkinChain, prefix: &str) -> bool {
    match skins.find_provider(|s| s.texture("hitcircle").is_some()) {
        Some(provider) => skins.skin(provider).texture(prefix).is_some(),
        None => skins.texture(prefix).is_some(),
    }
}

impl PlayTextures {
    /// Reads the textures of hit circles, slider heads, tails, ticks, reverse arrows, balls and
    /// follow circles, follow points, legacy spinners and the cursor from `skins` and queues them for upload
    /// with a white image for the HUD. Missing ones are not drawn. Does file I/O: call it off the hot paths.
    pub fn request(
        skins: &SkinChain,
        requests: &mut TextureRequests,
        limits: ImageLimits,
    ) -> Result<PlayTextures, RequestError> {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (GetFontPrefix)
        let prefix = skins
            .config(|s| s.setting("HitCirclePrefix").map(str::to_owned))
            .map_or_else(|| "default".to_owned(), |f| f.value);
        let head_prefix = uses_prefix(skins, "sliderstartcircle");
        let tail_prefix = uses_prefix(skins, "sliderendcircle");
        let ball_skin = BallStyle::provider(skins).map(|p| skins.skin(p));
        let cursor_sources = CursorSources::new(skins);
        let mut requested = [None; ELEMENTS.len()];
        for (slot, element) in requested.iter_mut().zip(ELEMENTS) {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs (load)
            let ball_layer = match element {
                Element::BallNd => Some("sliderb-nd"),
                Element::BallSpec => Some("sliderb-spec"),
                _ => None,
            };
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursor.cs (load)
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyCursorTrail.cs (load)
            let cursor_piece = match element {
                Element::Cursor => Some((cursor_sources.cursor, "cursor")),
                Element::CursorMiddle => Some((cursor_sources.cursor, "cursormiddle")),
                Element::CursorTrail => Some((cursor_sources.trail, "cursortrail")),
                _ => None,
            };
            let layer = ball_layer
                .map(|name| (ball_skin, name))
                .or(cursor_piece.map(|(source, name)| (source.map(|s| skins.skin(s)), name)));
            if let Some((skin, name)) = layer {
                if let Some(file) = skin.and_then(|skin| skin.texture(name)) {
                    *slot = Some(request_file(requests, &file)?);
                }
                continue;
            }
            let name = match element {
                Element::Circle => "hitcircle".to_owned(),
                Element::Overlay => "hitcircleoverlay".to_owned(),
                Element::Approach => "approachcircle".to_owned(),
                Element::HeadCircle if head_prefix => "sliderstartcircle".to_owned(),
                Element::HeadOverlay if head_prefix => "sliderstartcircleoverlay".to_owned(),
                Element::TailCircle if tail_prefix => "sliderendcircle".to_owned(),
                Element::TailOverlay if tail_prefix => "sliderendcircleoverlay".to_owned(),
                Element::HeadCircle
                | Element::HeadOverlay
                | Element::TailCircle
                | Element::TailOverlay => continue,
                Element::ScorePoint => "sliderscorepoint".to_owned(),
                Element::ReverseArrow => "reversearrow".to_owned(),
                Element::BallNd
                | Element::BallSpec
                | Element::Cursor
                | Element::CursorMiddle
                | Element::CursorTrail => continue,
                Element::Digit(d) => format!("{prefix}-{d}"),
                Element::Spinner(part) => part.name().to_owned(),
            };
            let Some(found) = skins.texture(&name) else {
                continue;
            };
            *slot = Some(request_file(requests, &found.value)?);
        }
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs (load)
        let ball_options = AnimationOptions {
            animatable: true,
            looping: true,
            apply_config_frame_rate: false,
            separator: "",
        };
        let ball_frames = ball_skin
            .and_then(|skin| skin.animation("sliderb", ball_options))
            .map_or_else(Vec::new, |a| a.frames);
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (SliderFollowCircle)
        let follow_options = AnimationOptions {
            animatable: true,
            looping: true,
            apply_config_frame_rate: true,
            separator: "-",
        };
        let follow_animation = skins.animation("sliderfollowcircle", follow_options);
        let follow_frame_length = follow_animation
            .as_ref()
            .map_or(SIXTY_FRAME_TIME, |a| a.value.frame_length_ms);
        let follow_frames = follow_animation.map_or_else(Vec::new, |a| a.value.frames);
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (FollowPoint)
        let follow_point_animation = skins.animation("followpoint", follow_options);
        let follow_point_frame_length = follow_point_animation
            .as_ref()
            .map_or(SIXTY_FRAME_TIME, |a| a.value.frame_length_ms);
        let follow_point_frames = follow_point_animation.map_or_else(Vec::new, |a| a.value.frames);
        let request_frames = |requests: &mut TextureRequests, frames: &[TextureFile]| {
            frames
                .iter()
                .take(MAX_ANIMATION_FRAMES)
                .map(|f| request_file(requests, f))
                .collect::<Result<Vec<_>, _>>()
        };
        let ball = request_frames(requests, &ball_frames)?;
        let follow = request_frames(requests, &follow_frames)?;
        let follow_points = request_frames(requests, &follow_point_frames)?;
        let mut judgements: [RequestedPiece; PIECE_SLOTS] = Default::default();
        let mut frame_count = 0;
        for (slot, source) in judgements.iter_mut().zip(piece_sources(skins)) {
            // The pieces share the frame budget; one past it is not drawn.
            let frames = source.frames.len().min(MAX_ANIMATION_FRAMES);
            if frame_count + frames > MAX_JUDGEMENT_FRAMES {
                tracing::warn!(
                    frames,
                    "judgement animation over the frame budget, not drawn"
                );
                continue;
            }
            frame_count += frames;
            *slot = RequestedPiece {
                kind: source.kind,
                frames: request_frames(requests, &source.frames)?,
                frame_length: source.frame_length,
                particle: source
                    .particle
                    .map(|p| request_file(requests, &p))
                    .transpose()?,
            };
        }
        let lighting = skins
            .texture("lighting")
            .map(|f| request_file(requests, &f.value))
            .transpose()?;
        let side = WHITE_SIDE as usize;
        let white = requests.load_pixels(
            Image {
                size: Extent {
                    width: WHITE_SIDE,
                    height: WHITE_SIDE,
                },
                pixels: vec![u8::MAX; side * side * 4],
            },
            TextureKind::Atlas,
        )?;
        let disc = requests.load_pixels(disc_image(), TextureKind::Atlas)?;
        let gradient = requests.load_pixels(gradient_image(), TextureKind::Atlas)?;
        let triangle = requests.load_pixels(triangle_image(), TextureKind::Atlas)?;
        let ring = requests.load_pixels(ring_image(), TextureKind::Atlas)?;
        let hud = HudRequests::request(skins, requests)?;
        let mut pause = [None; PAUSE_IMAGES.len()];
        let mut pause_sizes = [None; 3];
        for (index, (slot, name)) in pause.iter_mut().zip(PAUSE_IMAGES).enumerate() {
            if let Some(found) = skins.texture(name) {
                if let Some(size) = index.checked_sub(2).and_then(|i| pause_sizes.get_mut(i)) {
                    *size = image_size(&found.value, limits);
                }
                // The backgrounds fill the screen: too large for the atlas.
                let kind = if name.ends_with("overlay") || name.ends_with("background") {
                    TextureKind::Standalone
                } else {
                    TextureKind::Atlas
                };
                *slot = Some(request_file_as(requests, &found.value, kind)?);
            }
        }
        Ok(PlayTextures {
            requested,
            white,
            disc,
            gradient,
            triangle,
            head_prefix,
            tail_prefix,
            ball,
            follow,
            follow_frame_length,
            follow_points,
            follow_point_frame_length,
            cursor: CursorSkinConfig::new(skins),
            judgements,
            lighting,
            hud,
            ring,
            pause,
            pause_sizes,
        })
    }

    /// The display sizes of stable's three pause buttons when the skins have them all:
    /// without them stable's pause menu shows SLAM's (ADR-0122).
    pub fn stable_pause_sizes(&self) -> Option<[[f32; 2]; 3]> {
        let [a, b, c] = self.pause_sizes;
        Some([a?, b?, c?])
    }

    /// The textures uploaded by this frame; pending and failed ones are `None`.
    pub fn resolve(&self, uploads: &TextureUploads) -> FrameTextures {
        let mut frame = FrameTextures::default();
        frame.cursor.config = self.cursor;
        if let TextureState::Ready(region) = uploads.get(self.white) {
            frame.white = Some(region);
        }
        if let TextureState::Ready(disc) = uploads.get(self.disc) {
            frame.disc = Some(disc);
        }
        if let (
            Some(white),
            TextureState::Ready(disc),
            TextureState::Ready(gradient),
            TextureState::Ready(triangle),
        ) = (
            frame.white,
            uploads.get(self.disc),
            uploads.get(self.gradient),
            uploads.get(self.triangle),
        ) {
            frame.meter = Some(MeterTextures {
                white,
                disc,
                gradient,
                triangle,
            });
        }
        for (requested, element) in self.requested.iter().zip(ELEMENTS) {
            let Some(r) = requested else { continue };
            let TextureState::Ready(region) = uploads.get(r.handle) else {
                continue;
            };
            match element {
                Element::Circle => frame.circles.circle = Some(SkinSprite::circle(region, r.scale)),
                Element::Overlay => {
                    frame.circles.overlay = Some(SkinSprite::circle(region, r.scale))
                }
                Element::Approach => {
                    let sprite = SkinSprite::circle(region, r.scale);
                    let clear = uploads.clear_radius(r.handle);
                    frame.circles.approach_hole = sprite.hole(clear, r.scale);
                    frame.circles.approach = Some(sprite);
                }
                Element::HeadCircle => {
                    frame.sliders.head.circle = Some(SkinSprite::circle(region, r.scale))
                }
                Element::HeadOverlay => {
                    frame.sliders.head.overlay = Some(SkinSprite::circle(region, r.scale))
                }
                Element::TailCircle => {
                    frame.sliders.tail.circle = Some(SkinSprite::circle(region, r.scale))
                }
                Element::TailOverlay => {
                    frame.sliders.tail.overlay = Some(SkinSprite::circle(region, r.scale))
                }
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/OsuLegacySkinTransformer.cs (SliderScorePoint)
                Element::ScorePoint => {
                    frame.sliders.tick = Some(SkinSprite::new(region, r.scale, OBJECT_SIZE))
                }
                // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacyReverseArrow.cs (load)
                Element::ReverseArrow => {
                    frame.sliders.arrow = Some(SkinSprite::new(region, r.scale, OBJECT_SIZE * 2.0))
                }
                Element::BallNd => {
                    frame.sliders.ball_nd = Some(SkinSprite::new(
                        region,
                        r.scale,
                        MAX_FOLLOW_CIRCLE_AREA_SIZE,
                    ))
                }
                Element::BallSpec => {
                    frame.sliders.ball_spec = Some(SkinSprite::new(
                        region,
                        r.scale,
                        MAX_FOLLOW_CIRCLE_AREA_SIZE,
                    ))
                }
                Element::Cursor => {
                    frame.cursor.cursor = Some(SkinSprite::new(region, r.scale, f32::MAX))
                }
                Element::CursorMiddle => {
                    frame.cursor.middle = Some(SkinSprite::new(region, r.scale, f32::MAX))
                }
                Element::CursorTrail => {
                    frame.cursor.trail = Some(SkinSprite::new(region, r.scale, f32::MAX))
                }
                Element::Digit(d) => {
                    frame.circles.digits[usize::from(d)] = Some(SkinSprite::digit(region, r.scale))
                }
                // Legacy spinner sprites are not cropped.
                Element::Spinner(part) => {
                    *part.slot(&mut frame.spinners) =
                        Some(SkinSprite::new(region, r.scale, f32::MAX))
                }
            }
        }
        // An animation is drawn once all its frames are uploaded.
        let animation = |frames: &[Requested], frame_length, max_size| {
            let sprites = frames.iter().map(|r| match uploads.get(r.handle) {
                TextureState::Ready(region) => Some(SkinSprite::within(region, r.scale, max_size)),
                _ => None,
            });
            if sprites.clone().all(|s| s.is_some()) {
                AnimationSprites::new(sprites.flatten(), frame_length)
            } else {
                AnimationSprites::default()
            }
        };
        // The ball's frame length depends on the slider: see `BallPose::frame_length`.
        let follow_area = Vec2::new(MAX_FOLLOW_CIRCLE_AREA_SIZE, MAX_FOLLOW_CIRCLE_AREA_SIZE);
        frame.sliders.ball = animation(&self.ball, SIXTY_FRAME_TIME, follow_area);
        frame.sliders.follow = animation(&self.follow, self.follow_frame_length, follow_area);
        frame.follow_points = animation(
            &self.follow_points,
            self.follow_point_frame_length,
            MAX_FOLLOW_POINT_SIZE,
        );
        if !self.head_prefix {
            frame.sliders.head = PieceTextures::from(&frame.circles);
        }
        if !self.tail_prefix {
            frame.sliders.tail = PieceTextures::from(&frame.circles);
        }
        self.resolve_judgements(uploads, &mut frame.judgements);
        frame.hud = self.hud.resolve(uploads);
        frame.hud.disc = frame.disc;
        if let TextureState::Ready(ring) = uploads.get(self.ring) {
            frame.hud.ring = Some(ring);
        }
        for (sprite, requested) in frame.pause.images.iter_mut().zip(&self.pause) {
            *sprite = requested.and_then(|r| match uploads.get(r.handle) {
                TextureState::Ready(region) => Some(SkinSprite::new(region, r.scale, f32::MAX)),
                _ => None,
            });
        }
        // The spinners' counters are in the score font too.
        frame.spinners.score_digits = frame.hud.score.digits;
        frame
    }

    /// The popup textures uploaded by this frame: a piece is drawn once all its textures are.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (GetAnimation)
    fn resolve_judgements(&self, uploads: &TextureUploads, textures: &mut JudgementTextures) {
        // Neither popups nor the lighting are cropped (no `maxSize`).
        let sprite = |r: &Requested| match uploads.get(r.handle) {
            TextureState::Ready(region) => Some(SkinSprite::new(region, r.scale, f32::MAX)),
            _ => None,
        };
        textures.lighting = self.lighting.as_ref().and_then(sprite);
        let mut next = 0;
        for (piece, out) in self.judgements.iter().zip(&mut textures.pieces) {
            let start = next;
            let mut ready = true;
            for r in &piece.frames {
                textures.frames[next] = sprite(r);
                ready &= textures.frames[next].is_some();
                next += 1;
            }
            let particle = piece.particle.as_ref().map(sprite);
            ready &= particle.is_none_or(|p| p.is_some());
            if ready && !piece.frames.is_empty() {
                *out = PieceSprites {
                    kind: piece.kind,
                    // The request kept the frames within `MAX_JUDGEMENT_FRAMES`.
                    frames: (start as u16, piece.frames.len() as u16),
                    frame_length: piece.frame_length,
                    particle: particle.flatten(),
                };
            }
        }
    }

    /// Requests whose file was found in the skins, animation frames included.
    pub fn found(&self) -> usize {
        let judgements: usize = self
            .judgements
            .iter()
            .map(|p| p.frames.len() + usize::from(p.particle.is_some()))
            .sum();
        self.requested.iter().flatten().count()
            + self.ball.len()
            + self.follow.len()
            + self.follow_points.len()
            + judgements
            + usize::from(self.lighting.is_some())
            + self.hud.found()
            + self.pause.iter().flatten().count()
    }
}

/// The requested textures of a legacy font and its overlap.
#[derive(Debug, Clone, Default)]
struct FontRequests {
    /// The digits, then the characters of [`legacy_text::SYMBOLS`].
    glyphs: [Option<Requested>; 14],
    overlap: f32,
}

impl FontRequests {
    /// The font with `prefix` (`{prefix}-0`, ...): each character is looked up through the
    /// whole chain (`LegacyGlyphStore`).
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySpriteText.cs (LegacyGlyphStore.Get)
    fn request(
        skins: &SkinChain,
        requests: &mut TextureRequests,
        prefix: &str,
        overlap: f32,
    ) -> Result<FontRequests, RequestError> {
        let mut glyphs = [None; 14];
        for (slot, name) in glyphs.iter_mut().zip(legacy_text::glyph_names(prefix)) {
            if let Some(found) = skins.texture(&name) {
                *slot = Some(request_file(requests, &found.value)?);
            }
        }
        Ok(FontRequests { glyphs, overlap })
    }

    fn resolve(&self, uploads: &TextureUploads) -> FontSprites {
        let mut loaded = true;
        let mut sprite = |r: &Option<Requested>| {
            let r = r.as_ref()?;
            glyph_sprite(uploads.get(r.handle), r.scale, &mut loaded)
        };
        let mut font = FontSprites {
            overlap: self.overlap,
            ..FontSprites::default()
        };
        for (out, r) in font.digits.iter_mut().zip(&self.glyphs[..10]) {
            *out = sprite(r);
        }
        for (out, r) in font.symbols.iter_mut().zip(&self.glyphs[10..]) {
            *out = sprite(r);
        }
        font.loaded = loaded;
        font
    }

    fn found(&self) -> usize {
        self.glyphs.iter().flatten().count()
    }
}

/// The sprite of a requested glyph in `state`: a failed one is missing, a pending one
/// clears `loaded` (the font is not drawn yet).
fn glyph_sprite(state: TextureState, scale: f32, loaded: &mut bool) -> Option<SkinSprite> {
    match state {
        TextureState::Ready(region) => Some(SkinSprite::new(region, scale, f32::MAX)),
        TextureState::Failed(_) => None,
        TextureState::Pending => {
            *loaded = false;
            None
        }
    }
}

/// The requested textures of the health bar, all from the first skin with `scorebar-bg`.
#[derive(Debug, Clone, Default)]
struct HealthRequests {
    background: Option<Requested>,
    fill: Vec<Requested>,
    frame_length: f64,
    /// `None` without a skin; the new style's marker, or the old style's three.
    marker: Option<HealthMarkerRequests>,
}

#[derive(Debug, Clone)]
enum HealthMarkerRequests {
    New(Option<Requested>),
    Old([Option<Requested>; 3]),
}

impl HealthRequests {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyHealthDisplay.cs (load, LegacyFill, LegacyOldStyleMarker, LegacyNewStyleMarker)
    fn request(
        skins: &SkinChain,
        requests: &mut TextureRequests,
    ) -> Result<HealthRequests, RequestError> {
        let Some(provider) = skins.find_provider(|s| s.texture("scorebar-bg").is_some()) else {
            return Ok(HealthRequests::default());
        };
        let skin = skins.skin(provider);
        let mut request = |name: &str| {
            skin.texture(name)
                .map(|f| request_file(requests, &f))
                .transpose()
        };
        let background = request("scorebar-bg")?;
        // The marker decides the style.
        let marker = if skin.texture("scorebar-marker").is_some() {
            HealthMarkerRequests::New(request("scorebar-marker")?)
        } else {
            HealthMarkerRequests::Old([
                request("scorebar-ki")?,
                request("scorebar-kidanger")?,
                request("scorebar-kidanger2")?,
            ])
        };
        let options = AnimationOptions {
            animatable: true,
            looping: true,
            apply_config_frame_rate: true,
            separator: "-",
        };
        let animation = skin.animation("scorebar-colour", options);
        let frame_length = animation
            .as_ref()
            .map_or(SIXTY_FRAME_TIME, |a| a.frame_length_ms);
        let fill = animation
            .map_or_else(Vec::new, |a| a.frames)
            .iter()
            .take(MAX_FILL_FRAMES)
            .map(|f| request_file(requests, f))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HealthRequests {
            background,
            fill,
            frame_length,
            marker: Some(marker),
        })
    }

    /// The textures once all of them are uploaded; none before.
    fn resolve(&self, uploads: &TextureUploads) -> HealthTextures {
        let mut ready = true;
        let mut sprite = |r: &Requested| match uploads.get(r.handle) {
            TextureState::Ready(region) => Some(SkinSprite::new(region, r.scale, f32::MAX)),
            _ => {
                ready = false;
                None
            }
        };
        let mut textures = HealthTextures {
            background: self.background.as_ref().and_then(&mut sprite),
            frame_length: self.frame_length,
            fill_frames: self.fill.len(),
            ..HealthTextures::default()
        };
        for (out, r) in textures.fill.iter_mut().zip(&self.fill) {
            *out = sprite(r);
        }
        textures.marker = match &self.marker {
            None => MarkerTextures::None,
            Some(HealthMarkerRequests::New(marker)) => {
                MarkerTextures::New(marker.as_ref().and_then(&mut sprite))
            }
            Some(HealthMarkerRequests::Old(ki)) => {
                MarkerTextures::Old(ki.each_ref().map(|r| r.as_ref().and_then(&mut sprite)))
            }
        };
        if ready {
            textures
        } else {
            HealthTextures::default()
        }
    }

    fn found(&self) -> usize {
        let marker = match &self.marker {
            None => 0,
            Some(HealthMarkerRequests::New(m)) => usize::from(m.is_some()),
            Some(HealthMarkerRequests::Old(ki)) => ki.iter().flatten().count(),
        };
        usize::from(self.background.is_some()) + self.fill.len() + marker
    }
}

/// The requested textures of the HUD.
#[derive(Debug, Clone, Default)]
struct HudRequests {
    /// `inputoverlay-background`, `inputoverlay-key`, and `InputOverlayText`.
    key_background: Option<Requested>,
    key: Option<Requested>,
    key_text: Option<Color>,
    /// The frames of `play-skip` and their length.
    skip: Vec<Requested>,
    skip_frame_length: f64,
    score: FontRequests,
    /// `None` when the combo font is the score font.
    combo: Option<FontRequests>,
    combo_overlap: f32,
    health: HealthRequests,
}

impl HudRequests {
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (GetFontPrefix, GetFontOverlap)
    fn request(
        skins: &SkinChain,
        requests: &mut TextureRequests,
    ) -> Result<HudRequests, RequestError> {
        let setting = |key: &str, default: &str| {
            skins
                .config(|s| s.setting(key).map(str::to_owned))
                .map_or_else(|| default.to_owned(), |f| f.value)
        };
        let overlap = |key: &str| {
            skins
                .config(|s| s.setting_f32(key))
                .map_or(0.0, |f| f.value)
        };
        let score_prefix = setting("ScorePrefix", "score");
        let combo_prefix = setting("ComboPrefix", "score");
        let score = FontRequests::request(skins, requests, &score_prefix, overlap("ScoreOverlap"))?;
        let combo_overlap = overlap("ComboOverlap");
        let combo = if combo_prefix == score_prefix {
            None
        } else {
            Some(FontRequests::request(
                skins,
                requests,
                &combo_prefix,
                combo_overlap,
            )?)
        };
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacyKeyCounterDisplay.cs (load)
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkin.cs (InputOverlayText)
        let texture = |requests: &mut TextureRequests, name: &str| {
            skins
                .texture(name)
                .map(|f| request_file(requests, &f.value))
                .transpose()
        };
        let key_background = texture(requests, "inputoverlay-background")?;
        let key = texture(requests, "inputoverlay-key")?;
        let key_text = skins
            .config(|s| s.custom_colour("InputOverlayText"))
            .map(|f| crate::spinners::rgba(f.value));
        // Ported from McOsu db2add2: src/App/Osu/OsuSkin.cpp (play-skip, animatable)
        let skip_options = AnimationOptions {
            animatable: true,
            looping: true,
            apply_config_frame_rate: true,
            separator: "-",
        };
        let skip_animation = skins.animation("play-skip", skip_options);
        let skip_frame_length = skip_animation
            .as_ref()
            .map_or(SIXTY_FRAME_TIME, |a| a.value.frame_length_ms);
        let skip = skip_animation
            .map_or_else(Vec::new, |a| a.value.frames)
            .iter()
            .take(MAX_SKIP_FRAMES)
            .map(|f| request_file(requests, f))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HudRequests {
            key_background,
            key,
            key_text,
            skip,
            skip_frame_length,
            score,
            combo,
            combo_overlap,
            health: HealthRequests::request(skins, requests)?,
        })
    }

    fn resolve(&self, uploads: &TextureUploads) -> HudTextures {
        let score = self.score.resolve(uploads);
        let combo = match &self.combo {
            Some(combo) => combo.resolve(uploads),
            None => FontSprites {
                overlap: self.combo_overlap,
                ..score
            },
        };
        let sprite = |r: &Option<Requested>| {
            let r = r.as_ref()?;
            match uploads.get(r.handle) {
                TextureState::Ready(region) => Some(SkinSprite::new(region, r.scale, f32::MAX)),
                _ => None,
            }
        };
        let mut skip = SkipTextures {
            frame_length: self.skip_frame_length,
            ..SkipTextures::default()
        };
        // An animation is drawn once all its frames are uploaded.
        let mut ready = !self.skip.is_empty();
        for (out, r) in skip.frames.iter_mut().zip(&self.skip) {
            *out = sprite(&Some(*r));
            ready &= out.is_some();
        }
        skip.count = if ready { self.skip.len() } else { 0 };
        HudTextures {
            score,
            combo,
            health: self.health.resolve(uploads),
            keys: KeyOverlayTextures {
                background: sprite(&self.key_background),
                key: sprite(&self.key),
            },
            key_text: self.key_text,
            skip,
            ..HudTextures::default()
        }
    }

    fn found(&self) -> usize {
        usize::from(self.key_background.is_some())
            + usize::from(self.key.is_some())
            + self.skip.len()
            + self.score.found()
            + self.combo.as_ref().map_or(0, FontRequests::found)
            + self.health.found()
    }
}

/// The textures of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameTextures {
    pub circles: CircleTextures,
    pub sliders: SliderTextures,
    pub spinners: SpinnerTextures,
    pub cursor: CursorTextures,
    pub judgements: JudgementTextures,
    /// The frames of `followpoint`.
    pub follow_points: AnimationSprites,
    /// An opaque white image.
    pub white: Option<AtlasRegion>,
    /// The hit error meter's images.
    pub meter: Option<MeterTextures>,
    /// A white disc on transparent.
    pub disc: Option<AtlasRegion>,
    /// The HUD's skin textures.
    pub hud: HudTextures,
    /// Stable's pause and fail menus' images (ADR-0122).
    pub pause: PauseTextures,
}

/// The images of stable's pause and fail menus, in [`PAUSE_IMAGES`] order; missing or not
/// uploaded ones are `None`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PauseTextures {
    pub images: [Option<SkinSprite>; PAUSE_IMAGES.len()],
}

/// The draw side of a play.
#[derive(Debug, Clone)]
pub struct PlayView {
    pub circles: HitCircles,
    pub sliders: HitSliders,
    pub spinners: HitSpinners,
    pub follow_points: FollowPoints,
    pub judgements: JudgementView,
    pub cursor: CursorView,
    pub resume: ResumeView,
}

impl PlayView {
    /// Writes SLAM's cursor (ADR-0124) for the cursors that were set to draw as such: where
    /// the skin's textures would be, `cursor_gameplay_size` osu!pixels high at the
    /// cursor's size. Allocation-free once `shapes` has room for
    /// [`MAX_CURSOR_SHAPES`](slam_ui::cursor::MAX_CURSOR_SHAPES) twice.
    pub fn push_slam_cursors(
        &self,
        shapes: &mut slam_render::shape::ShapeBatch,
        theme: &slam_ui::theme::Theme,
        white: Option<AtlasRegion>,
    ) {
        let Some(white) = white else { return };
        for pose in [self.cursor.slam_pose(), self.resume.cursor().slam_pose()]
            .into_iter()
            .flatten()
        {
            slam_ui::cursor::push_slam_cursor(
                shapes,
                theme,
                white,
                pose.at,
                theme.cursor_gameplay_size * pose.scale,
                0.0,
            );
        }
    }

    /// Writes the playfield's sprites of `snapshot` drawn at engine time `now_ns` on a
    /// surface laid out by `layout`, and the slider bodies to draw between them into `bodies`
    /// (cleared first); the cursor goes on top of the HUD with [`PlayView::push_cursor`].
    /// Allocation-free once `batch` has room.
    pub fn push(
        &mut self,
        batch: &mut SpriteBatch,
        bodies: &mut SliderBodies,
        layout: &PlayfieldLayout,
        snapshot: &PlaySnapshot,
        now_ns: u64,
        textures: &FrameTextures,
    ) {
        bodies.clear();
        if snapshot.state != PlayState::Waiting {
            let time = snapshot.time_at(now_ns);
            let [width, height] = layout.window_size();
            let fail = FailFrame::new(snapshot.fail, time, now_ns, width, height);
            let fail = fail.as_ref();
            self.spinners.push(
                batch,
                layout,
                snapshot.spinners.as_slice(),
                time,
                &textures.spinners,
                fail,
            );
            self.follow_points
                .push(batch, layout, time, &textures.follow_points);
            let judgements = &mut self.judgements;
            judgements.update(&snapshot.judgements, snapshot.restarts, time);
            judgements.push_below(batch, layout, time, &textures.judgements);
            push_objects(
                &self.circles,
                &self.sliders,
                batch,
                bodies,
                layout,
                snapshot.circles.as_slice(),
                &snapshot.sliders,
                time,
                &snapshot.shakes,
                ObjectTextures {
                    circles: &textures.circles,
                    sliders: &textures.sliders,
                },
                fail,
            );
            judgements.push_above(batch, layout, time, &textures.judgements);
            push_approaches(
                &self.circles,
                &self.sliders,
                batch,
                layout,
                snapshot.circles.as_slice(),
                snapshot.sliders.as_slice(),
                time,
                &snapshot.shakes,
                &textures.circles,
                fail,
            );
        }
    }

    /// Writes the cursor of `snapshot` drawn at engine time `now_ns` and its trail; call
    /// once a frame, the trail moves on. While resuming, gameplay's cursor and trail hide
    /// (lazer's `ActiveCursor.Hide`, the trail too here) and the resume overlay's target
    /// and cursor show instead. Allocation-free once `batch` has room for
    /// [`MAX_CURSOR_SPRITES`](crate::cursor::MAX_CURSOR_SPRITES) and
    /// [`MAX_TARGET_SPRITES`](crate::resume::MAX_TARGET_SPRITES).
    pub fn push_cursor(
        &mut self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        snapshot: &PlaySnapshot,
        now_ns: u64,
        textures: &FrameTextures,
    ) {
        let frame = CursorFrame {
            path: &snapshot.cursor_path,
            position: snapshot.cursor,
            expand: snapshot.cursor_expand,
            time: snapshot.time_at(now_ns),
            restarts: snapshot.restarts,
            now_ns,
            visible: snapshot.state != PlayState::Resuming,
        };
        self.cursor.push(batch, layout, &frame, &textures.cursor);
        self.resume.push(
            batch,
            layout,
            snapshot,
            now_ns,
            textures.disc,
            &textures.cursor,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slam_render::upload::LoadError;

    #[test]
    fn a_font_is_loaded_once_no_glyph_is_pending() {
        let region = AtlasRegion {
            texture: slam_render::textures::TextureId(1),
            origin: slam_render::Origin { x: 0, y: 0 },
            size: Extent::new(20, 40),
            uv: slam_render::atlas::UvRect::FULL,
        };
        let mut loaded = true;
        // Ready: a sprite at the file's density.
        let sprite = glyph_sprite(TextureState::Ready(region), 2.0, &mut loaded).unwrap();
        assert_eq!(sprite.size, Vec2::new(10.0, 20.0));
        // Failed: missing, but loaded.
        assert_eq!(
            glyph_sprite(TextureState::Failed(LoadError::Decode), 1.0, &mut loaded),
            None
        );
        assert!(loaded);
        // Pending: not loaded yet.
        assert_eq!(glyph_sprite(TextureState::Pending, 1.0, &mut loaded), None);
        assert!(!loaded);
        // A font none of whose glyphs the skins have is loaded and empty.
        let empty = FontRequests::default();
        let mut loaded = true;
        for r in &empty.glyphs {
            assert!(r.is_none());
            let _ = r.map(|r| glyph_sprite(TextureState::Pending, r.scale, &mut loaded));
        }
        assert!(loaded);
    }
}
