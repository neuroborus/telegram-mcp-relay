//! Shared MCP handler state; application tools are introduced separately.

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use rmcp::{
    ServerHandler,
    model::{Implementation, ProtocolVersion, ServerConfig},
};

use crate::repositories::Database;

/// Agent sessions live in SQLite, independently of HTTP connections or SDK handlers.
pub struct McpHandler {
    pub database: Arc<Mutex<Database>>,
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::default().with_server_info(Implementation::new(
            "telegram-mcp-relay",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }
}
