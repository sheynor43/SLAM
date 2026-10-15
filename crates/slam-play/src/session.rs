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
use slam_osu::judgement::{JudgementLog, Statistics};
use slam_osu::lead_in::{IntroSkip, earliest_start_time, gameplay_start_time};
use slam_osu::replay::{Actions, Rules};
use slam_osu::rules::OsuRules;
use slam_osu::scoring::{ScoreProcessor, ScoreRank};
use slam_osu::{Beatmap, HealthProcessor, ModSet, Vec2};
use slam_render::Extent;
use slam_render::atlas::AtlasRegion;
use slam_render::image::Image;
use slam_render::sprite::{Sprite, SpriteBatch};
use slam_render::upload::{
    TextureHandle, TextureKind, TextureRequests, TextureState, TextureUploads,
};
use slam_skin::{AnimationOptions, SIXTY_FRAME_TIME, SkinChain, TextureFile};

use crate::circles::{
    CircleTextures, HitCircles, LiveCircles, OBJECT_SIZE, PieceTextures, SkinSprite,
};
use crate::gameplay_sounds::{GameplaySounds, VoiceSink};
use crate::hotkeys::{GlobalAction, Hotkeys};
use crate::input::{ClockTime, GameTime, LiveInput};
use crate::playfield::PlayfieldLayout;
use crate::sliders::{
    AnimationSprites, BallStyle, HitSliders, LiveSliders, MAX_ANIMATION_FRAMES,
    MAX_FOLLOW_CIRCLE_AREA_SIZE, ObjectTextures, SliderBodies, SliderTextures, push_objects,
};
use crate::time::Offsets;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (PauseCooldownDuration)
/// Gameplay time after a pause before the next one is allowed, in milliseconds.
pub const PAUSE_COOLDOWN: f64 = 1000.0;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/FailAnimationContainer.cs (duration)
/// How long gameplay runs on after failing, in nanoseconds of real time. Lazer slows the
/// track down to a stop meanwhile; here it runs at its speed, then stops.
pub const FAIL_ANIMATION_NS: u64 = 2_500_000_000;

// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (RESULTS_DISPLAY_DELAY)
/// How long gameplay runs on after passing before the results show, in nanoseconds of real
/// time (lazer schedules them on the player's clock, not the gameplay clock).
pub const RESULTS_DISPLAY_DELAY_NS: u64 = 1_000_000_000;

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

/// The numbers of the HUD: lazer's score processor as the HUD counters bind it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudValues {
    /// Total score with the mods' multiplier (`TotalScore`).
    pub score: i64,
    /// Accuracy, 0..=1 (`Accuracy`).
    pub accuracy: f64,
    /// Current combo (`Combo`).
    pub combo: u32,
}

impl Default for HudValues {
    fn default() -> Self {
        HudValues {
            score: 0,
            accuracy: 1.0,
            combo: 0,
        }
    }
}

impl HudValues {
    fn of(score: &ScoreProcessor) -> HudValues {
        HudValues {
            score: score.total_score(),
            accuracy: score.accuracy(),
            combo: score.statistics().combo,
        }
    }
}

/// The final score of a passed play, for the results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayResults {
    /// Total score with the mods' multiplier.
    pub score: i64,
    /// Accuracy, 0..=1.
    pub accuracy: f64,
    pub rank: ScoreRank,
    /// Counts per result and the highest combo.
    pub statistics: Statistics,
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
    /// Whether the intro can be skipped now.
    pub skip_available: bool,
    /// Whether gameplay is in a break (lazer's `IsBreakTime`): the background dims less.
    pub is_break_time: bool,
    /// Health, 0..=1.
    pub health: f64,
    pub hud: HudValues,
    /// Where to click to resume, in osu!pixels ([`PlayState::Resuming`]).
    pub resume_target: Vec2,
    /// How far quick retry is held, 0..=1.
    pub retry_progress: f32,
    /// How far quick exit is held, 0..=1.
    pub exit_progress: f32,
    /// Times the play was restarted.
    pub restarts: u32,
    pub circles: LiveCircles,
    pub sliders: LiveSliders,
}

