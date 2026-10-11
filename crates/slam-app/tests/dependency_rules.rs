//! Enforces crate dependency rules from ARCHITECTURE §3.

use std::collections::{HashMap, HashSet};

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, PackageId};

/// Crates that must stay independent of the engine, window, GPU, audio and OS layers.
const PURE_CRATES: &[&str] = &["slam-formats", "slam-osu"];

const FORBIDDEN: &[&str] = &[
    "slam-engine",
    "slam-render",
    "slam-audio",
    "slam-input",
    "slam-ui",
    "slam-play",
    "slam-app",
];

/// Workspace dependencies each crate may declare directly (ARCHITECTURE §3, ADR-0032).
/// `None` means any workspace crate. Applies to normal, build and dev dependencies.
/// Keep in sync with the crate table in docs/ARCHITECTURE.md §3.
const ALLOWED: &[(&str, Option<&[&str]>)] = &[
    ("slam-formats", Some(&[])),
    ("slam-osu", Some(&["slam-formats"])),
    ("slam-input", Some(&[])),
    ("slam-render", Some(&[])),
    ("slam-text", Some(&["slam-render"])),
    ("slam-audio", Some(&[])),
    (
        "slam-engine",
        Some(&["slam-input", "slam-render", "slam-audio"]),
    ),
    ("slam-skin", Some(&["slam-formats"])),
    ("slam-library", Some(&["slam-formats", "slam-osu"])),
    ("slam-plugins", Some(&["slam-osu"])),
    ("slam-ui", Some(&["slam-render", "slam-skin", "slam-text"])),
    (
        "slam-play",
        Some(&[
            "slam-osu",
            "slam-ui",
            "slam-render",
            "slam-skin",
            "slam-text",
            "slam-input",
            "slam-audio",
        ]),
    ),
    ("slam-ipc", Some(&["slam-osu"])),
    ("slam-app", None),
    ("slam-shader-build", Some(&[])),
    ("slam-testkit", Some(&[])),
];

/// Tooling crates allowed anywhere, but only as the given dependency kind.
const TOOLING: &[(&str, DependencyKind)] = &[
    ("slam-shader-build", DependencyKind::Build),
    ("slam-testkit", DependencyKind::Development),
];

fn metadata() -> Metadata {
    MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
        .exec()
        .expect("cargo metadata failed")
}

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
    let metadata = metadata();
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

#[test]
fn every_workspace_crate_has_dependency_rules() {
    let metadata = metadata();
    let mut members: Vec<_> = metadata
        .workspace_packages()
        .into_iter()
        .map(|p| p.name.as_str().to_owned())
        .collect();
    members.sort();
    let mut listed: Vec<_> = ALLOWED.iter().map(|(name, _)| name.to_string()).collect();
    listed.sort();
    assert_eq!(
        members, listed,
        "ALLOWED must list exactly the workspace crates"
    );
}

#[test]
fn direct_dependencies_follow_the_crate_table() {
    let metadata = metadata();
    let members: HashSet<_> = metadata
        .workspace_packages()
        .into_iter()
        .map(|p| p.name.as_str())
        .collect();

    let mut violations = Vec::new();
    for pkg in metadata.workspace_packages() {
        let krate = pkg.name.as_str();
        let Some(&(_, allowed)) = ALLOWED.iter().find(|(name, _)| *name == krate) else {
            continue; // reported by `every_workspace_crate_has_dependency_rules`
        };
        for dep in &pkg.dependencies {
            let name = dep.name.as_str();
            if !members.contains(name) {
                continue;
            }
            let ok = match TOOLING.iter().find(|(tool, _)| *tool == name) {
                Some(&(_, kind)) => dep.kind == kind,
                None => allowed.is_none_or(|allowed| allowed.contains(&name)),
            };
            if !ok {
                violations.push(format!("{krate} -> {name} ({:?})", dep.kind));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "dependencies not allowed by ARCHITECTURE §3: {violations:?}"
    );
}
