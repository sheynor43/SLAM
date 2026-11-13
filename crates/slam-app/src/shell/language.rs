//! Switching the interface language without a restart (ADR-0091): the update thread switches
//! the shared [`Localizer`] and asks the `slam-language` thread for the screens' views laid
//! out again; the draw thread swaps them in and hands the old ones back to be freed there.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use slam_play::FpsView;
use slam_text::FontRegistry;
use slam_ui::curtain::Curtain;
use slam_ui::i18n::{FALLBACK_LANGUAGE, Localizer};
use slam_ui::menu::{MenuLabels, MenuView};
use slam_ui::results::{ResultsLabels, ResultsView, StatKind};
use slam_ui::settings_screen::SettingsView;
use slam_ui::song_select::{DetailLabels, SongSelectLabels, SongSelectView};
use slam_ui::theme::Theme;

use super::rows::Rows;
use super::{mod_columns, mod_settings, results_label_key};
use crate::settings::Settings;

/// How long the thread sleeps between polls: a switch shows within this.
const POLL: Duration = Duration::from_millis(10);
/// Requests and view sets in flight; a newer request replaces the older ones.
const RING: usize = 2;

/// The draw thread's views of every screen whose texts are laid out once.
pub struct InterfaceViews {
    pub menu: MenuView,
    pub song_select: SongSelectView,
    pub results: ResultsView,
    pub settings: SettingsView,
    pub fps: FpsView,
    /// The theme they were laid out with: the draw side's from the swap on.
    pub theme: Theme,
    /// The switch ([`AppliedLanguage::switches`]) they were laid out for.
    pub switch: u64,
}

/// Lays out every screen's texts in the localizer's language: the settings screen shows
/// `rows` and `settings`. Allocates and takes milliseconds: call it off the hot paths.
pub fn interface_views(
    registry: &Arc<FontRegistry>,
    theme: Theme,
    localizer: &Localizer,
    rows: &Rows,
    settings: &Settings,
) -> InterfaceViews {
    let text = |key: &str| localizer.format(key, None).into_owned();
    let menu = MenuLabels {
        title: text("menu-title"),
        buttons: [
            text("menu-play-button"),
            text("menu-settings-button"),
            text("menu-exit-button"),
        ],
    };
    let song_select = SongSelectLabels {
        no_folder: text("song-select-no-folder"),
        failed: text("song-select-failed"),
        loading: text("song-select-loading"),
        empty: text("song-select-empty"),
        choose_folder: text("song-select-choose-folder"),
        hint: text("song-select-hint"),
        no_audio: text("song-select-no-audio"),
        search_placeholder: text("song-select-search-placeholder"),
        search_no_matches: text("song-select-search-no-matches"),
        mods_title: text("song-select-mods-title"),
        mods_hint: text("song-select-mods-hint"),
        mods_customise: text("song-select-mods-customise"),
        setting_texts: mod_settings::TEXT_KEYS
            .iter()
            .map(|key| (*key, text(key)))
            .collect(),
        mod_column_titles: [
            text("song-select-mods-reduction"),
            text("song-select-mods-increase"),
            text("song-select-mods-conversion"),
        ],
        details: DetailLabels {
            length: text("song-select-details-length"),
            bpm: text("song-select-details-bpm"),
            circles: text("song-select-details-circles"),
            sliders: text("song-select-details-sliders"),
            spinners: text("song-select-details-spinners"),
            circle_size: text("song-select-details-circle-size"),
            approach_rate: text("song-select-details-approach-rate"),
            overall_difficulty: text("song-select-details-overall-difficulty"),
            drain_rate: text("song-select-details-drain-rate"),
        },
        mod_columns: mod_columns(),
    };
    let results = ResultsLabels {
        score: text("results-score-title"),
        accuracy: text("results-accuracy-title"),
        max_combo: text("results-max-combo-title"),
        mods: text("results-mods-title"),
        no_mods: text("results-no-mods"),
        average_hit_error: text("results-average-hit-error-title"),
        unstable_rate: text("results-unstable-rate-title"),
        not_available: text("results-not-available"),
        stats: StatKind::ALL.map(|kind| text(results_label_key(kind))),
        retry: text("results-retry-button"),
        back: text("results-back-button"),
        hint: text("results-hint"),
    };
    let fps = crate::play::scene::hud_labels(localizer).fps;
    InterfaceViews {
        menu: MenuView::new(Arc::clone(registry), theme, &menu),
        song_select: SongSelectView::new(Arc::clone(registry), theme, &song_select),
        results: ResultsView::new(Arc::clone(registry), theme, &results),
        settings: SettingsView::new(
            Arc::clone(registry),
            theme,
            &rows.specs(settings),
            &rows.labels(localizer),
        ),
        fps: FpsView::new(Arc::clone(registry), &fps),
        theme,
        switch: 0,
    }
}

impl InterfaceViews {
    /// Lays every screen out for a `width` × `height` window ahead of its first frame there.
    /// Allocates: call it off the hot paths.
    fn lay_out_for(&mut self, width: f32, height: f32) {
        self.menu.lay_out_for(width, height);
        self.song_select.lay_out_for(width, height);
        self.results.lay_out_for(width, height);
        self.settings.lay_out_for_panel(width, height);
    }
}

