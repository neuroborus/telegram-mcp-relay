//! Binary bootstrap for the shared relay application.

#![forbid(unsafe_code)]

use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    if tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init()
        .is_err()
    {
        eprintln!("Failed to initialize diagnostics");
        return ExitCode::FAILURE;
    }

    telegram_mcp_relay::run().await;
    ExitCode::SUCCESS
}
