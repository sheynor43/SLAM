//! Comparing frames with the references on disk, shared by the screenshot tests: the update
//! mode, saving the frame and the map of differences of a failure, collecting the failures.

use std::path::{Path, PathBuf};

use super::Frame;
use super::compare::{Mismatch, Tolerance, compare};

/// Whether `SLAM_UPDATE_SCREENSHOTS=1` asks to write the references instead of comparing.
pub fn update_requested() -> bool {
    std::env::var_os("SLAM_UPDATE_SCREENSHOTS").is_some_and(|v| v == "1")
}

/// How one frame fared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Equal to the reference within the tolerance.
    Ok,
    /// The reference was written.
    Updated,
    /// Different from the reference, or the reference is missing or unreadable.
    Failed(String),
}

/// Checks frames against the references in one folder. A failure saves the frame as
/// `<name>.actual.png` and the differences as `<name>.diff.png` in `failures_dir`.
#[derive(Debug)]
pub struct Checker {
    references: PathBuf,
    failures_dir: PathBuf,
    update: bool,
    failures: Vec<String>,
}

impl Checker {
    /// `failures_dir` is emptied first; it is created when a failure needs it.
    pub fn new(references: PathBuf, failures_dir: PathBuf, update: bool) -> Self {
        let _ = std::fs::remove_dir_all(&failures_dir);
        Self {
            references,
            failures_dir,
            update,
            failures: Vec::new(),
        }
    }

    pub fn failures_dir(&self) -> &Path {
        &self.failures_dir
    }

    /// The failures so far, one line each, `<name>: <what>`.
    pub fn failures(&self) -> &[String] {
        &self.failures
    }

    /// Compares `frame` with the reference `<name>.png`, or writes it in the update mode.
    /// A failure is recorded in [`Checker::failures`] too.
    pub fn check(&mut self, name: &str, frame: &Frame) -> Outcome {
        let reference = self.references.join(format!("{name}.png"));
        if self.update {
            return match std::fs::create_dir_all(&self.references)
                .map_err(|e| format!("{}: {e}", self.references.display()))
                .and_then(|()| frame.write_png(&reference))
            {
                Ok(()) => Outcome::Updated,
                Err(e) => self.fail(name, e, frame, None),
            };
        }
        let expected = match Frame::read_png(&reference) {
            Ok(expected) => expected,
            Err(e) => return self.fail(name, e, frame, None),
        };
        match compare(&expected, frame, Tolerance::default()) {
            Ok(_) => Outcome::Ok,
            Err(mismatch) => {
                let diff = match &mismatch {
                    Mismatch::Pixels(d) => Some(&d.image),
                    Mismatch::Size { .. } => None,
                };
                self.fail(name, mismatch.to_string(), frame, diff)
            }
        }
    }

    /// Records a failure that is not about a frame, such as a skin that did not render.
    pub fn record(&mut self, name: &str, message: &str) {
        self.failures.push(format!("{name}: {message}"));
    }

    fn fail(
        &mut self,
        name: &str,
        message: String,
        frame: &Frame,
        diff: Option<&Frame>,
    ) -> Outcome {
        self.record(name, &message);
        save(&self.failures_dir, name, frame, diff);
        Outcome::Failed(message)
    }

    /// Panics with the failures, if any.
    pub fn assert_no_failures(&self) {
        assert!(
            self.failures.is_empty(),
            "screenshots differ from the references (frames and differences in {}):\n{}",
            self.failures_dir.display(),
            self.failures.join("\n")
        );
    }
}

fn save(dir: &Path, name: &str, actual: &Frame, diff: Option<&Frame>) {
    std::fs::create_dir_all(dir).unwrap();
    actual
        .write_png(&dir.join(format!("{name}.actual.png")))
        .unwrap();
    if let Some(diff) = diff {
        diff.write_png(&dir.join(format!("{name}.diff.png")))
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(value: u8) -> Frame {
        Frame {
            width: 4,
            height: 4,
            pixels: vec![value; 4 * 4 * 4],
        }
    }

    fn dirs(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("slam-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (root.join("reference"), root.join("failures"))
    }

    #[test]
    fn update_then_compare_then_mismatch() {
        let (references, failures) = dirs("flow");
        let mut update = Checker::new(references.clone(), failures.clone(), true);
        assert_eq!(update.check("a", &frame(10)), Outcome::Updated);

        let mut check = Checker::new(references.clone(), failures.clone(), false);
        assert_eq!(check.check("a", &frame(10)), Outcome::Ok);
        assert!(matches!(check.check("a", &frame(200)), Outcome::Failed(_)));
        assert!(matches!(
            check.check("missing", &frame(0)),
            Outcome::Failed(_)
        ));
        assert_eq!(check.failures().len(), 2);
        assert!(failures.join("a.actual.png").is_file());
        assert!(failures.join("a.diff.png").is_file());
        std::fs::remove_dir_all(references.parent().unwrap()).unwrap();
    }
}
