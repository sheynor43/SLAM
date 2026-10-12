//! `i18n check`: consistency of the built-in locales against English.

use slam_ui::i18n::{BUILTIN_LANGUAGES, check};

pub const USAGE: &str = "usage: slam i18n check";

/// Runs the subcommand; `Ok(true)` means the locales are clean.
pub fn run(mut args: impl Iterator<Item = String>) -> Result<bool, String> {
    match args.next().as_deref() {
        Some("check") if args.next().is_none() => {
            let report =
                check(&BUILTIN_LANGUAGES[0], &BUILTIN_LANGUAGES[1..]).map_err(|e| e.to_string())?;
            print!("{report}");
            Ok(report.is_clean())
        }
        _ => Err("expected `check`".to_owned()),
    }
}
