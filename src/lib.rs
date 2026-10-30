//! Shared application entry point for Telegram MCP Relay.
//!
//! Production startup and cancellable composition share the same durable services.

#![forbid(unsafe_code)]

pub mod app;
pub mod clients;
pub mod config;
pub mod domain;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod repositories;
pub mod routes;
pub mod services;

pub use app::{run, run_until};
