//! Shared application entry point for Telegram MCP Relay.
//!
//! The scaffold entry point only logs; library storage and delivery operations are explicit.

#![forbid(unsafe_code)]

pub mod clients;
pub mod config;
pub mod domain;
pub mod error;
pub mod repositories;
pub mod services;

/// Run the application scaffold without starting relay services.
pub async fn run() {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "Application scaffold initialized; relay services are not implemented yet"
    );
}
