//! SLAM entry point.

use std::process::ExitCode;

use slam_app::{bench, click, i18n, play, shell};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let mut args = std::env::args().skip(1);
    // Developer commands only so far; their console output is not localised.
    match args.next().as_deref() {
        None => match shell::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
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
        _ => {
            eprint_usage();
            ExitCode::from(2)
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

fn print_usage() {
    println!("{}", usage());
}

fn eprint_usage() {
    eprintln!("{}", usage());
}
