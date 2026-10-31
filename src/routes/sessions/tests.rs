use super::*;
use std::borrow::Cow;

use futures_util::StreamExt;
use rmcp::{
    ErrorData, ServerHandler,
    model::{ListToolsResult, ProtocolVersion, ServerConfig},
    service::RequestContext,
};
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle};

fn initialize() -> ClientJsonRpcMessage {
    serde_json::from_value(json!({
        "jsonrpc":"2.0", "id":0, "method":"initialize", "params":{
            "protocolVersion":"2025-06-18", "capabilities":{},
            "clientInfo":{"name":"synthetic-client", "version":"1"}
        }
    }))
    .unwrap()
}

fn request(id: i64) -> ClientJsonRpcMessage {
    serde_json::from_value(json!({"jsonrpc":"2.0", "id":id, "method":"tools/list"})).unwrap()
}

struct Probe {
    started: mpsc::Sender<()>,
    gate: Arc<Semaphore>,
}

impl ServerHandler for Probe {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.protocol_version = ProtocolVersion::V_2025_06_18;
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2025_06_18])
    }

    async fn list_tools(
        &self,
        _params: Option<rmcp::model::PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.started.send(()).await.unwrap();
        tokio::select! {
            _ = context.ct.cancelled() => Err(ErrorData::internal_error("Cancelled", None)),
            permit = self.gate.acquire() => {
                permit.unwrap().forget();
                Ok(ListToolsResult::default())
            }
        }
    }
}

struct Client {
    id: SessionId,
    server: JoinHandle<()>,
    started: mpsc::Receiver<()>,
    gate: Arc<Semaphore>,
}

async fn connect(manager: &BoundedSessions, initialized: bool) -> Client {
    let (id, transport) = manager.create_session().await.unwrap();
    let (started, receive) = mpsc::channel(8);
    let gate = Arc::new(Semaphore::new(0));
    let probe = Probe {
        started,
        gate: gate.clone(),
    };
    let server = tokio::spawn(async move {
        if let Ok(service) = rmcp::serve_server(probe, transport).await {
            let _ = service.waiting().await;
        }
    });
    let response = manager.initialize_session(&id, initialize()).await.unwrap();
    assert!(matches!(response, ServerJsonRpcMessage::Response(_)));
    if initialized {
        manager
            .accept_message(
                &id,
                serde_json::from_value(json!({
                    "jsonrpc":"2.0", "method":"notifications/initialized"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        // Admission is returned only after the SDK consumes the notification.
        let permit = manager.control.clone().acquire_owned().await.unwrap();
        drop(permit);
    }
    Client {
        id,
        server,
        started: receive,
        gate,
    }
}

async fn reclaimed(manager: &BoundedSessions, capacity: u32) {
    let permits = manager
        .admission
        .clone()
        .acquire_many_owned(capacity)
        .await
        .unwrap();
    assert!(lock(&manager.sessions).is_empty());
    drop(permits);
}

#[tokio::test]
async fn admission_includes_retiring_workers_and_closure_is_idempotent() {
    let manager = BoundedSessions::new(2, CancellationToken::new());
    let (first, one) = manager.create_session().await.unwrap();
    let (second, two) = manager.create_session().await.unwrap();
    assert!(manager.create_session().await.is_err());
    manager.close_session(&first).await.unwrap();
    manager.close_session(&first).await.unwrap();
    assert!(!manager.has_session(&first).await.unwrap());
    assert!(manager.has_session(&second).await.unwrap());
    // Closing HTTP state alone does not free a still-owned SDK transport.
    assert!(manager.create_session().await.is_err());
    drop(one);
    drop(two);
    reclaimed(&manager, 2).await;
    manager.drain().await;
    assert!(manager.create_session().await.is_err());
}

#[tokio::test]
async fn failed_initialization_reclaims_admission() {
    struct ModernOnly;
    impl ServerHandler for ModernOnly {
        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
        }
        fn get_info(&self) -> ServerConfig {
            let mut info = ServerConfig::default();
            info.protocol_version = ProtocolVersion::V_2026_07_28;
            info
        }
    }
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let (id, transport) = manager.create_session().await.unwrap();
    // A server with no handshake protocol must reject initialize.
    let server = tokio::spawn(async move {
        assert!(rmcp::serve_server(ModernOnly, transport).await.is_err());
    });
    let result = manager.initialize_session(&id, initialize()).await;
    assert!(!matches!(result, Ok(ServerJsonRpcMessage::Response(_))));
    server.await.unwrap();
    reclaimed(&manager, 1).await;
    manager.drain().await;
}

#[tokio::test]
async fn abandoned_initialize_retires_a_worker_waiting_for_its_handler_reply() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let (id, mut transport) = manager.create_session().await.unwrap();
    let mut initializing = Box::pin(manager.initialize_session(&id, initialize()));
    assert!(futures_util::poll!(initializing.as_mut()).is_pending());
    // Consume the SDK worker's initialization input but supply no handler reply.
    assert!(matches!(
        transport.receive().await,
        Some(ClientJsonRpcMessage::Request(_))
    ));
    drop(initializing);
    assert!(!manager.has_session(&id).await.unwrap());
    drop(transport);
    reclaimed(&manager, 1).await;
    manager.drain().await;
}

#[tokio::test(start_paused = true)]
async fn missing_initialized_notification_expires_the_handshake() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let client = connect(&manager, false).await;
    tokio::time::advance(MCP_SESSION_INIT + MCP_SESSION_SWEEP).await;
    client.server.await.unwrap();
    reclaimed(&manager, 1).await;
    manager.drain().await;
}

#[tokio::test]
async fn work_saturation_duplicate_ids_and_completed_channels_are_reclaimed() {
    let manager = BoundedSessions::new(2, CancellationToken::new());
    let mut client = connect(&manager, true).await;
    let first = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    assert!(manager.create_stream(&client.id, request(1)).await.is_err());
    let second = manager.create_stream(&client.id, request(2)).await.unwrap();
    client.started.recv().await.unwrap();
    assert!(manager.create_stream(&client.id, request(3)).await.is_err());
    client.gate.add_permits(2);
    assert_eq!(first.collect::<Vec<_>>().await.len(), 1);
    assert_eq!(second.collect::<Vec<_>>().await.len(), 1);
    let permits = manager.work.clone().acquire_many_owned(2).await.unwrap();
    assert!(
        lock(&manager.session(&client.id).unwrap().state)
            .work
            .is_empty()
    );
    drop(permits);
    // Successful calls do not consume a lifetime quota or leave replay channels.
    let next = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    client.gate.add_permits(1);
    assert_eq!(next.collect::<Vec<_>>().await.len(), 1);
    manager.close_session(&client.id).await.unwrap();
    client.server.await.unwrap();
    manager.drain().await;
}

#[tokio::test]
async fn dropped_body_keeps_work_admission_until_the_sdk_handler_finishes() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let mut client = connect(&manager, true).await;
    let stream = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    drop(stream);
    assert_eq!(manager.work.available_permits(), 0);
    assert!(manager.create_stream(&client.id, request(2)).await.is_err());
    client.gate.add_permits(1);
    let permit = manager.work.clone().acquire_owned().await.unwrap();
    drop(permit);
    assert!(manager.has_session(&client.id).await.unwrap());
    manager.close_session(&client.id).await.unwrap();
    client.server.await.unwrap();
    manager.drain().await;
}

