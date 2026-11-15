use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use tracing_subscriber::layer::SubscriberExt;

use super::*;

fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "slam-logging-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read(dir: &Path, age: usize) -> Option<String> {
    std::fs::read_to_string(log_path(dir, age)).ok()
}

#[test]
fn logs_are_named_by_age() {
    let dir = Path::new("logs");
    assert_eq!(log_path(dir, 0), dir.join("slam.log"));
    assert_eq!(log_path(dir, 3), dir.join("slam.3.log"));
}

#[test]
fn rotation_keeps_the_last_runs() {
    let dir = temp_dir("rotate");
    for run in 0..7 {
        let (file, path) = open_log(&dir).unwrap();
        drop(file);
        std::fs::write(path, format!("run {run}")).unwrap();
    }
    assert_eq!(read(&dir, 0).as_deref(), Some("run 6"));
    assert_eq!(read(&dir, 1).as_deref(), Some("run 5"));
    assert_eq!(read(&dir, KEPT_LOGS - 1).as_deref(), Some("run 2"));
    assert_eq!(read(&dir, KEPT_LOGS), None);
    let files = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(files, KEPT_LOGS);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn rotation_skips_missing_logs() {
    let dir = temp_dir("gaps");
    std::fs::write(log_path(&dir, 0), "current").unwrap();
    std::fs::write(log_path(&dir, 2), "older").unwrap();
    rotate(&dir, KEPT_LOGS).unwrap();
    assert_eq!(read(&dir, 0), None);
    assert_eq!(read(&dir, 1).as_deref(), Some("current"));
    assert_eq!(read(&dir, 2), None);
    assert_eq!(read(&dir, 3).as_deref(), Some("older"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_log_creates_the_folder() {
    let root = temp_dir("create");
    let dir = root.join("logs");
    let (_, path) = open_log(&dir).unwrap();
    assert_eq!(path, dir.join(LOG_FILE));
    assert!(path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn events_reach_the_file_at_once_without_colour() {
    let dir = temp_dir("events");
    let (file, path) = open_log(&dir).unwrap();
    let subscriber = tracing_subscriber::registry().with(file_layer(file));
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!("first line");
        // Nothing is buffered: the line is in the file while the subscriber still lives.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("WARN"), "{text}");
        assert!(text.contains("first line"), "{text}");
        assert!(!text.contains('\u{1b}'), "colour codes in {text:?}");
    });
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_the_game_writes_a_log_file() {
    let args = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    assert!(starts_the_game(&args(&[])));
    assert!(starts_the_game(&args(&["--songs", "/x"])));
    assert!(!starts_the_game(&args(&["--help"])));
    assert!(!starts_the_game(&args(&["-h"])));
    assert!(!starts_the_game(&args(&["screenshot", "--out", "a.png"])));
    assert!(!starts_the_game(&args(&["bench"])));
}

#[test]
fn panic_messages_are_read_from_text_payloads() {
    let text: Box<dyn Any + Send> = Box::new("static text");
    assert_eq!(panic_message(text.as_ref()), "static text");
    let owned: Box<dyn Any + Send> = Box::new(String::from("formatted 7"));
    assert_eq!(panic_message(owned.as_ref()), "formatted 7");
    let other: Box<dyn Any + Send> = Box::new(7_u32);
    assert_eq!(panic_message(other.as_ref()), "a panic without a message");
}

#[test]
fn the_panic_report_names_the_thread_place_and_backtrace() {
    let location = "src/play.rs:10:5";
    let report = panic_report("update", "boom", Some(&location), &"  0: frame");
    assert_eq!(
        report,
        "thread 'update' panicked at src/play.rs:10:5:\nboom\nbacktrace:\n  0: frame"
    );
    let report = panic_report("main", "boom", None, &"");
    assert!(report.starts_with("thread 'main' panicked:\nboom"));
}

#[test]
fn the_fatal_dialog_is_localised_and_names_the_log() {
    let log = Path::new("/data/logs/slam.log");
    let (title, message) = fatal_text(None, "no adapter", Some(log));
    assert_eq!(title, "SLAM has stopped");
    assert!(message.contains("no adapter"), "{message}");
    assert!(message.contains("/data/logs/slam.log"), "{message}");
    let (title, message) = fatal_text(Some("ru"), "no adapter", Some(log));
    assert_eq!(title, "SLAM остановился");
    assert!(message.contains("Лог сохранён"), "{message}");
    let (_, message) = fatal_text(Some("xx"), "no adapter", None);
    assert!(message.contains("could not be saved"), "{message}");
}

#[test]
fn the_saved_language_is_read_without_creating_settings() {
    let dir = temp_dir("language");
    let dirs = Dirs {
        config: dir.clone(),
        data: dir.clone(),
        portable: true,
    };
    assert_eq!(saved_language(Some(&dirs)), None);
    assert!(!dirs.settings_file().exists());
    std::fs::write(dirs.settings_file(), "[general]\nlanguage = \"ru\"\n").unwrap();
    assert_eq!(saved_language(Some(&dirs)).as_deref(), Some("ru"));
    assert_eq!(saved_language(None), None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn panics_pass_any_log_filter() {
    let directive: tracing_subscriber::filter::Directive = PANIC_DIRECTIVE.parse().unwrap();
    let filter = EnvFilter::new("off").add_directive(directive);
    let subscriber = tracing_subscriber::registry().with(filter);
    tracing::subscriber::with_default(subscriber, || {
        assert!(tracing::event_enabled!(target: FATAL_TARGET, tracing::Level::ERROR));
        assert!(!tracing::event_enabled!(target: "slam_render", tracing::Level::ERROR));
    });
}

#[test]
fn letting_panics_through_keeps_the_other_levels() {
    let directive: tracing_subscriber::filter::Directive = PANIC_DIRECTIVE.parse().unwrap();
    let filter = EnvFilter::new("info").add_directive(directive);
    let subscriber = tracing_subscriber::registry().with(filter);
    tracing::subscriber::with_default(subscriber, || {
        // The start line of the log is this module's info.
        assert!(tracing::event_enabled!(target: "slam_app::logging", tracing::Level::INFO));
        assert!(tracing::event_enabled!(target: FATAL_TARGET, tracing::Level::ERROR));
        assert!(!tracing::event_enabled!(target: "slam_render", tracing::Level::DEBUG));
    });
}

#[test]
fn a_new_log_starts_empty() {
    let dir = temp_dir("fresh");
    std::fs::write(log_path(&dir, 0), "old run").unwrap();
    let (_, path) = open_log(&dir).unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), "");
    assert_eq!(read(&dir, 1).as_deref(), Some("old run"));
    std::fs::remove_dir_all(dir).unwrap();
}
