//! SLAM entry point.

use std::process::ExitCode;

use slam_app::{bench, click};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let mut args = std::env::args().skip(1);
    // Developer commands only so far; their console output is not localised.
    match args.next().as_deref() {
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
        _ => {
            eprintln!(
                "{}\n{}\n{}",
                bench::USAGE,
                click::USAGE,
                click::analyze::USAGE
            );
            ExitCode::from(2)
        }
    }
}
