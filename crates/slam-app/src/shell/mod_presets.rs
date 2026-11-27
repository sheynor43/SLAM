//! The song select footer's mod presets and the right-click on a mod tile (ADR-0175,
//! ADR-0177): the presets live in the settings (`song_select.mod_presets`), the footer shows
//! their labels, and the shell applies, pins and removes them.

use std::sync::Arc;

use super::ShellState;
use crate::mod_presets::{MAX, encode, label, parse, same};

impl ShellState {
    /// Gives the footer the presets in the settings; call after they changed.
    pub(super) fn sync_presets(&mut self) {
        let labels: Arc<[Arc<str>]> = self
            .settings
            .song_select
            .mod_presets
            .iter()
            .map(|p| Arc::from(label(p)))
            .collect();
        self.song_select.set_mod_presets(labels);
        self.preset_mods = self
            .settings
            .song_select
            .mod_presets
            .iter()
            .map(|p| parse(p).unwrap_or_default())
            .collect();
        self.sync_pinned();
    }

    /// Tells the footer whether the selected mods are a preset already.
    pub(super) fn sync_pinned(&mut self) {
        // Compared as mods, in no order, without allocating: this runs at every toggle.
        let mods = self.selected_mods.mods();
        let current = (!mods.is_empty())
            .then(|| {
                self.preset_mods
                    .iter()
                    .position(|p| p.len() == mods.len() && mods.iter().all(|m| p.contains(m)))
            })
            .flatten();
        self.song_select.set_current_preset(current);
    }

    /// Changes the presets (the settings in force and the settings screen's edit of them),
    /// saves and tells the footer.
    fn edit_presets(&mut self, change: impl Fn(&mut Vec<String>)) {
        change(&mut self.settings.song_select.mod_presets);
        if let Some(editing) = &mut self.editing {
            change(&mut editing.song_select.mod_presets);
        }
        self.save_settings();
        self.sync_presets();
    }

    /// The footer's "+": pins the selected mods.
    pub(super) fn pin_preset(&mut self) {
        let mods = self.selected_mods.mods();
        if mods.is_empty() {
            return;
        }
        let text = encode(mods);
        let presets = &self.settings.song_select.mod_presets;
        if presets.len() >= MAX || presets.iter().any(|p| same(p, &text)) {
            return;
        }
        self.edit_presets(|presets| presets.push(text.clone()));
    }

    /// Right click on a pinned preset: removes it.
    pub(super) fn remove_preset(&mut self, index: usize) {
        if index < self.settings.song_select.mod_presets.len() {
            self.edit_presets(|presets| {
                if index < presets.len() {
                    presets.remove(index);
                }
            });
        }
    }

    /// Click on a pinned preset: selects its mods, as they were pinned. Not while a play
    /// loads.
    pub(super) fn apply_preset(&mut self, index: usize) {
        if self.loading {
            return;
        }
        let Some(mods) = self.preset_mods.get(index).filter(|m| !m.is_empty()) else {
            return;
        };
        self.selected_mods.apply_selection(mods);
        self.sync_mod_flags();
    }

    /// Right click on a mod tile at `at`: shows the mod's settings there, selecting the mod
    /// first when it is not selected. Does nothing for a mod without settings.
    pub(super) fn show_mod_settings(&mut self, column: usize, index: usize, at: (f32, f32)) {
        if self.loading {
            return;
        }
        let Some(template) = self
            .mod_templates
            .get(column)
            .and_then(|mods| mods.get(index))
        else {
            return;
        };
        if template.setting_infos().is_empty() {
            return;
        }
        let selected = self.song_select.mods().is_active(column, index);
        if !selected {
            self.toggle_mod(column, index);
        }
        self.song_select
            .mods_mut()
            .open_customisation_at(at.0, at.1);
    }
}
