//! SLAM entry point.

// Release builds on Windows open no console window (ADR-0068); run from a terminal, they print
// into it (see `attach_parent_console`).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;

use slam_app::{bench, click, i18n, play, shell};

fn main() -> ExitCode {
    attach_parent_console();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let mut args = std::env::args().skip(1);
    // Developer commands only so far; their console output is not localised.
    match args.next().as_deref() {
        None => run_shell(Vec::new()),
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
        Some("i18n") => match i18n::run(args) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("{e}\n{}", i18n::USAGE);
                ExitCode::FAILURE
            }
        },
        Some(flag) if flag.starts_with("--") => {
            run_shell(std::iter::once(flag.to_owned()).chain(args).collect())
        }
        _ => {
            eprint_usage();
            ExitCode::from(2)
        }
    }
}

fn run_shell(args: Vec<String>) -> ExitCode {
    match shell::parse_songs(args)
        .map(shell::load_settings)
        .and_then(shell::run)
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}\n{}", shell::USAGE);
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        shell::USAGE,
        bench::USAGE,
        click::USAGE,
        click::analyze::USAGE,
        play::USAGE,
        i18n::USAGE
    )
}

/// A GUI-subsystem process has no console; attaching to the parent's (if started from a
/// terminal) brings back the logs and the developer commands' output.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no pointers; fails harmlessly when there is no parent console.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
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
