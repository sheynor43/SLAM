//! The screen before play in the shell (ADR-0190): it comes in over the song select when a
//! beatmap is played, shows the card and the quick settings while the play is prepared,
//! and hands over to the play once the play is ready and the screen may go.
//!
//! The quick settings edit a copy of the settings, as the toolbar's popups do (ADR-0168);
//! the copy is committed when the screen goes. A play prepared before a change is prepared
//! again, so that the settings read at preparation count too.

use std::sync::Arc;

use slam_osu::mods::GameplayMod;
use slam_ui::player_loader::{
    LoaderCard, LoaderEvent, LoaderLayout, PlayerLoader, QuickGroup, QuickRow,
};
use slam_ui::screen::TransitionStyle;
use slam_ui::settings_screen::{RowValue, SettingsInput};
use slam_ui::song_select::ScoreMods;

use super::rows::Row;
use super::*;
use crate::menu_sounds::MenuSound;

/// The quick settings, top to bottom.
const QUICK: [(Row, QuickGroup); 9] = [
    (Row::Dim, QuickGroup::Visual),
    (Row::CursorSize, QuickGroup::Visual),
    (Row::BeatmapSkins, QuickGroup::Visual),
    (Row::BeatmapColours, QuickGroup::Visual),
    (Row::MusicVolume, QuickGroup::Audio),
    (Row::HitsoundVolume, QuickGroup::Audio),
    (Row::GlobalOffset, QuickGroup::Audio),
    (Row::BeatmapHitsounds, QuickGroup::Audio),
    (Row::MouseButtons, QuickGroup::Input),
];

/// The source as the card shows it: left out when the interface's fonts cannot draw it.
/// There is no system fallback for CJK yet (KNOWN_ISSUES), and a row of empty boxes says
/// nothing.
fn shown_source(source: &str) -> &str {
    let drawable = |c: char| {
        !matches!(
            u32::from(c),
            0x1100..=0x11FF       // Hangul Jamo
                | 0x2E80..=0x9FFF // CJK radicals, kana, ideographs
                | 0xA960..=0xA97F
                | 0xAC00..=0xD7FF // Hangul syllables
                | 0xF900..=0xFAFF // CJK compatibility ideographs
                | 0xFF00..=0xFFEF // half- and full-width forms
                | 0x20000..=0x3FFFF
        )
    };
    if source.chars().all(drawable) {
        source.trim()
    } else {
        ""
    }
}

impl ShellState {
    /// The screen before play for difficulty `difficulty` of set `set`, when the setting
    /// shows it; `None` plays at once as before. Allocates the card's texts (navigation).
    pub(super) fn enter_loader(
        &mut self,
        set: usize,
        difficulty: usize,
        now_ns: u64,
    ) -> Option<Nav<AppScreen>> {
        if !self.settings.gameplay.player_loader {
            return None;
        }
        let row = self.song_select.list().sets.get(set)?;
        let diff = row.difficulties.get(difficulty)?;
        self.loader_key = self.loader_key.wrapping_add(1);
        self.loader_card = LoaderCard {
            key: self.loader_key,
            title: Arc::from(row.title.as_str()),
            artist: Arc::from(row.artist.as_str()),
            version: Arc::from(diff.version.as_str()),
            source: Arc::from(shown_source(&diff.source)),
            mapper: Arc::from(row.creator.as_str()),
            stars: diff.stars,
            mods: ScoreMods::from_acronyms(self.selected_mods.mods().iter().map(|m| m.acronym())),
        };
        let quick: Vec<QuickRow> = QUICK
            .iter()
            .filter_map(|&(row, group)| {
                let widget = self.rows.quick_widget(row, &self.settings)?;
                let index = Row::ALL.iter().position(|r| *r == row)?;
                Some(QuickRow {
                    row: index as u16,
                    group,
                    widget,
                })
            })
            .collect();
        self.close_popup();
        self.editing = Some(self.settings.clone());
        self.loader_editing = true;
        self.loader_dirty = false;
        Some(Nav::Push(AppScreen::Loader(PlayerLoader::new(
            &quick, now_ns,
        ))))
    }

    /// The layout of the screen before play on top, if it is.
    fn loader_layout(&self) -> Option<LoaderLayout> {
        let AppScreen::Loader(loader) = self.stack.top() else {
            return None;
        };
        let (rows, count) = loader.row_list();
        Some(LoaderLayout::new(
            self.size[0],
            self.size[1],
            &self.theme,
            &rows[..count],
        ))
    }