/// Moves the dropdowns of `rows` from `old` to `new`, which replaces it, and gives `old`
/// those of `new`: lists that came after `new` was laid out stay shown, with as many
/// options as the update thread's rows (ADR-0091). Does not allocate.
pub fn carry_dropdowns(old: &mut SettingsView, new: &mut SettingsView, rows: &[usize]) {
    for &row in rows {
        if let Some(view) = old.dropdown_mut(row) {
            new.swap_dropdown(row, view);
        }
    }
}

/// Views numbered by the switch they were laid out for.
pub trait Numbered {
    fn switch(&self) -> u64;
}

impl Numbered for Box<InterfaceViews> {
    fn switch(&self) -> u64 {
        self.switch
    }
}

/// The draw thread's side of a switch: closes the curtain for each new switch, holds views
/// until it is closed and opens it once those of the last switch show (ADR-0091).
/// Does not allocate.
pub struct LanguageSwap<V> {
    curtain: Curtain,
    /// Views that arrived while the curtain was not closed yet.
    held: Option<V>,
    /// The last switch the curtain closed for, and the one whose views show.
    seen: u64,
    shown: u64,
}

impl<V: Numbered> Default for LanguageSwap<V> {
    fn default() -> LanguageSwap<V> {
        LanguageSwap {
            curtain: Curtain::new(),
            held: None,
            seen: 0,
            shown: 0,
        }
    }
}

impl<V: Numbered> LanguageSwap<V> {
    /// Takes the snapshot's switch number at `now_ns` and the newest views that `arrived`;
    /// returns views they replace, to give back.
    /// Views of a switch the snapshot has not shown yet close the curtain for it too: the
    /// two may arrive in either order.
    pub fn receive(&mut self, now_ns: u64, switch: u64, arrived: Option<V>) -> Option<V> {
        let newest = arrived
            .as_ref()
            .map_or(switch, |views| views.switch().max(switch));
        if newest > self.seen {
            self.seen = newest;
            self.curtain.close(now_ns);
        }
        arrived.and_then(|views| self.held.replace(views))
    }

    /// The views to swap in now: behind the closed curtain, or at once when it is not shown.
    pub fn take(&mut self) -> Option<V> {
        if !(self.curtain.is_closed() || !self.curtain.is_shown()) {
            return None;
        }
        let views = self.held.take()?;
        self.shown = views.switch();
        Some(views)
    }

    /// Moves the curtain on to `now_ns`: it opens once the last switch's views show and
    /// `glyphs_pending` (asked only while closed) says their glyphs are made.
    pub fn advance(&mut self, now_ns: u64, glyphs_pending: impl FnOnce() -> usize) {
        let ready = self.curtain.is_closed() && self.shown == self.seen && glyphs_pending() == 0;
        self.curtain.advance(now_ns, ready);
    }

    pub fn curtain(&self) -> &Curtain {
        &self.curtain
    }
}

/// What the views are laid out from: the settings screen's options and values.
pub struct Rebuild {
    pub rows: Rows,
    pub settings: Settings,
    /// The theme to lay the views out with (ADR-0117, ADR-0118).
    pub theme: Theme,
    /// Numbers the switch, from 1.
    pub switch: u64,
}

/// The update thread's end: asks for views in the language just switched to.
pub struct LanguageLink {
    requests: rtrb::Producer<Box<Rebuild>>,
}

impl LanguageLink {
    /// Queues `rebuild`; gives it back when the ring is full, to try again next tick.
    /// Does not allocate.
    pub fn request(&mut self, rebuild: Box<Rebuild>) -> Result<(), Box<Rebuild>> {
        self.requests.push(rebuild).map_err(|e| match e {
            rtrb::PushError::Full(rebuild) => rebuild,
        })
    }
}

/// The draw thread's end: views in, old views back.
pub struct LanguageViews {
    views: rtrb::Consumer<Box<InterfaceViews>>,
    old: rtrb::Producer<Box<InterfaceViews>>,
}

impl LanguageViews {
    /// The newest views, if any arrived; older ones still queued go straight back.
    /// Does not allocate.
    pub fn take(&mut self) -> Option<Box<InterfaceViews>> {
        let mut newest = None;
        while let Ok(views) = self.views.pop() {
            if let Some(older) = newest.replace(views) {
                self.give_back(older);
            }
        }
        newest
    }

    /// Hands `views` to the thread to free. Does not allocate; with the ring full (the
    /// thread stopped) they are kept rather than freed here.
    pub fn give_back(&mut self, views: Box<InterfaceViews>) {
        if let Err(rtrb::PushError::Full(views)) = self.old.push(views) {
            // The draw thread must not free: they leak, until the process exits.
            std::mem::forget(views);
        }
    }
}