impl Default for PlaySnapshot {
    fn default() -> Self {
        PlaySnapshot {
            state: PlayState::Waiting,
            time: 0.0,
            clock: None,
            cursor: Vec2::ZERO,
            skip_available: false,
            // Lazer's `BreakTracker` starts in a break.
            is_break_time: true,
            health: 1.0,
            hud: HudValues::default(),
            resume_target: Vec2::ZERO,
            retry_progress: 0.0,
            exit_progress: 0.0,
            restarts: 0,
            circles: LiveCircles::new(),
            sliders: LiveSliders::new(),
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
    log: JudgementLog,
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
    /// Time of the last gameplay step: lazer's gameplay clock time, which stands still
    /// while paused and seeking.
    last_time: f64,
    /// A mod restarts the play on failing: at the end of the tick.
    restart_on_fail: bool,
    /// Every judgement is in without failing (lazer's `HasPassed`); stays after quitting.
    passed: bool,
    restarts: u32,
    initial: Initial,
    /// Clock commands that failed (queue full); retried on the next tick.
    clock_errors: u64,
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
        PlaySession {
            clock: GameClock::new(track),
            offsets,
            start_time,
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
            log,
            health,
            score,
            mods: mods.clone(),
            applied: 0,
            failed_at: None,
            sounds,
            last_pause_time: None,
            resume_target: Vec2::ZERO,
            last_time: start_time,
            restart_on_fail: false,
            passed: false,
            restarts: 0,
            clock_errors: 0,
        }
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

    pub fn input(&self) -> &LiveInput {
        &self.input
    }

    /// Times the play was restarted.
    pub fn restarts(&self) -> u32 {
        self.restarts
    }

    /// The final score once the play passed: while the results wait to show, show, and
    /// after quitting them.
    pub fn results(&self) -> Option<PlayResults> {
        self.passed.then(|| PlayResults {
            score: self.score.total_score(),
            accuracy: self.score.accuracy(),
            rank: self.score.rank(&self.initial.mods),
            statistics: *self.score.statistics(),
        })
    }

    /// Whether the player quit.
    pub fn has_exited(&self) -> bool {
        self.phase == Phase::Exited
    }

    /// Clock commands that failed and were retried.
    pub fn clock_errors(&self) -> u64 {
        self.clock_errors
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
                return self.waiting(events, snapshot);
            }
            None => return self.waiting(events, snapshot),
        };

        let time = ClockTime {
            clock,
            offsets: self.offsets,
        };
        let current = time.game_time(now_ns);
        // A seek in the track is heard one output latency later; until then time is the old
        // position, which must not reach the rules (a restart would miss every object).
        let seeking = self.clock.is_seeking();

        // Global actions take effect between the events around them: the events before go
        // to gameplay (or are followed while it is suspended), those after see the new state.
        let mut skip_requested = false;
        let mut from = 0;
        for (index, event) in events.iter().enumerate() {
            let action = self.hotkeys.on_event(event);
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
            match action {
                Some(GlobalAction::Back | GlobalAction::PauseGameplay) => {
                    if self.in_gameplay() {
                        self.step(&events[from..index], now_ns, &time, seeking, sink);
                        from = index + 1;
                    }
                    self.back(sink);
                }
                Some(GlobalAction::SkipCutscene) => skip_requested = true,
                _ => {}
            }
        }
        if self.in_gameplay() {
            self.step(&events[from..], now_ns, &time, seeking, sink);
        }

        if let Phase::Failing { until_ns } = self.phase
            && now_ns >= until_ns
        {
            self.complete_fail(sink);
        }
        if let Phase::Passed { results_at_ns } = self.phase
            && now_ns >= results_at_ns
        {
            self.show_results(sink);
        }

        let mut restart = std::mem::take(&mut self.restart_on_fail);
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

        if restart && self.phase != Phase::Exited {
            // Lazer's quick restarts skip the intro (`AutomaticallySkipIntro`).
            self.restart(sink, true);
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
        snapshot.clock =
            (!restart && !seek_sent && !self.clock.is_seeking() && self.phase != Phase::Results)
                .then_some(time);
        snapshot.cursor = self.input.position();
        snapshot.skip_available = self.phase == Phase::Playing
            && !seek_sent
            && !self.clock.is_seeking()
            && self.skip.is_available(current);
        snapshot.is_break_time = self.is_break_time(self.last_time);
        self.fill(snapshot, now_ns);
    }

    /// Stops the looping sounds (the play is closing). Returns whether all stops were
    /// queued; call again until it does.
    pub fn stop_sounds(&mut self, sink: &mut impl VoiceSink) -> bool {
        self.sounds.stop_all(sink)
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
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (LoadComponents)
        // The health processor is a frame-stable component ahead of the playfield: it drains
        // first, then takes the step's judgements as they come.
        self.health.update(step_time);
        self.rules.update(&step, &mut self.log);
        self.sounds.update(step_time, &self.log, &self.rules, sink);
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (LoadComponents)
        // Each judgement goes to the health processor, then to the score processor.
        for (index, judgement) in self.log.events().iter().enumerate().skip(self.applied) {
            let failed_before = self.health.has_failed();
            self.health.apply(judgement, &mut self.mods);
            self.score.apply(judgement, failed_before);
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
                self.phase = Phase::Failing {
                    until_ns: now_ns.saturating_add(FAIL_ANIMATION_NS),
                };
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
        }
    }

    /// Back or pause-gameplay pressed: lazer's `PerformExitWithConfirmation` from the hold-for-
    /// menu button, or the pause overlay's first button (continue) while paused.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (PerformExitWithConfirmation)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/PauseOverlay.cs (BackAction, OnPressed)
    fn back(&mut self, sink: &mut impl VoiceSink) {
        match self.phase {
            // A restart on failing is on its way: nothing to pause.
            Phase::Playing if self.restart_on_fail => {}
            Phase::Playing | Phase::Resuming => self.pause(sink),
            Phase::Paused => self.resume(),
            // The fail animation is cut short; the fail overlay shows.
            Phase::Failing { .. } => self.complete_fail(sink),
            Phase::Failed => self.exit(sink),
            // Passed, pausing is over (`ValidForResume` is false): the exit goes on.
            Phase::Passed { .. } | Phase::Results => self.perform_exit(sink),
            Phase::Exited => {}
        }
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
    fn pause(&mut self, sink: &mut impl VoiceSink) {
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
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (Resume)
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/UI/OsuResumeOverlay.cs (PopIn)
    fn resume(&mut self) {
        // Breaks resume at once, and so does a hidden cursor (never moved).
        if self.is_break_time(self.last_time) || !self.input.cursor_moved() {
            self.complete_resume();
        } else {
            self.phase = Phase::Resuming;
            // The gameplay cursor stood still while paused.
            self.resume_target = self.input.position();
        }
    }

    fn complete_resume(&mut self) {
        self.phase = Phase::Playing;
        self.run = true;
        self.input.resume();
    }

    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (onFailComplete)
    fn complete_fail(&mut self, sink: &mut impl VoiceSink) {
        self.phase = Phase::Failed;
        self.run = false;
        self.sounds.stop_all(sink);
        self.input.suspend();
    }

    fn exit(&mut self, sink: &mut impl VoiceSink) {
        if self.phase == Phase::Exited {
            return;
        }
        self.phase = Phase::Exited;
        self.run = false;
        self.sounds.stop_all(sink);
        self.input.suspend();
    }

    /// Starts the play over from its start time. Allocates: the rules, health and mods are
    /// copied from their initial state.
    // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Screens/Play/Player.cs (Restart, StartGameplay)
    fn restart(&mut self, sink: &mut impl VoiceSink, quick: bool) {
        self.rules = self.initial.rules.clone();
        self.health = self.initial.health.clone();
        self.score = self.initial.score.clone();
        self.mods = self.initial.mods.clone();
        self.log = JudgementLog::with_capacity(self.rules.max_judgements());
        self.skip = self.initial.skip;
        // A stop that did not queue is retried by the sounds' next update.
        self.sounds.reset(sink);
        self.input.reset();
        self.phase = Phase::Playing;
        self.pending_seek = Some(self.start_time);
        self.run = true;
        self.auto_skip = quick;
        self.applied = 0;
        self.failed_at = None;
        self.last_pause_time = None;
        self.last_time = self.start_time;
        self.restart_on_fail = false;
        self.passed = false;
        self.restarts += 1;
    }

    fn is_break_time(&mut self, time: f64) -> bool {
        self.breaks
            .update(time, self.score.has_completed(time))
            .is_break_time
    }

    /// Sends the pending seek, then starts or stops the clock as wanted. A failed seek may
    /// leave it stopped; the start is sent with the seek's retry.
    fn sync_clock(&mut self, now_ns: u64, track: &mut impl TrackControl) -> Result<(), ClockError> {
        if let Some(target) = self.pending_seek {
            // Game time includes the offsets; the clock runs in track time. Rate 1: no rate
            // mods exist yet (take the clock's rate once they do).
            let track_time = crate::time::track_time_for(target, 1.0, &self.offsets);
            self.clock.seek(now_ns, track_time, track)?;
            self.pending_seek = None;
        }
        if self.run {
            self.clock.start(now_ns, track)
        } else {
            self.clock.stop(now_ns, track)
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
        snapshot.skip_available = false;
        snapshot.health = self.health.health();
        snapshot.hud = HudValues::of(&self.score);
        snapshot.resume_target = Vec2::ZERO;
        snapshot.retry_progress = 0.0;
        snapshot.exit_progress = 0.0;
        snapshot.restarts = self.restarts;
        snapshot.circles.clear();
        snapshot.sliders.clear();
    }

    /// Fills the fields every gameplay tick writes alike.
    fn fill(&self, snapshot: &mut PlaySnapshot, now_ns: u64) {
        snapshot.health = self.health.health();
        snapshot.hud = HudValues::of(&self.score);
        snapshot.resume_target = self.resume_target;
        snapshot.retry_progress = self.hotkeys.retry_progress(now_ns);
        snapshot.exit_progress = self.hotkeys.exit_progress(now_ns);
        snapshot.restarts = self.restarts;
        snapshot.circles.capture(&self.rules);
        snapshot.sliders.capture(&self.rules);
    }
}

// Ported from osu-framework 2026.921.1: osu.Framework/Graphics/Primitives/RectangleF.cs (Contains)
/// Whether the click-to-resume cursor at `target` is hovered by the cursor at `at`: its
/// square hit area, as the framework's rectangle (left and top edges inside).
fn hovers(target: Vec2, at: Vec2) -> bool {
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
    Cursor,
    Digit(u8),
}

const ELEMENTS: [Element; 22] = [
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

/// Reads `file` and queues it for upload.
fn request_file(
    requests: &mut TextureRequests,
    file: &TextureFile,
) -> Result<Requested, RequestError> {
    let bytes = std::fs::read(&file.path).map_err(|source| RequestError::Read {
        path: file.path.clone(),
        source,
    })?;
    let handle = requests.load(bytes, TextureKind::Atlas)?;
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
    /// Whether slider heads use `sliderstartcircle` rather than the hit circle's textures.
    head_prefix: bool,
    /// Whether slider tails use `sliderendcircle` rather than the hit circle's textures.
    tail_prefix: bool,
    /// The frames of the slider ball.
    ball: Vec<Requested>,
    /// The frames of the follow circle and their length.
    follow: Vec<Requested>,
    follow_frame_length: f64,
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
    /// Reads the textures of hit circles, slider heads, tails, ticks, reverse arrows, balls and follow circles and the cursor from `skins` and queues them for
    /// upload with a white image for the HUD. Missing ones are not drawn. Does file I/O: call it off the hot paths.
    pub fn request(
        skins: &SkinChain,
        requests: &mut TextureRequests,
    ) -> Result<PlayTextures, RequestError> {
        // Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Skinning/LegacySkinExtensions.cs (GetFontPrefix)
        let prefix = skins
            .config(|s| s.setting("HitCirclePrefix").map(str::to_owned))
            .map_or_else(|| "default".to_owned(), |f| f.value);
        let head_prefix = uses_prefix(skins, "sliderstartcircle");
        let tail_prefix = uses_prefix(skins, "sliderendcircle");
        let ball_skin = BallStyle::provider(skins).map(|p| skins.skin(p));
        let mut requested = [None; ELEMENTS.len()];
        for (slot, element) in requested.iter_mut().zip(ELEMENTS) {
            // Ported from osu!lazer 2026.1005.0-lazer: osu.Game.Rulesets.Osu/Skinning/Legacy/LegacySliderBall.cs (load)
            let ball_layer = match element {
                Element::BallNd => Some("sliderb-nd"),
                Element::BallSpec => Some("sliderb-spec"),
                _ => None,
            };
            if let Some(name) = ball_layer {
                if let Some(file) = ball_skin.and_then(|skin| skin.texture(name)) {
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
                Element::BallNd | Element::BallSpec => continue,
                Element::Cursor => "cursor".to_owned(),
                Element::Digit(d) => format!("{prefix}-{d}"),
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
        let request_frames = |requests: &mut TextureRequests, frames: &[TextureFile]| {
            frames
                .iter()
                .take(MAX_ANIMATION_FRAMES)
                .map(|f| request_file(requests, f))
                .collect::<Result<Vec<_>, _>>()
        };
        let ball = request_frames(requests, &ball_frames)?;
        let follow = request_frames(requests, &follow_frames)?;
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
        Ok(PlayTextures {
            requested,
            white,
            head_prefix,
            tail_prefix,
            ball,
            follow,
            follow_frame_length,
        })
    }

    /// The textures uploaded by this frame; pending and failed ones are `None`.
    pub fn resolve(&self, uploads: &TextureUploads) -> FrameTextures {
        let mut frame = FrameTextures::default();
        if let TextureState::Ready(region) = uploads.get(self.white) {
            frame.white = Some(region);
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
                    frame.circles.approach = Some(SkinSprite::circle(region, r.scale))
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
                Element::Cursor => frame.cursor = Some(SkinSprite::new(region, r.scale, f32::MAX)),
                Element::Digit(d) => {
                    frame.circles.digits[usize::from(d)] = Some(SkinSprite::digit(region, r.scale))
                }
            }
        }
        // An animation is drawn once all its frames are uploaded.
        let animation = |frames: &[Requested], frame_length| {
            let sprites = frames.iter().map(|r| match uploads.get(r.handle) {
                TextureState::Ready(region) => Some(SkinSprite::new(
                    region,
                    r.scale,
                    MAX_FOLLOW_CIRCLE_AREA_SIZE,
                )),
                _ => None,
            });
            if sprites.clone().all(|s| s.is_some()) {
                AnimationSprites::new(sprites.flatten(), frame_length)
            } else {
                AnimationSprites::default()
            }
        };
        // The ball's frame length depends on the slider: see `BallPose::frame_length`.
        frame.sliders.ball = animation(&self.ball, SIXTY_FRAME_TIME);
        frame.sliders.follow = animation(&self.follow, self.follow_frame_length);
        if !self.head_prefix {
            frame.sliders.head = PieceTextures::from(&frame.circles);
        }
        if !self.tail_prefix {
            frame.sliders.tail = PieceTextures::from(&frame.circles);
        }
        frame
    }

    /// Requests whose file was found in the skins, animation frames included.
    pub fn found(&self) -> usize {
        self.requested.iter().flatten().count() + self.ball.len() + self.follow.len()
    }
}

/// The textures of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameTextures {
    pub circles: CircleTextures,
    pub sliders: SliderTextures,
    pub cursor: Option<SkinSprite>,
    /// An opaque white image.
    pub white: Option<AtlasRegion>,
}

/// The draw side of a play.
#[derive(Debug, Clone)]
pub struct PlayView {
    pub circles: HitCircles,
    pub sliders: HitSliders,
}

impl PlayView {
    /// Writes the playfield's sprites of `snapshot` drawn at engine time `now_ns` on a
    /// surface laid out by `layout`, and the slider bodies to draw between them into `bodies`
    /// (cleared first); the cursor goes on top of the HUD with [`PlayView::push_cursor`].
    /// Allocation-free once `batch` has room.
    pub fn push(
        &self,
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
            push_objects(
                &self.circles,
                &self.sliders,
                batch,
                bodies,
                layout,
                snapshot.circles.as_slice(),
                &snapshot.sliders,
                time,
                ObjectTextures {
                    circles: &textures.circles,
                    sliders: &textures.sliders,
                },
            );
        }
    }

    /// Writes the cursor of `snapshot`. Allocation-free once `batch` has room.
    pub fn push_cursor(
        &self,
        batch: &mut SpriteBatch,
        layout: &PlayfieldLayout,
        snapshot: &PlaySnapshot,
        textures: &FrameTextures,
    ) {
        // A stand-in for the skinned cursor and its trail.
        if let Some(t) = &textures.cursor {
            let centre = layout.to_window(snapshot.cursor);
            let size = [t.size.x * layout.scale(), t.size.y * layout.scale()];
            let mut sprite = Sprite::new(t.texture, t.uv, [centre.x, centre.y], size);
            sprite.origin = [size[0] / 2.0, size[1] / 2.0];
            batch.push(&sprite);
        }
    }
}
