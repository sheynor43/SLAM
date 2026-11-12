//! The manifest of the local corpus of third-party skins (`docs/SKIN_CORPUS.md`): skins that
//! live outside the repository, with their own references.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Environment variable with the corpus folder.
pub const ROOT_VAR: &str = "SLAM_SKIN_CORPUS";

/// The corpus folder: `SLAM_SKIN_CORPUS`, or `slam-skins` next to the repository. `None`
/// when it does not exist.
pub fn corpus_root() -> Option<PathBuf> {
    let root = match std::env::var_os(ROOT_VAR) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../slam-skins"),
    };
    root.is_dir().then_some(root)
}

/// Which spinner the skin draws (`spinner-circle` and friends: new; `spinner-background`
/// and friends: old).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Spinner {
    Old,
    New,
}

/// One skin of the corpus.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Unique; lowercase letters, digits and `-`. The prefix of the references' names.
    pub name: String,
    /// Folder under `skins/` with the extracted skin.
    pub folder: String,
    /// The version expected from `skin.ini`: `"1.0"`, `"2.5"`, `"latest"`.
    #[serde(default)]
    pub version: Option<String>,
    pub url: String,
    /// Whether the skin has `@2x` images.
    pub hd: bool,
    pub spinner: Spinner,
    #[serde(default)]
    pub notes: Option<String>,
    /// Issues known for the skin: informational.
    #[serde(default)]
    pub known_issues: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    skin: Vec<Entry>,
}

/// Parses `manifest.toml` and checks the names. Folders are not looked at: see
/// [`Entry::dir`].
pub fn parse_manifest(text: &str) -> Result<Vec<Entry>, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    for entry in &file.skin {
        let valid = !entry.name.is_empty()
            && entry
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid {
            return Err(format!(
                "skin `{}`: the name must be lowercase letters, digits and `-`",
                entry.name
            ));
        }
        if !seen.insert(entry.name.as_str()) {
            return Err(format!("skin `{}`: the name is used twice", entry.name));
        }
        if entry.folder.is_empty()
            || entry.folder.contains(['/', '\\'])
            || entry.folder == ".."
            || entry.folder == "."
        {
            return Err(format!(
                "skin `{}`: `folder` must be one folder name under skins/",
                entry.name
            ));
        }
    }
    Ok(file.skin)
}

impl Entry {
    /// The skin's folder in the corpus at `root`.
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join("skins").join(&self.folder)
    }

    /// Whether `skin.ini`'s version is the expected one. `None` expectation always agrees.
    pub fn check_version(
        &self,
        actual: slam_skin::SkinVersion,
        is_latest: bool,
    ) -> Result<(), String> {
        let Some(expected) = &self.version else {
            return Ok(());
        };
        let agrees = if expected == "latest" {
            is_latest
        } else {
            slam_skin::SkinVersion::parse(expected)
                .ok_or_else(|| format!("`version = \"{expected}\"` is not a skin version"))?
                == actual
        };
        if agrees {
            Ok(())
        } else {
            Err(format!(
                "skin.ini has version {actual}{}, the manifest expects {expected}",
                if is_latest { " (latest)" } else { "" }
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r##"
[[skin]]
name = "first"
folder = "First Skin"
version = "2.5"
url = "https://example.com/first"
hd = true
spinner = "new"
notes = "a note"
known_issues = ["#12", "cursor"]

[[skin]]
name = "second-2"
folder = "second"
url = "https://example.com/second"
hd = false
spinner = "old"
"##;

    #[test]
    fn parses_a_valid_manifest() {
        let skins = parse_manifest(VALID).unwrap();
        assert_eq!(skins.len(), 2);
        assert_eq!(skins[0].version.as_deref(), Some("2.5"));
        assert_eq!(skins[0].spinner, Spinner::New);
        assert_eq!(skins[0].known_issues, ["#12", "cursor"]);
        assert_eq!(skins[1].version, None);
        assert_eq!(skins[1].spinner, Spinner::Old);
        assert!(skins[1].known_issues.is_empty());
        assert_eq!(
            skins[0].dir(Path::new("/c")),
            Path::new("/c/skins/First Skin")
        );
    }

    #[test]
    fn empty_manifest_has_no_skins() {
        assert!(parse_manifest("").unwrap().is_empty());
    }

    #[test]
    fn rejects_unknown_keys() {
        let text = VALID.replace("hd = true", "hd = true\nfoo = 1");
        assert!(parse_manifest(&text).is_err());
        assert!(parse_manifest("[[skins]]\n").is_err());
    }

    #[test]
    fn rejects_duplicate_names() {
        let text = VALID.replace("second-2", "first");
        let e = parse_manifest(&text).unwrap_err();
        assert!(e.contains("`first`") && e.contains("twice"), "{e}");
    }

    #[test]
    fn rejects_a_bad_spinner() {
        let text = VALID.replace("\"old\"", "\"middle\"");
        assert!(parse_manifest(&text).is_err());
    }

    #[test]
    fn rejects_bad_names_and_folders() {
        assert!(parse_manifest(&VALID.replace("second-2", "Second")).is_err());
        assert!(
            parse_manifest(&VALID.replace("folder = \"second\"", "folder = \"../x\"")).is_err()
        );
    }

    #[test]
    fn checks_the_version() {
        let skins = parse_manifest(VALID).unwrap();
        let v25 = slam_skin::SkinVersion::parse("2.5").unwrap();
        let v1 = slam_skin::SkinVersion::V1;
        assert!(skins[0].check_version(v25, false).is_ok());
        assert!(skins[0].check_version(v1, false).is_err());
        assert!(skins[1].check_version(v1, false).is_ok());
        let mut latest = skins[0].clone();
        latest.version = Some("latest".into());
        assert!(
            latest
                .check_version(slam_skin::SkinVersion::LATEST, true)
                .is_ok()
        );
        assert!(latest.check_version(v25, false).is_err());
    }
}
