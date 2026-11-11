use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

fn temp_store(tag: &str) -> (LayoutStore, PathBuf) {
    static N: AtomicU32 = AtomicU32::new(0);
    let root = std::env::temp_dir().join(format!(
        "slam-hud-layouts-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    (LayoutStore::new(root.join("hud-layouts")), root)
}

fn id(s: &str) -> LayoutId {
    LayoutId::parse(s).unwrap()
}

#[test]
fn no_root_lists_only_builtins() {
    let (store, root) = temp_store("empty");
    let list = store.list();
    let ids: Vec<_> = list.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["builtin:stable", "builtin:lazer", "builtin:slam"]);
    assert!(list.iter().all(|e| e.builtin));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn duplicating_a_builtin_makes_an_editable_copy() {
    let (store, root) = temp_store("dup");
    let new = store.duplicate(&id("builtin:lazer"), "My HUD!").unwrap();
    assert_eq!(new.as_str(), "my-hud");
    assert!(store.sprites_dir(&new).unwrap().is_dir());
    let list = store.list();
    assert_eq!(list.len(), 4);
    assert_eq!(list[3].id, new);
    assert!(!list[3].builtin);
    let mut copy = store.load(&new).unwrap();
    assert_eq!(copy.name, "My HUD!");
    let mut original = Builtin::Lazer.layout();
    copy.name.clear();
    original.name.clear();
    assert_eq!(copy, original);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn duplicating_twice_suffixes_the_id() {
    let (store, root) = temp_store("twice");
    let a = store.duplicate(&id("builtin:stable"), "x").unwrap();
    let b = store.duplicate(&id("builtin:stable"), "x").unwrap();
    assert_eq!((a.as_str(), b.as_str()), ("x", "x-2"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn duplicate_copies_sprites_but_not_folders() {
    let (store, root) = temp_store("sprites");
    let a = store.duplicate(&id("builtin:stable"), "a").unwrap();
    let dir = store.sprites_dir(&a).unwrap();
    std::fs::write(dir.join("s.png"), b"png").unwrap();
    std::fs::create_dir(dir.join("nested")).unwrap();
    let b = store.duplicate(&a, "b").unwrap();
    let copy = store.sprites_dir(&b).unwrap();
    assert_eq!(std::fs::read(copy.join("s.png")).unwrap(), b"png");
    assert!(!copy.join("nested").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn builtins_are_read_only() {
    let (store, root) = temp_store("ro");
    let b = id("builtin:stable");
    let layout = store.load(&b).unwrap();
    assert!(matches!(
        store.save(&b, &layout),
        Err(StoreError::ReadOnly(_))
    ));
    assert!(matches!(
        store.rename(&b, "x"),
        Err(StoreError::ReadOnly(_))
    ));
    assert!(matches!(store.delete(&b), Err(StoreError::ReadOnly(_))));
    assert!(store.sprites_dir(&b).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rename_keeps_the_id_and_delete_removes() {
    let (store, root) = temp_store("rename");
    let a = store.duplicate(&id("builtin:stable"), "old").unwrap();
    store.rename(&a, "New name").unwrap();
    let list = store.list();
    assert_eq!(list[3].id, a);
    assert_eq!(list[3].name, "New name");
    store.delete(&a).unwrap();
    assert_eq!(store.list().len(), 3);
    assert!(matches!(store.delete(&a), Err(StoreError::NotFound(_))));
    assert!(matches!(store.load(&a), Err(StoreError::NotFound(_))));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn save_then_load_round_trips() {
    let (store, root) = temp_store("save");
    let a = store.duplicate(&id("builtin:stable"), "a").unwrap();
    let mut layout = store.load(&a).unwrap();
    layout.gameplay[0].alpha = 0.5;
    layout.version = 0;
    store.save(&a, &layout).unwrap();
    layout.version = LAYOUT_VERSION;
    assert_eq!(store.load(&a).unwrap(), layout);
    assert!(matches!(
        store.save(&id("nope"), &layout),
        Err(StoreError::NotFound(_))
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn list_skips_broken_and_badly_named_folders() {
    let (store, root) = temp_store("skip");
    let a = store.duplicate(&id("builtin:stable"), "good").unwrap();
    let bad = store.dir(&id("broken"));
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join(LAYOUT_FILE), "{ not json").unwrap();
    std::fs::create_dir_all(store.root.join("Bad Id")).unwrap();
    std::fs::create_dir_all(store.root.join("empty")).unwrap();
    let list = store.list();
    assert_eq!(list.len(), 4);
    assert_eq!(list[3].id, a);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn user_layouts_sort_by_name_ignoring_case() {
    let (store, root) = temp_store("sort");
    store.duplicate(&id("builtin:stable"), "b").unwrap();
    store.duplicate(&id("builtin:stable"), "A").unwrap();
    let names: Vec<_> = store.list().into_iter().skip(3).map(|e| e.name).collect();
    assert_eq!(names, ["A", "b"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn id_parsing() {
    for bad in [
        "../x",
        "a/b",
        "A",
        "",
        &"a".repeat(65),
        "builtin:other",
        "a:b",
    ] {
        assert!(LayoutId::parse(bad).is_none(), "{bad}");
    }
    assert!(LayoutId::parse("builtin:stable").unwrap().is_builtin());
    assert!(LayoutId::parse(&"a".repeat(64)).is_some());
    assert!(!LayoutId::parse("my_hud-2").unwrap().is_builtin());
}

#[test]
fn slug_rules() {
    assert_eq!(slug("My HUD!"), "my-hud");
    assert_eq!(slug("Привет"), "layout");
    assert_eq!(slug("--a   b--"), "a-b");
    assert_eq!(slug(&"a".repeat(80)).len(), 48);
}