    /// Input for the screen before play; the navigation it asks for.
    pub(super) fn loader_input(&mut self, input: ScreenInput) -> Option<Nav<AppScreen>> {
        let layout = self.loader_layout()?;
        let AppScreen::Loader(loader) = self.stack.top_mut() else {
            return None;
        };
        let event = match input {
            ScreenInput::Common(SongSelectInput::PointerMove { x, y }) => {
                loader.pointer(x, y, &layout)
            }
            ScreenInput::Common(SongSelectInput::PointerPress { x, y }) => {
                loader.press(x, y, &layout)
            }
            ScreenInput::Release => loader.release(&layout),
            ScreenInput::Common(input) | ScreenInput::Repeat(input) => {
                let key = match input {
                    SongSelectInput::Left => SettingsInput::Left,
                    SongSelectInput::Right => SettingsInput::Right,
                    SongSelectInput::Activate => SettingsInput::Activate,
                    SongSelectInput::Back => SettingsInput::Back,
                    SongSelectInput::DeselectMods | SongSelectInput::Erase => SettingsInput::Erase,
                    _ => return None,
                };
                loader.key(key, &layout)
            }
            _ => None,
        };
        match event? {
            LoaderEvent::Absorbed => None,
            LoaderEvent::Changed { row, value } => {
                self.ui_sounds.push(match value {
                    RowValue::Toggle(true) => MenuSound::ToggleOn,
                    RowValue::Toggle(false) => MenuSound::ToggleOff,
                    RowValue::Slider(_) => MenuSound::SliderStep,
                    RowValue::Choice(_) => MenuSound::Click,
                });
                if let Some(editing) = &mut self.editing {
                    self.rows.apply(editing, row, value);
                }
                self.loader_dirty = true;
                None
            }
            LoaderEvent::Back => {
                // The play being prepared is dropped.
                self.play_request = None;
                self.set_loading(false);
                self.commit_loader();
                Some(Nav::Pop)
            }
        }
    }

    /// Commits what the quick settings changed, when the screen goes another way.
    pub(super) fn commit_loader_now(&mut self) {
        self.commit_loader();
    }

    /// Commits what the quick settings changed, once.
    fn commit_loader(&mut self) {
        if !std::mem::take(&mut self.loader_editing) {
            return;
        }
        if commit_settings(
            &mut self.settings,
            &mut self.editing,
            &mut self.save_request,
        ) {
            self.settings_changed = true;
        }
    }

    /// Once per tick: the screen before play starts going when it may. A play prepared
    /// before the quick settings changed is asked for again with them.
    pub(super) fn tick_loader(&mut self, now_ns: u64) {
        let theme = self.theme;
        let dirty = self.loader_dirty;
        let AppScreen::Loader(loader) = self.stack.top_mut() else {
            return;
        };
        if !loader.may_leave(now_ns, &theme) {
            return;
        }
        if dirty {
            loader.set_prepared(false);
            self.loader_dirty = false;
            self.commit_loader();
            self.editing = Some(self.settings.clone());
            self.loader_editing = true;
            self.play_request = self.last_play;
            return;
        }
        loader.leave(now_ns);
        self.commit_loader();
    }

    /// The window's play host has the play prepared: the screen before play may go.
    pub fn set_play_prepared(&mut self) {
        if let AppScreen::Loader(loader) = self.stack.top_mut() {
            loader.set_prepared(true);
        }
    }

    /// Whether a prepared play may start at `now_ns`: at once, or once the screen before
    /// play has gone out.
    pub fn play_may_start(&self, now_ns: u64) -> bool {
        match self.stack.top() {
            AppScreen::Loader(loader) => loader.gone(now_ns, &self.theme),
            _ => true,
        }
    }

    /// The play comes in over the screen before play, in its place, out of the black the
    /// screen went to (ADR-0191), or at once when transitions are off.
    pub(super) fn replace_loader(&mut self, play: AppScreen, now_ns: u64) {
        let style = self.live_settings().general.transitions;
        if style != TransitionStyle::None {
            self.stack.set_style(TransitionStyle::Fade);
        }
        self.stack.navigate(Nav::Replace(play), now_ns);
        // The screen went to black itself: the play comes in from black (ADR-0191).
        self.stack
            .advance_transition(slam_ui::theme::transition::FADE_BLACK.1, now_ns);
        self.stack.set_style(style);
    }

    /// The card of the screen before play, for the snapshot.
    pub fn loader_card(&self) -> &LoaderCard {
        &self.loader_card
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod source_tests {
    use super::shown_source;

    #[test]
    fn a_source_the_fonts_cannot_draw_is_left_out() {
        assert_eq!(shown_source("Touhou"), "Touhou");
        assert_eq!(shown_source("  Ateliér "), "Ateliér");
        assert_eq!(shown_source("東方Project"), "");
        assert_eq!(shown_source("ぼっち・ざ・ろっく！"), "");
        assert_eq!(shown_source("블루 아카이브"), "");
        assert_eq!(shown_source(""), "");
    }
}
