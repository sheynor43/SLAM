//! Every shader in the workspace translates (no GPU needed).

use std::path::Path;

#[test]
fn all_project_shaders_translate() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut count = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap() {
        let dir = entry.unwrap().path().join("shaders");
        if !dir.is_dir() {
            continue;
        }
        match slam_shader_build::translate_dir(&dir) {
            Ok(shaders) => count += shaders.len(),
            Err(errors) => failures.extend(errors.iter().map(ToString::to_string)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert!(count > 0, "no shaders found under {}", crates.display());
}
