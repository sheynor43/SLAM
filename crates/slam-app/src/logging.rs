//! The log in the console and in `logs/slam.log`, the panic hook and the fatal error
//! dialog (ADR-0143).

use std::any::Any;
use std::fmt::Display;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use slam_ui::i18n::{FluentArgs, Localizer};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, filter::LevelFilter, fmt};

use crate::settings::Dirs;

/// The folder of the log files in the data folder.
pub const LOG_DIR: &str = "logs";
/// The log of the current run; the earlier ones are `slam.1.log` (the last) and on.
pub const LOG_FILE: &str = "slam.log";
/// How many runs' logs are kept, the current one included.
pub const KEPT_LOGS: usize = 5;
/// Set (to anything), the game panics on its main thread at start: a check of the log and
/// the error dialog on any machine.
pub const DEBUG_PANIC_ENV: &str = "SLAM_DEBUG_PANIC";

/// The log folder of `dirs`.
pub fn log_dir(dirs: &Dirs) -> PathBuf {
    dirs.data.join(LOG_DIR)
}

/// The log of the run `age` runs ago in `dir`: 0 is `slam.log`, 1 is `slam.1.log`.
pub fn log_path(dir: &Path, age: usize) -> PathBuf {
    if age == 0 {
        dir.join(LOG_FILE)
    } else {
        dir.join(format!("slam.{age}.log"))
    }
}

/// Moves each log of `dir` one run older and drops the ones beyond `keep` runs, so that
/// `slam.log` is free. Logs that are missing are skipped.
pub fn rotate(dir: &Path, keep: usize) -> io::Result<()> {
    let oldest = keep.saturating_sub(1);
    match std::fs::remove_file(log_path(dir, oldest)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    for age in (0..oldest).rev() {
        match std::fs::rename(log_path(dir, age), log_path(dir, age + 1)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Rotates the logs in `dir` and creates a new `slam.log` there.
pub fn open_log(dir: &Path) -> io::Result<(File, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    rotate(dir, KEPT_LOGS)?;
    let path = log_path(dir, 0);
    // Appending: a write split by the OS still lands at the end, after other threads' lines.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    Ok((file, path))
}

/// `info` unless `RUST_LOG` says otherwise: the startup lines (render backend, audio mode,
/// library scan) are what a bug report needs. Panics and fatal errors
/// ([`FATAL_TARGET`]) are let through whatever `RUST_LOG` says.
fn env_filter() -> EnvFilter {
    EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy()
        // A constant, parsed in a test.
        .add_directive(PANIC_DIRECTIVE.parse().expect("a valid directive"))
}

/// The target of the panic and fatal error events: their own, so that letting them through
/// does not change the level of anything else.
pub const FATAL_TARGET: &str = "slam_fatal";
/// Lets the panics and the fatal errors through.
const PANIC_DIRECTIVE: &str = "slam_fatal=error";

/// No arguments or only flags start the game; a first word is a developer command, which
/// logs to the console only.
pub fn starts_the_game(args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        None => true,
        Some("--help" | "-h") => false,
        Some(arg) => arg.starts_with("--"),
    }
}

/// The layer that writes the log into `file`: no colour codes, each event written at once.
pub fn file_layer<S>(file: File) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fmt::layer()
        .with_ansi(false)
        // `&File` writes without a lock: nothing to poison or to wait for, and each event
        // is a single write.
        .with_writer(Arc::new(file))
        .boxed()
}

/// Logs to the console only: the developer commands.
pub fn init_console() {
    tracing_subscriber::fmt()
        .with_env_filter(env_filter())
        .init();
}

/// Logs to the console and, when `dir` is given and the log opens there, to `slam.log` in
/// it. Every event goes to the file at once, unbuffered, so the log is whole up to a crash.
/// Returns the log's path.
pub fn init_with_file(dir: Option<&Path>) -> Option<PathBuf> {
    let opened = dir.map(open_log);
    let (file_layer, path, error) = match opened {
        Some(Ok((file, path))) => (Some(file_layer(file)), Some(path), None),
        Some(Err(e)) => (None, None, Some(e)),
        None => (None, None, None),
    };
    tracing_subscriber::registry()
        .with(fmt::layer().with_filter(env_filter()))
        .with(file_layer.map(|layer| layer.with_filter(env_filter())))
        .init();
    if let Some(e) = error {
        tracing::warn!("log: cannot write the log file: {e}");
    }
    path
}

/// The first lines of a log: what a bug report needs to know about the build and the OS.
pub fn log_start(args: &[String]) {
    tracing::info!(
        "SLAM {} on {} {}, arguments {args:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}

/// The message a panic was started with, if it is text.
pub fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(text) = payload.downcast_ref::<&str>() {
        text
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text
    } else {
        "a panic without a message"
    }
}

/// The log entry of a panic.
pub fn panic_report(
    thread: &str,
    message: &str,
    location: Option<&dyn Display>,
    backtrace: &dyn Display,
) -> String {
    match location {
        Some(location) => {
            format!("thread '{thread}' panicked at {location}:\n{message}\nbacktrace:\n{backtrace}")
        }
        None => format!("thread '{thread}' panicked:\n{message}\nbacktrace:\n{backtrace}"),
    }
}

/// Logs every panic, of any thread, with a backtrace, instead of printing it to the
/// console: the log's console output prints it there too.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let location = info.location().map(|l| l as &dyn Display);
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(
            target: FATAL_TARGET,
            "{}",
            panic_report(
                thread.name().unwrap_or("<unnamed>"),
                panic_message(info.payload()),
                location,
                &backtrace,
            )
        );
    }));
}

/// Panics when [`DEBUG_PANIC_ENV`] is set.
pub fn debug_panic() {
    if std::env::var_os(DEBUG_PANIC_ENV).is_some() {
        panic!("{DEBUG_PANIC_ENV} is set: a test panic of the main thread");
    }
}

/// The title and the text of the fatal error dialog in `language` (a built-in tag; `None`
/// or an unknown one is English).
pub fn fatal_text(language: Option<&str>, error: &str, log: Option<&Path>) -> (String, String) {
    let Ok(localizer) = Localizer::builtin() else {
        // The built-in translations are embedded and parsed in tests; this is a fallback.
        return (
            "SLAM".to_owned(),
            format!("SLAM stopped because of an error:\n{error}"),
        );
    };
    if let Some(language) = language {
        let _ = localizer.set_language(language);
    }
    let mut args = FluentArgs::new();
    args.set("error", error);
    let key = match log {
        Some(log) => {
            args.set("log", log.display().to_string());
            "fatal-error-message"
        }
        None => "fatal-error-message-no-log",
    };
    let title = localizer.format("fatal-error-title", None).into_owned();
    let message = localizer.format(key, Some(&args)).into_owned();
    (title, message)
}

/// The interface language saved in the settings of `dirs`, read without creating them.
pub fn saved_language(dirs: Option<&Dirs>) -> Option<String> {
    let file = dirs?.settings_file();
    if !file.exists() {
        return None;
    }
    crate::settings::load(&file).settings.general.language
}

/// Logs the fatal `error` and shows it in the error dialog with the log's path.
pub fn report_fatal(error: &str, dirs: Option<&Dirs>, log: Option<&Path>) {
    tracing::error!(target: FATAL_TARGET, "fatal: {error}");
    let (title, message) = fatal_text(saved_language(dirs).as_deref(), error, log);
    if let Err(e) = slam_input::show_error_box(&title, &message) {
        tracing::warn!("log: cannot show the error dialog: {e}");
    }
}

#[cfg(test)]
mod tests;
