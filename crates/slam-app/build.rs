//! Embeds the default skin (ADR-0068): generates the table of its files and a hash of their
//! content, so a new skin version unpacks into a new folder. On MSVC targets also gives
//! `slam.exe` its icon.

use std::fmt::Write as _;
use std::path::Path;

/// 64-bit FNV-1a.
fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn main() {
    embed_default_skin();
    embed_icon();
}

/// Only MSVC targets compile the icon: the Windows clippy check on Linux targets GNU and has no
/// resource compiler to rely on.
fn embed_icon() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icon/slam.ico");
    // Not canonicalized: on Windows that gives a `\\?\` path, which rc.exe does not take.
    assert!(icon.is_file(), "icon {} is missing", icon.display());
    println!("cargo::rerun-if-changed={}", icon.display());
    let rc = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("slam.rc");
    let icon = icon.to_str().expect("UTF-8 path").replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 ICON \"{icon}\"\n"))
        .unwrap_or_else(|e| panic!("{}: {e}", rc.display()));
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .unwrap_or_else(|e| panic!("icon resource: {e:?}"));
}

fn embed_default_skin() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default-skin");
    let dir = dir
        .canonicalize()
        .unwrap_or_else(|e| panic!("default skin {}: {e}", dir.display()));
    println!("cargo::rerun-if-changed={}", dir.display());

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("default skin {}: {e}", dir.display()))
        .map(|entry| entry.expect("default skin entry").path())
        .collect();
    files.sort();

    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut table = String::from("pub(crate) const FILES: &[(&str, &[u8])] = &[\n");
    for path in &files {
        assert!(
            path.is_file(),
            "default skin must be flat, found {}",
            path.display()
        );
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_else(|| panic!("non UTF-8 file name {}", path.display()));
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        // The name's terminator keeps "ab" + "c" apart from "a" + "bc".
        fnv1a(&mut hash, name.as_bytes());
        fnv1a(&mut hash, &[0]);
        fnv1a(&mut hash, &bytes);
        let path = path.to_str().expect("UTF-8 path");
        writeln!(table, "    ({name:?}, include_bytes!({path:?})),").unwrap();
    }
    table.push_str("];\n");
    writeln!(table, "pub(crate) const HASH: u64 = {hash:#018x};").unwrap();

    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("default_skin.rs");
    std::fs::write(&out, table).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}
