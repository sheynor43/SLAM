use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{embedded, is_version_name, unpack};

fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "slam-default-skin-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

const FILES: &[(&str, &[u8])] = &[("hitcircle.png", b"circle"), ("count.wav", b"count")];

#[test]
fn unpacks_every_file_into_the_hash_folder() {
    let root = temp_dir("unpack");
    let dir = unpack(&root, FILES, 0xab).unwrap();
    assert_eq!(dir, root.join("00000000000000ab"));
    for (name, bytes) in FILES {
        assert_eq!(std::fs::read(dir.join(name)).unwrap(), *bytes);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unpacked_version_is_not_rewritten() {
    let root = temp_dir("again");
    let dir = unpack(&root, FILES, 1).unwrap();
    std::fs::write(dir.join("hitcircle.png"), b"edited").unwrap();
    assert_eq!(unpack(&root, FILES, 1).unwrap(), dir);
    assert_eq!(std::fs::read(dir.join("hitcircle.png")).unwrap(), b"edited");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn other_versions_and_leftovers_are_removed_but_foreign_entries_kept() {
    let root = temp_dir("stale");
    let old = unpack(&root, FILES, 1).unwrap();
    let leftover = root.join("0000000000000002.tmp-77");
    std::fs::create_dir_all(leftover.join("x")).unwrap();
    let foreign = root.join("my-skin");
    std::fs::create_dir_all(&foreign).unwrap();
    let new = unpack(&root, FILES, 2).unwrap();
    assert!(new.join("count.wav").is_file());
    assert!(!old.exists());
    assert!(!leftover.exists());
    assert!(foreign.is_dir());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn version_names() {
    assert!(is_version_name("0123456789abcdef"));
    assert!(is_version_name("0123456789abcdef.tmp-42"));
    assert!(!is_version_name("0123456789abcde"));
    assert!(!is_version_name("0123456789abcdef.tmp-"));
    assert!(!is_version_name("0123456789abcdef.bak"));
    assert!(!is_version_name("skins"));
    assert!(!is_version_name("ёёёёёёёё"));
}

#[test]
fn embedded_skin_matches_the_source_tree_and_loads() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default-skin");
    let on_disk = std::fs::read_dir(&source).unwrap().count();
    assert_eq!(embedded::FILES.len(), on_disk);
    assert!(
        embedded::FILES
            .iter()
            .any(|(name, _)| *name == "hitcircle@2x.png")
    );
    let root = temp_dir("embedded");
    let dir = unpack(&root, embedded::FILES, embedded::HASH).unwrap();
    assert!(slam_skin::Skin::load_default(&dir).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}
