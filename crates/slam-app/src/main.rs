//! SLAM entry point.

// Release builds on Windows open no console window (ADR-0068); run from a terminal, they print
// into it (see `attach_parent_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;

use slam_app::settings::Dirs;
use slam_app::{bench, click, i18n, logging, play, replay_stats, screenshot, shell};

fn main() -> ExitCode {
    attach_parent_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if logging::starts_the_game(&args) {
        return run_game(args);
    }
    logging::init_console();
    let mut args = args.into_iter();
    // Developer commands only so far; their console output is not localised.
    match args.next().as_deref() {
        Some("--help" | "-h") => {
            print_usage();
            ExitCode::SUCCESS
        }
        Some("bench") => {
            let result = bench::Options::parse(args).and_then(|options| bench::run(&options));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}\n{}", bench::USAGE);
                    ExitCode::FAILURE
                }
            }
        }
        Some("click") => {
            let result = click::Options::parse(args).and_then(|options| click::run(&options));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}\n{}", click::USAGE);
                    ExitCode::FAILURE
                }
            }
        }
        Some("click-analyze") => {
            let result = click::analyze::Options::parse(args)
                .and_then(|options| click::analyze::run(&options));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}\n{}", click::analyze::USAGE);
                    ExitCode::FAILURE
                }
            }
        }
        Some("play") => {
            let result = play::Options::parse(args).and_then(|options| play::run(&options));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}\n{}", play::USAGE);
                    ExitCode::FAILURE
                }
            }
        }
        Some("screenshot") => {
            let result =
                screenshot::Options::parse(args).and_then(|options| screenshot::run(&options));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}\n{}", screenshot::USAGE);
                    ExitCode::FAILURE
                }
            }
        }
        Some("ui-preview") => {
            let result = screenshot::preview::Options::parse(args)
                .and_then(|options| screenshot::preview::run(&options));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}\n{}", screenshot::preview::USAGE);
                    ExitCode::FAILURE
                }
            }
        }
        Some("replay-stats") => {
            let result =
                replay_stats::Options::parse(args).and_then(|options| replay_stats::run(&options));
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}\n{}", replay_stats::USAGE);
                    ExitCode::FAILURE
                }
            }
        }
        Some("i18n") => match i18n::run(args) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("{e}\n{}", i18n::USAGE);
                ExitCode::FAILURE
            }
        },
        _ => {
            eprint_usage();
            ExitCode::from(2)
        }
    }
}

/// Runs the game with its log in `logs/slam.log` (ADR-0143). A panic of the main thread
/// or an error that ends the game is shown in the error dialog.
fn run_game(args: Vec<String>) -> ExitCode {
    let dirs = Dirs::detect();
    let log = logging::init_with_file(dirs.as_ref().map(logging::log_dir).as_deref());
    logging::install_panic_hook();
    logging::log_start(&args);
    let songs = match shell::parse_songs(args) {
        Ok(songs) => songs,
        Err(e) => {
            eprintln!("{e}\n{}", shell::USAGE);
            return ExitCode::FAILURE;
        }
    };
    let ended = std::panic::catch_unwind(move || {
        logging::debug_panic();
        shell::run(shell::load_settings(songs))
    });
    let error = match ended {
        Ok(Ok(())) => return ExitCode::SUCCESS,
        Ok(Err(e)) => e,
        Err(payload) => logging::panic_message(payload.as_ref()).to_owned(),
    };
    logging::report_fatal(&error, dirs.as_ref(), log.as_deref());
    ExitCode::FAILURE
}

fn usage() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        shell::USAGE,
        bench::USAGE,
        click::USAGE,
        click::analyze::USAGE,
        play::USAGE,
        replay_stats::USAGE,
        screenshot::USAGE,
        screenshot::preview::USAGE,
        i18n::USAGE
    )
}

/// A GUI-subsystem process has no console; attaching to the parent's (if started from a
/// terminal) brings back the logs and the developer commands' output. Attaching points
/// the standard handles at the console, so the ones the parent redirected (`> log.txt`, a
/// pipe) are put back afterwards.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };
    const STREAMS: [STD_HANDLE; 2] = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
    // SAFETY: no pointers; each call fails harmlessly (a null or invalid handle when the
    // stream is not redirected, no attach without a parent console).
    unsafe {
        let inherited = STREAMS.map(|stream| GetStdHandle(stream));
        AttachConsole(ATTACH_PARENT_PROCESS);
        for (stream, handle) in STREAMS.into_iter().zip(inherited) {
            if !handle.is_null() && handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                SetStdHandle(stream, handle);
            }
        }
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_parent_console() {}

fn print_usage() {
    println!("{}", usage());
}

fn eprint_usage() {
    eprintln!("{}", usage());
}
