//! Binary bootstrap for the shared relay application.

#![forbid(unsafe_code)]

use std::process::ExitCode;
use tracing_subscriber::{filter::Targets, prelude::*};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // SDK diagnostics can include raw request metadata; emit our safe boundary events.
    if tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr)
                .with_filter(
                    Targets::new()
                        .with_default(tracing::Level::INFO)
                        .with_target("rmcp", tracing_subscriber::filter::LevelFilter::OFF),
                ),
        )
        .try_init()
        .is_err()
    {
        eprintln!("Failed to initialize diagnostics");
        return ExitCode::FAILURE;
    }

    let mut arguments = std::env::args_os().skip(1);
    let Some(path) = arguments.next().filter(|_| arguments.next().is_none()) else {
        eprintln!("Usage: telegram-mcp-relay <config.toml>");
        return ExitCode::FAILURE;
    };
    match telegram_mcp_relay::run(std::path::Path::new(&path)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(code = error.code(), "Relay stopped");
            ExitCode::FAILURE
        }
    }
}
