//! One-time move of the retired `gameplay.hit_error_scale` setting into a HUD layout
//! (ADR-0109), and the lookup of the active layout for a play.

use slam_ui::hud_layout::{Builtin, ComponentKind, HudLayout};
use slam_ui::i18n::{FluentArgs, Localizer};

use super::{LayoutId, LayoutStore};
use crate::settings::Settings;

/// The layout `id` names; the stable preset when the id is unusable or the layout cannot be
/// read. Cold path: a play calls it while it loads.
pub fn resolve(store: Option<&LayoutStore>, id: &str) -> HudLayout {
    let Some(parsed) = LayoutId::parse(id) else {
        tracing::warn!("hud: `{id}` is not a layout id, using the stable layout");
        return Builtin::Stable.layout();
    };
    let loaded = match (store, parsed.builtin()) {
        (_, Some(builtin)) => Ok(builtin.layout()),
        (Some(store), None) => store.load(&parsed),
        (None, None) => Err(super::StoreError::NotFound(parsed)),
    };
    loaded.unwrap_or_else(|e| {
        tracing::warn!("hud: cannot load layout `{id}`: {e}, using the stable layout");
        Builtin::Stable.layout()
    })
}

/// Multiplies the scale of every hit error meter of the gameplay screen by `factor`.
/// The top-level field a layout gets once the old setting is moved into it, so that a
/// start that cannot save the settings afterwards does not move it again. Unknown fields
/// survive edits (ADR-0109).
pub const MIGRATED: &str = "migrated_hit_error_scale";

fn mark_migrated(layout: &mut HudLayout, scale: f64) {
    layout.extra.insert(MIGRATED.to_owned(), scale.into());
}

pub fn scale_hit_error_meters(layout: &mut HudLayout, factor: f64) {
    let factor = factor as f32;
    for element in &mut layout.gameplay {
        if element.component == ComponentKind::HitErrorMeter {
            element.scale[0] *= factor;
            element.scale[1] *= factor;
        }
    }
}

/// Moves the old meter size into the layout of `settings`: a built-in layout is copied into a
/// new user layout (named by `name`, given the preset's name) and the setting switches to it;
/// a user layout is scaled in place. Returns whether `settings` changed. A value of 1 is
/// nothing to move.
pub fn migrate_hit_error_scale(
    store: &LayoutStore,
    settings: &mut Settings,
    scale: f64,
    name: impl FnOnce(&str, f64) -> String,
) -> Result<bool, super::StoreError> {
    if (scale - 1.0).abs() < 1e-9 {
        return Ok(false);
    }
    let active = LayoutId::parse(&settings.hud.layout).ok_or(super::StoreError::InvalidId)?;
    if active.is_builtin() {
        // A start before took the scale but could not save the settings: select its copy
        // instead of making another.
        let done = store.list().into_iter().find(|entry| {
            !entry.builtin
                && store.load(&entry.id).is_ok_and(|layout| {
                    layout.extra.get(MIGRATED).and_then(|v| v.as_f64()) == Some(scale)
                })
        });
        if let Some(entry) = done {
            settings.hud.layout = entry.id.as_str().to_owned();
            return Ok(true);
        }
        let preset = store.load(&active)?;
        let new_id = store.duplicate(&active, &name(&preset.name, scale))?;
        let result = store.load(&new_id).and_then(|mut layout| {
            scale_hit_error_meters(&mut layout, scale);
            mark_migrated(&mut layout, scale);
            store.save(&new_id, &layout)
        });
        if let Err(e) = result {
            // Do not leave a half-made copy behind.
            let _ = store.delete(&new_id);
            return Err(e);
        }
        settings.hud.layout = new_id.as_str().to_owned();
        Ok(true)
    } else {
        let mut layout = store.load(&active)?;
        // Scaled by a start before that could not save the settings: not again.
        if !layout.extra.contains_key(MIGRATED) {
            scale_hit_error_meters(&mut layout, scale);
            mark_migrated(&mut layout, scale);
            store.save(&active, &layout)?;
        }
        Ok(false)
    }
}

/// The name of the layout a built-in one is copied into, in the interface language.
pub fn migrated_name(localizer: &Localizer, preset: &str, scale: f64) -> String {
    let mut args = FluentArgs::new();
    args.set("name", preset.to_owned());
    args.set("scale", scale);
    localizer
        .format("hud-layout-migrated-name", Some(&args))
        .into_owned()
}

/// Runs the migration for a settings file that still has the old key; any failure is a
/// warning. Returns whether `settings` should be written back, which drops the old key.
pub fn migrate_on_startup(
    store: &LayoutStore,
    settings: &mut Settings,
    scale: f64,
    localizer: &Localizer,
) -> bool {
    match migrate_hit_error_scale(store, settings, scale, |preset, scale| {
        migrated_name(localizer, preset, scale)
    }) {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!("hud: cannot move the hit error meter size into a layout: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests;