#[tokio::test(start_paused = true)]
async fn finished_handler_keeps_admission_until_its_terminal_reply_is_delivered() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let (id, mut transport) = manager.create_session().await.unwrap();
    let mut initializing = Box::pin(manager.initialize_session(&id, initialize()));
    assert!(futures_util::poll!(initializing.as_mut()).is_pending());
    assert!(transport.receive().await.is_some());
    transport
        .send(
            serde_json::from_value(json!({
                "jsonrpc":"2.0", "id":0, "result":{
                    "protocolVersion":"2025-06-18", "capabilities":{},
                    "serverInfo":{"name":"synthetic-server", "version":"1"}
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    initializing.await.unwrap();
    let stream = manager.create_stream(&id, request(1)).await.unwrap();
    let context_owner = transport.receive().await.unwrap();
    drop(stream);
    drop(context_owner); // Handler finished; response is still queued in the SDK.
    tokio::time::advance(MCP_SESSION_SWEEP).await;
    tokio::task::yield_now().await;
    assert_eq!(manager.work.available_permits(), 0);
    assert!(manager.create_stream(&id, request(2)).await.is_err());
    transport
        .send(
            serde_json::from_value(json!({
                "jsonrpc":"2.0", "id":1, "result":{"tools":[]}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let permit = manager.work.clone().acquire_owned().await.unwrap();
    drop(permit);
    drop(transport);
    manager.drain().await;
}

#[tokio::test(start_paused = true)]
async fn abandonment_cancels_work_and_recovers_session_capacity() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let mut client = connect(&manager, true).await;
    let stream = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    drop(stream);
    tokio::time::advance(MCP_SESSION_ABANDONED + MCP_SESSION_SWEEP).await;
    client.server.await.unwrap();
    reclaimed(&manager, 1).await;
    assert_eq!(manager.work.available_permits(), 1);
    let replacement = connect(&manager, true).await;
    manager.close_session(&replacement.id).await.unwrap();
    replacement.server.await.unwrap();
    manager.drain().await;
}

#[tokio::test(start_paused = true)]
async fn abandoned_request_cancellation_preserves_an_attached_sibling_call() {
    let manager = BoundedSessions::new(2, CancellationToken::new());
    let mut client = connect(&manager, true).await;
    let abandoned = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    let attached = manager.create_stream(&client.id, request(2)).await.unwrap();
    client.started.recv().await.unwrap();
    drop(abandoned);
    tokio::time::advance(MCP_SESSION_ABANDONED + MCP_SESSION_SWEEP).await;
    let permit = manager.work.clone().acquire_owned().await.unwrap();
    assert!(manager.has_session(&client.id).await.unwrap());
    assert!(!client.server.is_finished());
    drop(permit);
    client.gate.add_permits(1);
    assert_eq!(attached.collect::<Vec<_>>().await.len(), 1);
    manager.close_session(&client.id).await.unwrap();
    client.server.await.unwrap();
    manager.drain().await;
}

#[tokio::test]
async fn explicit_cancellation_is_admitted_when_request_work_is_saturated() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let mut client = connect(&manager, true).await;
    let stream = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    manager
        .accept_message(
            &client.id,
            serde_json::from_value(json!({
                "jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":1}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(stream.collect::<Vec<_>>().await.is_empty());
    let permit = manager.work.clone().acquire_owned().await.unwrap();
    drop(permit);
    assert!(manager.has_session(&client.id).await.unwrap());
    manager.close_session(&client.id).await.unwrap();
    client.server.await.unwrap();
    manager.drain().await;
}

#[tokio::test(start_paused = true)]
async fn channel_setup_ready_across_retirement_releases_an_unpolled_late_stream() {
    struct SweepWitness(Notify);
    impl futures_util::task::ArcWake for SweepWitness {
        fn wake_by_ref(witness: &Arc<Self>) {
            witness.0.notify_one();
        }
    }

    let manager = BoundedSessions::new(2, CancellationToken::new());
    let client = connect(&manager, true).await;
    let session = manager.session(&client.id).unwrap();
    let usage = manager.admit(&session, None, false).unwrap();
    let receiver = session.handle.establish_common_channel().await.unwrap();
    let registered = session.stream(receiver, usage);
    let sweep = Arc::new(SweepWitness(Notify::new()));
    // This witness is woken only by the retirement sweep, independently of SDK EOF.
    registered
        .receiver
        .waker
        .register(&futures_util::task::waker_ref(&sweep));

    let late_usage = manager.admit(&session, None, false).unwrap();
    let receiver = session.handle.establish_common_channel().await.unwrap();
    let setup = std::future::ready(Ok::<_, SessionError>(receiver));
    manager.close_session(&client.id).await.unwrap();
    tokio::time::timeout(MCP_SESSION_INIT, sweep.0.notified())
        .await
        .unwrap();

    // The SDK result was ready before cancellation, but is consumed after the sweep.
    let receiver = session.operation(setup).await.unwrap();
    let late = session.stream(receiver, late_usage);
    drop(session);
    tokio::time::timeout(MCP_SESSION_INIT, manager.drain())
        .await
        .unwrap();
    client.server.await.unwrap();
    assert_eq!(manager.work.available_permits(), 2);
    assert_eq!(manager.admission.available_permits(), 2);
    // Neither returned stream was polled or dropped while reclamation completed.
    drop((registered, late));
}

#[tokio::test(start_paused = true)]
async fn retained_stream_at_eof_releases_work_and_allows_idle_reclamation() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let mut client = connect(&manager, true).await;
    let mut stream = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    client.gate.add_permits(1);
    assert!(stream.next().await.is_some());
    assert!(stream.next().await.is_none());
    let permit = tokio::time::timeout(MCP_SESSION_INIT, manager.work.clone().acquire_owned())
        .await
        .unwrap()
        .unwrap();
    drop(permit);
    assert!(manager.has_session(&client.id).await.unwrap());

    tokio::time::advance(MCP_SESSION_IDLE + MCP_SESSION_SWEEP).await;
    tokio::time::timeout(MCP_SESSION_INIT, client.server)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(MCP_SESSION_INIT, reclaimed(&manager, 1))
        .await
        .unwrap();
    manager.drain().await;
    assert_eq!(manager.work.available_permits(), 1);
    // Retain the completed stream through idle expiry and verify subsequent EOF.
    assert!(stream.next().await.is_none());
}

#[tokio::test(start_paused = true)]
async fn attached_long_calls_outlive_idle_expiry_and_shutdown_drops_unpolled_receivers() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let mut client = connect(&manager, true).await;
    let mut stream = manager.create_stream(&client.id, request(1)).await.unwrap();
    client.started.recv().await.unwrap();
    tokio::time::advance(MCP_SESSION_IDLE * 2).await;
    tokio::task::yield_now().await;
    assert!(manager.has_session(&client.id).await.unwrap());
    assert!(!client.server.is_finished());
    manager.drain().await;
    client.server.await.unwrap();
    assert!(stream.next().await.is_none());
    assert_eq!(manager.work.available_permits(), 1);
    assert_eq!(manager.admission.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn idle_sessions_expire_without_sdk_keep_alive_terminating_active_calls() {
    let manager = BoundedSessions::new(1, CancellationToken::new());
    let client = connect(&manager, true).await;
    tokio::time::advance(MCP_SESSION_IDLE + MCP_SESSION_SWEEP).await;
    client.server.await.unwrap();
    reclaimed(&manager, 1).await;
    manager.drain().await;
}
