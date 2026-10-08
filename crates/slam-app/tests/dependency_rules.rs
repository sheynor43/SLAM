//! Enforces crate dependency rules from ARCHITECTURE §3.

use std::collections::{HashMap, HashSet};

use cargo_metadata::{MetadataCommand, PackageId};

/// Crates that must stay independent of the engine, window, GPU, audio and OS layers.
const PURE_CRATES: &[&str] = &["slam-formats", "slam-osu"];

const FORBIDDEN: &[&str] = &[
    "slam-engine",
    "slam-render",
    "slam-audio",
    "slam-input",
    "slam-ui",
    "slam-app",
];

/// Returns names of all packages transitively reachable from `root` (normal and build deps).
fn transitive_deps(
    root: &PackageId,
    graph: &HashMap<&PackageId, &cargo_metadata::Node>,
    names: &HashMap<&PackageId, &str>,
) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    let mut found = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for dep in &graph[id].deps {
            let non_dev = dep
                .dep_kinds
                .iter()
                .any(|k| k.kind != cargo_metadata::DependencyKind::Development);
            if non_dev {
                found.insert(names[&dep.pkg].to_owned());
                stack.push(&dep.pkg);
            }
        }
    }
    found
}

#[test]
fn pure_crates_do_not_depend_on_engine_layers() {
    let metadata = MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
        .exec()
        .expect("cargo metadata failed");
    let resolve = metadata.resolve.as_ref().expect("no resolve graph");
    let graph: HashMap<_, _> = resolve.nodes.iter().map(|n| (&n.id, n)).collect();
    let names: HashMap<_, _> = metadata
        .packages
        .iter()
        .map(|p| (&p.id, p.name.as_str()))
        .collect();

    for &krate in PURE_CRATES {
        let pkg = metadata
            .workspace_packages()
            .into_iter()
            .find(|p| p.name.as_str() == krate)
            .unwrap_or_else(|| panic!("{krate} not found in workspace"));
        let deps = transitive_deps(&pkg.id, &graph, &names);
        let violations: Vec<_> = FORBIDDEN.iter().filter(|f| deps.contains(**f)).collect();
        assert!(
            violations.is_empty(),
            "{krate} must not depend on {violations:?}"
        );
    }
}