/// The `slam-language` thread; stops and joins on drop.
pub struct LanguageThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for LanguageThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The rings between the threads, without a thread: the thread's ends come back for tests.
#[allow(clippy::type_complexity)]
fn rings() -> (
    LanguageLink,
    LanguageViews,
    (
        rtrb::Consumer<Box<Rebuild>>,
        rtrb::Producer<Box<InterfaceViews>>,
        rtrb::Consumer<Box<InterfaceViews>>,
    ),
) {
    let (requests, requests_rx) = rtrb::RingBuffer::new(RING);
    let (views_tx, views) = rtrb::RingBuffer::new(RING);
    // Room for every set the draw thread may hold back at once.
    let (old, old_rx) = rtrb::RingBuffer::new(RING * 2);
    (
        LanguageLink { requests },
        LanguageViews { views, old },
        (requests_rx, views_tx, old_rx),
    )
}

/// Starts the `slam-language` thread, laying out views with `registry` in the language of
/// `localizer` at the time each request is taken, for the window size in `surface` (packed
/// as [`crate::play::pack_size`]).
pub fn spawn(
    registry: Arc<FontRegistry>,
    localizer: Arc<Localizer>,
    surface: Arc<AtomicU64>,
) -> std::io::Result<(LanguageLink, LanguageViews, LanguageThread)> {
    let (link, views, (mut requests, mut out, mut old)) = rings();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("slam-language".to_owned())
        .spawn(move || {
            let mut waiting: Option<Box<Rebuild>> = None;
            while !flag.load(Ordering::Relaxed) {
                while old.pop().is_ok() {}
                while let Ok(rebuild) = requests.pop() {
                    waiting = Some(rebuild);
                }
                if out.slots() > 0
                    && let Some(rebuild) = waiting.take()
                {
                    let mut views = interface_views(
                        &registry,
                        rebuild.theme,
                        &localizer,
                        &rebuild.rows,
                        &rebuild.settings,
                    );
                    let (width, height) = crate::play::unpack_size(surface.load(Ordering::Relaxed));
                    // A minimised window reports 0×0: the views lay out when it comes back.
                    if width != 0 && height != 0 {
                        views.lay_out_for(width as f32, height as f32);
                    }
                    views.switch = rebuild.switch;
                    // A slot is free and this thread is the only producer.
                    let _ = out.push(Box::new(views));
                    continue;
                }
                std::thread::sleep(POLL);
            }
        })?;
    Ok((
        link,
        views,
        LanguageThread {
            stop,
            thread: Some(thread),
        },
    ))
}

/// The language applied to the shared localizer, on the update thread.
pub struct AppliedLanguage {
    localizer: Arc<Localizer>,
    /// `general.language` as last applied.
    applied: Option<String>,
    link: Option<LanguageLink>,
    /// A request the full ring refused, sent again next tick.
    pending: Option<Box<Rebuild>>,
    /// Switches that asked for views so far.
    switches: u64,
    /// The interface theme (colour scheme, menu scale) the views were last asked with.
    theme: Theme,
}

impl AppliedLanguage {
    /// `localizer` already has `applied` active; `link` reaches the `slam-language` thread
    /// (`None` without it: the language still switches for texts formatted on use).
    pub fn new(
        localizer: Arc<Localizer>,
        applied: Option<String>,
        link: Option<LanguageLink>,
        theme: Theme,
    ) -> AppliedLanguage {
        AppliedLanguage {
            localizer,
            applied,
            link,
            pending: None,
            switches: 0,
            theme,
        }
    }

    /// Whether a thread lays views out again: without one, nothing the draw side shows
    /// changes with the language or theme.
    pub fn lays_out_views(&self) -> bool {
        self.link.is_some()
    }

    /// How many switches asked for views so far; the views carry the number of theirs.
    pub fn switches(&self) -> u64 {
        self.switches
    }

    /// Switches the localizer when `settings` (with the settings screen's edits) name another
    /// language and asks for views laid out in it, with the settings screen showing `rows`;
    /// asks for views also when they name another colour scheme or menu scale (ADR-0117,
    /// ADR-0118). Allocates only on a switch, which follows the player's pick.
    /// Returns whether the language switched.
    pub fn apply(&mut self, settings: &Settings, rows: &Rows, theme: Theme) -> bool {
        let mut switched = false;
        let restyled = theme != self.theme;
        self.theme = theme;
        if settings.general.language != self.applied {
            self.applied.clone_from(&settings.general.language);
            let tag = self.applied.as_deref().unwrap_or(FALLBACK_LANGUAGE);
            if let Err(e) = self.localizer.set_language(tag) {
                // The file reader keeps unknown tags out; nothing to lay out again.
                tracing::warn!("settings: general.language: {e}");
            } else {
                switched = true;
            }
        }
        if (switched || restyled) && self.link.is_some() {
            self.switches += 1;
            // A request the full ring refused is freed here: a player's pick, as a commit
            // of the settings is.
            self.pending = Some(Box::new(Rebuild {
                rows: rows.clone(),
                settings: settings.clone(),
                theme,
                switch: self.switches,
            }));
        }
        if let Some(link) = &mut self.link
            && let Some(rebuild) = self.pending.take()
            && let Err(rebuild) = link.request(rebuild)
        {
            self.pending = Some(rebuild);
        }
        switched
    }
}

#[cfg(test)]
mod tests;
