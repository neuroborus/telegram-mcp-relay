//! Thin typed MCP tools over shared durable registration and outbound services.

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig,
        Tool,
    },
    service::RequestContext,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    clients::telegram::TelegramClient,
    config::Config,
    dto::{RegisterRequest, RegisterResult, SendFileRequest, SendRequest, SendResult},
    error::{AppError, TelegramError},
    repositories::Database,
    services::{files::OutboundFiles, outbound::OutboundService, unix_millis},
};

/// Agent sessions live in SQLite, independently of HTTP connections or SDK handlers.
pub struct McpHandler {
    database: Arc<Mutex<Database>>,
    outbound: OutboundService,
    files: OutboundFiles,
}

impl McpHandler {
    /// Construct once; every per-request SDK factory must share this handler.
    pub fn new(
        database: Arc<Mutex<Database>>,
        telegram: TelegramClient,
        config: &Config,
    ) -> Result<Self, AppError> {
        Ok(Self {
            outbound: OutboundService::new(database.clone(), telegram, config)?,
            files: OutboundFiles::new(&config.files)?,
            database,
        })
    }

    fn register(&self, request: RegisterRequest) -> Result<RegisterResult, AppError> {
        let session = self
            .database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .register_session(
                request.external_session_id.as_deref(),
                request.session_key.as_deref(),
                &request.title,
                unix_millis()?,
            )?;
        Ok(RegisterResult {
            inbox_uri: session.inbox_uri(),
            title: session.title.as_str().to_owned(),
            session_key: session.session_key,
        })
    }

    async fn send(&self, request: SendRequest) -> Result<SendResult, AppError> {
        let sent = self
            .outbound
            .send_text(&request.session_key, &request.text)
            .await?;
        Ok(SendResult {
            telegram_chat_id: sent.chat.id,
            outbound_message_id: sent.message_id,
        })
    }

    async fn send_file(&self, request: SendFileRequest) -> Result<SendResult, AppError> {
        let file = self.files.open(&request.path)?;
        let sent = self
            .outbound
            .send_file(&request.session_key, file, &request.caption)
            .await?;
        Ok(SendResult {
            telegram_chat_id: sent.chat.id,
            outbound_message_id: sent.message_id,
        })
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("telegram-mcp-relay", env!("CARGO_PKG_VERSION")),
        )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|request| request.cursor.is_some()) {
            return Err(ErrorData::invalid_params("Invalid tools cursor", None));
        }
        Ok(ListToolsResult::with_all_items(tools())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = match request.name.as_ref() {
            "telegram_register" => tool_result(self.register(arguments(&request)?)),
            "telegram_send" => tool_result(self.send(arguments(&request)?).await),
            "telegram_send_file" => tool_result(self.send_file(arguments(&request)?).await),
            _ => return Err(ErrorData::invalid_params("Unknown tool", None)),
        };
        result.map(Into::into)
    }
}

fn tools() -> Vec<Tool> {
    vec![
        Tool::new("telegram_register", "Register or resume an agent conversation; preserve the returned session key.", serde_json::Map::new())
            .with_input_schema::<RegisterRequest>().with_output_schema::<RegisterResult>(),
        Tool::new("telegram_send", "Send concise plain text with a session header and durable reply route. Never automatically retry an uncertain send.", serde_json::Map::new())
            .with_input_schema::<SendRequest>().with_output_schema::<SendResult>(),
        Tool::new("telegram_send_file", "Stream an existing approved local file with a short session caption and durable reply route. Never automatically retry an uncertain send.", serde_json::Map::new())
            .with_input_schema::<SendFileRequest>().with_output_schema::<SendResult>(),
    ]
}

fn arguments<T: DeserializeOwned>(request: &CallToolRequestParams) -> Result<T, ErrorData> {
    // Serde diagnostics can quote supplied field names/values; never return them.
    serde_json::from_value(Value::Object(request.arguments.clone().unwrap_or_default()))
        .map_err(|_| ErrorData::invalid_params("Invalid tool arguments", None))
}

fn tool_result<T: Serialize>(result: Result<T, AppError>) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(result) => serde_json::to_value(result)
            .map(CallToolResult::structured)
            .map_err(|_| ErrorData::internal_error("Tool result unavailable", None)),
        Err(error) => {
            let details = match &error {
                AppError::TextTooLong(details) => json!(details),
                AppError::FileTooLarge {
                    limit_bytes,
                    actual_bytes,
                } => json!({"limit_bytes":limit_bytes,"actual_bytes":actual_bytes}),
                AppError::Telegram(TelegramError::Rejected {
                    code,
                    retry_after_seconds,
                }) => json!({"telegram_code":code,"retry_after_seconds":retry_after_seconds}),
                _ => Value::Null,
            };
            Ok(CallToolResult::structured_error(
                json!({"code":error.code(),"message":error.to_string(),"details":details}),
            ))
        }
    }
}
