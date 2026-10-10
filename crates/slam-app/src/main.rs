//! SLAM entry point.

use std::process::ExitCode;

use slam_app::bench;

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
        _ => {
            eprintln!("{}", bench::USAGE);
            ExitCode::from(2)
        }
    }
}
