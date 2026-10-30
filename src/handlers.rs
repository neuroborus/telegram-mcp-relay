//! Thin typed MCP tools and resources over shared durable services.

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse, ResourceTemplate,
        ServerCapabilities, ServerConfig, SubscriptionFilter, Tool,
    },
    service::{RequestContext, SubscriptionContext},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    clients::telegram::TelegramClient,
    config::Config,
    dto::{
        AckRequest, AckResult, AskResult, AskStatus, ClaimedReplyResult, InboxRequest, InboxResult,
        ProcessingRequest, ProcessingResult, RegisterRequest, RegisterResult, SendFileRequest,
        SendRequest, SendResult,
    },
    error::{AppError, TelegramError},
    repositories::{Database, InboxNotifications},
    services::{
        ask::AskService, files::OutboundFiles, inbox::InboxService, outbound::OutboundService,
        typing::TypingService, unix_millis,
    },
};

mod inbox;
mod subscriptions;

/// Agent sessions live in SQLite, independently of HTTP connections or SDK handlers.
pub struct McpHandler {
    database: Arc<Mutex<Database>>,
    outbound: OutboundService,
    asks: AskService,
    files: OutboundFiles,
    inbox: InboxService,
    notifications: InboxNotifications,
}

impl McpHandler {
    /// Construct once; every per-request SDK factory must share this handler.
    pub fn new(
        database: Arc<Mutex<Database>>,
        telegram: TelegramClient,
        config: &Config,
    ) -> Result<Self, AppError> {
        let notifications = database
            .lock()
            .map_err(|_| AppError::StorageFailure)?
            .inbox_notifications
            .clone();
        let outbound = OutboundService::new(database.clone(), telegram, config)?;
        let inbox = InboxService::new(&config.delivery)?.with_typing(outbound.typing());
        Ok(Self {
            asks: AskService::new(database.clone(), outbound.clone(), &config.delivery)?,
            outbound,
            files: OutboundFiles::new(&config.files)?,
            inbox,
            database,
            notifications,
        })
    }

    pub(crate) fn typing(&self) -> TypingService {
        self.outbound.typing()
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

    async fn ask(&self, request: SendRequest) -> Result<AskResult, AppError> {
        let asked = self.asks.ask(&request.session_key, &request.text).await?;
        let status = if asked.reply.is_some() {
            AskStatus::ReplyReceived
        } else if asked.waited {
            AskStatus::WaitTimeout
        } else {
            AskStatus::Sent
        };
        Ok(AskResult {
            telegram_chat_id: asked.sent.chat.id,
            outbound_message_id: asked.sent.message_id,
            status,
            reply: asked
                .reply
                .map(|claim| -> Result<_, AppError> {
                    Ok(ClaimedReplyResult {
                        reply: inbox::reply(claim.reply)?,
                        claim_token: claim.claim_token,
                    })
                })
                .transpose()?,
        })
    }
}

impl ServerHandler for McpHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_server_info(Implementation::new(
            "telegram-mcp-relay",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(self.accept_inboxes(requested))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), ErrorData> {
        self.listen_inboxes(context).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        inbox::validate_listing(request)?;
        // Never enumerate session credentials or change listings on registration.
        Ok(ListResourcesResult::with_all_items(Vec::new())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private))
    }

    async fn list_resource_templates(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        inbox::validate_listing(request)?;
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("telegram-bridge://sessions/{session_key}/inbox", "telegram_inbox")
                .with_description("Read-only inbox; use your saved session key. Optional limit=1..100; follow next_uri for later pages. Reads never claim. Timestamps are Unix milliseconds.")
                .with_mime_type("application/json"),
        ]).with_ttl_ms(0).with_cache_scope(CacheScope::Private))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.read_inbox(&request.uri).map(Into::into)
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = match request.name.as_ref() {
            "telegram_register" => tool_result(self.register(arguments(&request)?)),
            "telegram_send" => tool_result(self.send(arguments(&request)?).await),
            "telegram_send_file" => tool_result(self.send_file(arguments(&request)?).await),
            "telegram_ask" => {
                let request = arguments(&request)?;
                tokio::select! {
                    biased;
                    _ = context.ct.cancelled() => return Err(ErrorData::internal_error("Request cancelled", None)),
                    result = self.ask(request) => tool_result(result),
                }
            }
            "telegram_inbox" => tool_result(self.claim_inbox(arguments(&request)?)),
            "telegram_processing" => tool_result(self.processing(arguments(&request)?)),
            "telegram_ack" => tool_result(self.acknowledge(arguments(&request)?)),
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
        Tool::new("telegram_inbox", "Atomically claim eligible replies on startup, resume or notification; preserve reply IDs and tokens. Never poll periodically. Read resources for snapshots without claiming.", serde_json::Map::new())
            .with_input_schema::<InboxRequest>().with_output_schema::<InboxResult>(),
        Tool::new("telegram_processing", "Confirm processing start or inactivity with active; neither completes the reply. Typing merges per chat and stops by ten minutes from first start. For explicit resume supply only reconcile_request_id and the saved token, then confirm active to resume typing; retain the rotated token.", serde_json::Map::new())
            .with_input_schema::<ProcessingRequest>().with_output_schema::<ProcessingResult>(),
        Tool::new("telegram_ack", "Acknowledge successful handling with the current session, reply ID and token. Identical valid-token retries are idempotent; stale tokens cannot complete work.", serde_json::Map::new())
            .with_input_schema::<AckRequest>().with_output_schema::<AckResult>(),
        Tool::new("telegram_ask", "Send a concise question and, when enabled, wait for a matching reply up to the server timeout. Events stay active. Preserve the reply ID and token; confirm processing and acknowledge separately. Timeout grants no permission and later replies remain valid. Never automatically retry an uncertain send.", serde_json::Map::new())
            .with_input_schema::<SendRequest>().with_output_schema::<AskResult>(),
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
                AppError::AskWaitFailed {
                    telegram_chat_id,
                    outbound_message_id,
                } => {
                    json!({"telegram_chat_id":telegram_chat_id,"outbound_message_id":outbound_message_id})
                }
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
