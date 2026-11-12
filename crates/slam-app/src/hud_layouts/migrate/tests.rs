use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

fn temp_store(tag: &str) -> (LayoutStore, std::path::PathBuf) {
    static N: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "slam-hud-migrate-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    (LayoutStore::new(root.join("hud-layouts")), root)
}

fn meter_scales(layout: &HudLayout) -> Vec<[f32; 2]> {
    layout
        .gameplay
        .iter()
        .filter(|e| e.component == ComponentKind::HitErrorMeter)
        .map(|e| e.scale)
        .collect()
}

fn name(preset: &str, scale: f64) -> String {
    format!("{preset} x{scale}")
}

#[test]
fn scaling_touches_only_the_meters() {
    let mut layout = Builtin::Stable.layout();
    let before = layout.clone();
    assert!(!meter_scales(&before).is_empty());
    scale_hit_error_meters(&mut layout, 1.5);
    for (old, new) in before.gameplay.iter().zip(&layout.gameplay) {
        if old.component == ComponentKind::HitErrorMeter {
            assert_eq!(new.scale, [old.scale[0] * 1.5, old.scale[1] * 1.5]);
        } else {
            assert_eq!(old, new);
        }
    }
}

#[test]
fn a_builtin_layout_is_copied_scaled_and_selected() {
    let (store, root) = temp_store("builtin");
    let mut settings = Settings::default();
    assert!(migrate_hit_error_scale(&store, &mut settings, 1.5, name).unwrap());
    let id = LayoutId::parse(&settings.hud.layout).unwrap();
    assert!(!id.is_builtin());
    let copy = store.load(&id).unwrap();
    assert_eq!(
        copy.name,
        "Stable x1.5".replace("Stable", &Builtin::Stable.layout().name)
    );
    let original = Builtin::Stable.layout();
    let expected: Vec<[f32; 2]> = meter_scales(&original)
        .iter()
        .map(|s| [s[0] * 1.5, s[1] * 1.5])
        .collect();
    assert_eq!(meter_scales(&copy), expected);
    // The built-in preset itself is untouched.
    assert_eq!(Builtin::Stable.layout(), original);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scale_of_one_changes_nothing() {
    let (store, root) = temp_store("one");
    let mut settings = Settings::default();
    assert!(!migrate_hit_error_scale(&store, &mut settings, 1.0, name).unwrap());
    assert_eq!(settings, Settings::default());
    assert_eq!(store.list().len(), Builtin::ALL.len());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_user_layout_is_scaled_in_place() {
    let (store, root) = temp_store("user");
    let id = store
        .duplicate(&LayoutId::parse("builtin:lazer").unwrap(), "Mine")
        .unwrap();
    let before = meter_scales(&store.load(&id).unwrap());
    let mut settings = Settings::default();
    settings.hud.layout = id.as_str().to_owned();
    assert!(!migrate_hit_error_scale(&store, &mut settings, 2.0, name).unwrap());
    assert_eq!(settings.hud.layout, id.as_str());
    let after = meter_scales(&store.load(&id).unwrap());
    let expected: Vec<[f32; 2]> = before.iter().map(|s| [s[0] * 2.0, s[1] * 2.0]).collect();
    assert_eq!(after, expected);
    assert_eq!(store.list().len(), Builtin::ALL.len() + 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_user_layout_fails_and_startup_keeps_going() {
    let (store, root) = temp_store("missing");
    let mut settings = Settings::default();
    settings.hud.layout = "gone".into();
    let localizer = Localizer::builtin().unwrap();
    assert!(!migrate_on_startup(&store, &mut settings, 1.5, &localizer));
    assert_eq!(settings.hud.layout, "gone");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_migrated_name_is_localised() {
    let localizer = Localizer::builtin().unwrap();
    localizer.set_language("en").unwrap();
    let en = migrated_name(&localizer, "Stable", 1.5);
    assert!(
        en.contains("Stable") && en.contains("1.5") && en.contains("hit error"),
        "{en}"
    );
    localizer.set_language("ru").unwrap();
    let ru = migrated_name(&localizer, "Stable", 1.5);
    assert!(
        ru.contains("Stable") && ru.contains("1.5") && ru.contains("шкала"),
        "{ru}"
    );
}

#[test]
fn resolve_falls_back_to_stable() {
    let (store, root) = temp_store("resolve");
    let stable = Builtin::Stable.layout();
    assert_eq!(resolve(Some(&store), "gone"), stable);
    assert_eq!(resolve(Some(&store), "../bad id"), stable);
    assert_eq!(resolve(None, "gone"), stable);
    assert_eq!(resolve(None, "builtin:lazer"), Builtin::Lazer.layout());
    let id = store
        .duplicate(&LayoutId::parse("builtin:slam").unwrap(), "Mine")
        .unwrap();
    assert_eq!(resolve(Some(&store), id.as_str()).name, "Mine");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_second_start_after_a_failed_settings_save_does_not_migrate_again() {
    // Built-in active: the copy of the first start is selected, not copied again.
    let (store, root) = temp_store("again-builtin");
    let mut settings = Settings::default();
    assert!(migrate_hit_error_scale(&store, &mut settings, 1.5, name).unwrap());
    let first = settings.hud.layout.clone();
    let mut unsaved = Settings::default();
    assert!(migrate_hit_error_scale(&store, &mut unsaved, 1.5, name).unwrap());
    assert_eq!(unsaved.hud.layout, first);
    assert_eq!(store.list().iter().filter(|e| !e.builtin).count(), 1);
    let id = LayoutId::parse(&first).unwrap();
    assert_eq!(meter_scales(&store.load(&id).unwrap()), vec![[1.5, 1.5]]);
    let _ = std::fs::remove_dir_all(&root);

    // A user layout is scaled once.
    let (store, root) = temp_store("again-user");
    let id = store
        .duplicate(&LayoutId::parse("builtin:stable").unwrap(), "mine")
        .unwrap();
    let mut settings = Settings::default();
    settings.hud.layout = id.as_str().to_owned();
    for _ in 0..2 {
        migrate_hit_error_scale(&store, &mut settings, 2.0, name).unwrap();
    }
    assert_eq!(meter_scales(&store.load(&id).unwrap()), vec![[2.0, 2.0]]);
    let _ = std::fs::remove_dir_all(&root);
}

/// A copy of the old preset as the migration made it, scaled by `scale`.
fn old_copy(store: &LayoutStore, scale: f64) -> LayoutId {
    let id = store
        .duplicate(&LayoutId::parse("builtin:stable").unwrap(), "old copy")
        .unwrap();
    let mut layout = store.load(&id).unwrap();
    layout.gameplay = serde_json::from_str(PRESET_GAMEPLAY_V1).unwrap();
    scale_hit_error_meters(&mut layout, scale);
    mark_migrated(&mut layout, scale);
    store.save(&id, &layout).unwrap();
    id
}

#[test]
fn untouched_copies_of_the_old_preset_take_the_current_one() {
    let (store, root) = temp_store("refresh");
    let id = old_copy(&store, 1.3);
    // One the user moved something in is left alone.
    let edited = old_copy(&store, 1.3);
    let mut layout = store.load(&edited).unwrap();
    layout.gameplay[1].position = [-20.0, 0.0];
    store.save(&edited, &layout).unwrap();
    let before_edited = store.load(&edited).unwrap();

    assert_eq!(refresh_migrated_copies(&store), 1);
    let refreshed = store.load(&id).unwrap();
    let mut expected = Builtin::Stable.layout();
    scale_hit_error_meters(&mut expected, 1.3);
    assert_eq!(refreshed.gameplay, expected.gameplay);
    assert!(refreshed.extra.contains_key(MIGRATED));
    assert_eq!(store.load(&edited).unwrap(), before_edited);
    // Done once.
    assert_eq!(refresh_migrated_copies(&store), 0);
    let _ = std::fs::remove_dir_all(root);
}
