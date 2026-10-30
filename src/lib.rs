//! Shared application entry point for Telegram MCP Relay.
//!
//! This initial scaffold does not yet open a listener, contact Telegram or create storage.

#![forbid(unsafe_code)]

pub mod clients;
pub mod config;
pub mod domain;
pub mod error;
pub mod repositories;

/// Run the application scaffold without starting relay services.
pub async fn run() {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        "Application scaffold initialized; relay services are not implemented yet"
    );
}
