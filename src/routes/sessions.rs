//! Bounded ownership around the SDK's local session worker; no application identities.

use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, Weak},
    task::{Context, Poll},
};

use futures_util::{Stream, task::AtomicWaker};
use rmcp::{
    RoleServer,
    model::{
        CancelledNotification, CancelledNotificationParam, ClientJsonRpcMessage,
        ClientNotification, GetExtensions, RequestId, ServerJsonRpcMessage,
    },
    transport::{
        Transport, WorkerTransport,
        common::server_side_http::session_id,
        streamable_http_server::session::{
            ServerSseMessage, SessionId, SessionManager,
            local::{
                LocalSessionHandle, LocalSessionWorker, LocalSessionWorkerError, SessionConfig,
                StreamableHttpMessageReceiver, create_local_session,
            },
        },
    },
};
use tokio::{
    sync::{Notify, OwnedSemaphorePermit, Semaphore},
    time::Instant,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::config::{
    MCP_CONTROL_REQUESTS, MCP_SESSION_ABANDONED, MCP_SESSION_IDLE, MCP_SESSION_INIT,
    MCP_SESSION_QUEUE, MCP_SESSION_SWEEP,
};

/// Errors deliberately omit SDK session IDs, event IDs and parser input.
#[derive(Debug)]
pub(super) struct SessionError;

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MCP transport session unavailable")
    }
}
impl std::error::Error for SessionError {}

pub(super) struct BoundedSessions {
    sessions: Arc<Mutex<HashMap<SessionId, Arc<Session>>>>,
    admission: Arc<Semaphore>,
    work: Arc<Semaphore>,
    control: Arc<Semaphore>,
    recovery: Arc<Semaphore>,
    tasks: TaskTracker,
    shutdown: CancellationToken,
    queue: usize,
}

struct Session {
    handle: LocalSessionHandle,
    state: Mutex<State>,
    changed: Arc<Notify>,
    stop: CancellationToken,
    retired: CancellationToken,
}

struct State {
    ready: bool,
    created: Instant,
    idle: Instant,
    work: Vec<Work>,
}

struct Work {
    usage: Weak<Usage>,
    request: Option<RequestId>,
    channel: Option<u64>,
    receiver: Weak<StreamState>,
    cancel_sent: bool,
    response_pending: bool,
    _permit: OwnedSemaphorePermit,
}

/// Held by SDK request/notification extensions AND the HTTP stream. A dropped body
/// cannot release work admission while a detached SDK handler still owns its context.
struct Usage {
    abandoned: Mutex<Option<Instant>>,
    changed: Arc<Notify>,
}

impl Drop for Usage {
    fn drop(&mut self) {
        self.changed.notify_one();
    }
}

// Keep cleanup possible after poisoning without logging private panic payloads.
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl BoundedSessions {
    pub(super) fn new(capacity: usize, shutdown: CancellationToken) -> Self {
        Self {
            sessions: Arc::default(),
            admission: Arc::new(Semaphore::new(capacity)),
            work: Arc::new(Semaphore::new(capacity)),
            control: Arc::new(Semaphore::new(MCP_CONTROL_REQUESTS)),
            recovery: Arc::new(Semaphore::new(MCP_CONTROL_REQUESTS)),
            tasks: TaskTracker::new(),
            shutdown,
            queue: capacity.clamp(2, MCP_SESSION_QUEUE),
        }
    }

    fn session(&self, id: &SessionId) -> Result<Arc<Session>, SessionError> {
        lock(&self.sessions)
            .get(id)
            .filter(|session| !session.stop.is_cancelled())
            .cloned()
            .ok_or(SessionError)
    }

    /// Endpoint shutdown awaits both SDK transport retirement and detached contexts.
    pub(super) async fn drain(&self) {
        self.shutdown.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }

    fn admit(
        &self,
        session: &Session,
        request: Option<RequestId>,
        control: bool,
        recovery: bool,
    ) -> Result<Arc<Usage>, SessionError> {
        // Reserve notification capacity so cancellation remains admissible when calls
        // consume the work budget; notifications cannot create an unbounded SDK backlog.
        let admission = if control { &self.control } else { &self.work };
        let permit = admission
            .clone()
            .try_acquire_owned()
            .or_else(|error| {
                if recovery {
                    self.recovery.clone().try_acquire_owned()
                } else {
                    Err(error)
                }
            })
            .map_err(|_| SessionError)?;
        let mut state = lock(&session.state);
        if session.stop.is_cancelled()
            || request.as_ref().is_some_and(|id| {
                state
                    .work
                    .iter()
                    .any(|work| work.request.as_ref() == Some(id))
            })
        {
            return Err(SessionError);
        }
        let usage = Arc::new(Usage {
            abandoned: Mutex::new(None),
            changed: session.changed.clone(),
        });
        state.idle = Instant::now();
        state.work.push(Work {
            usage: Arc::downgrade(&usage),
            response_pending: request.is_some(),
            request,
            channel: None,
            receiver: Weak::new(),
            cancel_sent: false,
            _permit: permit,
        });
        Ok(usage)
    }
}

impl Session {
    fn completed(&self, id: &RequestId) {
        if let Some(work) = lock(&self.state)
            .work
            .iter_mut()
            .find(|work| work.request.as_ref() == Some(id))
        {
            work.response_pending = false;
        }
        self.changed.notify_one();
    }

    /// An interrupted/failed channel operation leaves uncertain SDK state: retire
    /// that transport instead of admitting more work into a partially set-up channel.
    async fn operation<T>(
        &self,
        future: impl Future<Output = Result<T, impl std::fmt::Display>>,
    ) -> Result<T, SessionError> {
        let guard = self.stop.clone().drop_guard();
        let result = tokio::select! {
            biased;
            result = future => result.map_err(|_| SessionError),
            _ = self.stop.cancelled() => Err(SessionError),
            _ = tokio::time::sleep(MCP_SESSION_INIT) => Err(SessionError),
        }?;
        guard.disarm();
        Ok(result)
    }

    fn stream(&self, receiver: StreamableHttpMessageReceiver, usage: Arc<Usage>) -> SessionStream {
        let channel = receiver.http_request_id;
        let receiver = Arc::new(StreamState {
            inner: Mutex::new(Some((receiver, usage.clone()))),
            waker: AtomicWaker::new(),
        });
        let mut state = lock(&self.state);
        // Retirement cancels before sweeping under this lock. A late channel
        // must either join that sweep or release its receiver here.
        if self.stop.is_cancelled() {
            lock(&receiver.inner).take();
        } else if let Some(work) = state
            .work
            .iter_mut()
            .find(|work| work.usage.ptr_eq(&Arc::downgrade(&usage)))
        {
            work.channel = channel;
            work.receiver = Arc::downgrade(&receiver);
        }
        SessionStream { receiver }
    }

    async fn maintain(&self) {
        loop {
            let now = Instant::now();
            let expired = {
                let state = lock(&self.state);
                let attached = state.work.iter().any(|work| {
                    work.usage
                        .upgrade()
                        .is_some_and(|usage| lock(&usage.abandoned).is_none())
                });
                (!state.ready && now.duration_since(state.created) >= MCP_SESSION_INIT)
                    || (state.work.is_empty() && now.duration_since(state.idle) >= MCP_SESSION_IDLE)
                    || (!attached
                        && state.work.iter().any(|work| {
                            work.usage.upgrade().is_some_and(|usage| {
                                lock(&usage.abandoned).is_some_and(|at| {
                                    now.duration_since(at) >= MCP_SESSION_ABANDONED
                                })
                            })
                        }))
            };
            if expired || self.stop.is_cancelled() {
                break;
            }
            let cancel = lock(&self.state).work.iter_mut().find_map(|work| {
                let usage = work.usage.upgrade()?;
                if work.cancel_sent
                    || !lock(&usage.abandoned)
                        .is_some_and(|at| now.duration_since(at) >= MCP_SESSION_ABANDONED)
                {
                    return None;
                }
                let id = work.request.clone()?;
                work.cancel_sent = true;
                Some((id, usage))
            });
            if let Some((id, usage)) = cancel {
                let mut cancelled =
                    CancelledNotification::new(CancelledNotificationParam::new(Some(id), None));
                cancelled.extensions.insert(usage);
                let notification = ClientJsonRpcMessage::notification(cancelled.into());
                if self
                    .operation(self.handle.push_message(notification, None))
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
            // Retain each work permit through SDK channel removal, including periods
            // when no response body exists. This also bounds completed channel state.
            let dead = {
                let mut state = lock(&self.state);
                state
                    .work
                    .iter()
                    .position(|work| !work.response_pending && work.usage.strong_count() == 0)
                    .map(|index| state.work.swap_remove(index))
            };
            if let Some(work) = dead {
                if let Some(channel) = work.channel
                    && self
                        .operation(self.handle.close_request_wise_channel(channel))
                        .await
                        .is_err()
                {
                    break;
                }
                lock(&self.state).idle = Instant::now();
                continue;
            }
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => break,
                _ = self.changed.notified() => (),
                _ = tokio::time::sleep(MCP_SESSION_SWEEP) => (),
            }
        }
        self.stop.cancel();
        // Drop receivers even when an embedding caller keeps an unpolled body.
        // This releases workers backpressured on a full SSE channel before close.
        for work in &lock(&self.state).work {
            if let Some(receiver) = work.receiver.upgrade() {
                lock(&receiver.inner).take();
                receiver.waker.wake();
            }
        }
        self.retired.cancelled().await;
        // Cancellation-aware handlers release their extensions when they finish.
        // Retain admission until that happens, even if the transport already closed.
        loop {
            let notified = self.changed.notified();
            if lock(&self.state)
                .work
                .iter()
                .all(|work| work.usage.strong_count() == 0)
            {
                break;
            }
            notified.await;
        }
    }
}

impl SessionManager for BoundedSessions {
    type Error = SessionError;
    type Transport = SessionTransport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), SessionError> {
        let permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| SessionError)?;
        if self.shutdown.is_cancelled() {
            return Err(SessionError);
        }
        let id = session_id();
        let mut config = SessionConfig::default();
        config.channel_capacity = self.queue;
        // The SDK's activity timeout would expire attached long-running calls.
        config.keep_alive = None;
        config.init_timeout = Some(MCP_SESSION_INIT);
        config.completed_cache_ttl = std::time::Duration::ZERO;
        config.sse_retry = None;
        let (handle, worker) = create_local_session(id.clone(), config);
        let stop = self.shutdown.child_token();
        let retired = CancellationToken::new();
        let session = Arc::new(Session {
            handle,
            stop: stop.clone(),
            retired: retired.clone(),
            changed: Arc::new(Notify::new()),
            state: Mutex::new(State {
                ready: false,
                created: Instant::now(),
                idle: Instant::now(),
                work: Vec::new(),
            }),
        });
        lock(&self.sessions).insert(id.clone(), session.clone());
        let sessions = self.sessions.clone();
        let retiring_id = id.clone();
        let retiring_session = session.clone();
        self.tasks.spawn(async move {
            retiring_session.maintain().await;
            lock(&sessions).remove(&retiring_id);
            drop(permit);
        });
        Ok((
            id,
            SessionTransport {
                inner: Some(WorkerTransport::spawn_with_ct(worker, stop.clone())),
                stop,
                retired,
                tasks: self.tasks.clone(),
                session,
            },
        ))
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, SessionError> {
        let session = self.session(id)?;
        let response = session
            .operation(session.handle.initialize(message))
            .await?;
        if !matches!(response, ServerJsonRpcMessage::Response(_)) {
            session.stop.cancel();
        }
        Ok(response)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        Ok(self.session(id).is_ok())
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), SessionError> {
        if let Some(session) = lock(&self.sessions).get(id) {
            session.stop.cancel();
        }
        Ok(())
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        mut message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, SessionError> {
        let session = self.session(id)?;
        let ClientJsonRpcMessage::Request(request) = &mut message else {
            return Err(SessionError);
        };
        let usage = self.admit(
            &session,
            Some(request.id.clone()),
            false,
            super::control_request(&request.request),
        )?;
        request.request.extensions_mut().insert(usage.clone());
        // A single cancellation guard spans both SDK operations.
        session
            .operation(async {
                let receiver = session.handle.establish_request_wise_channel().await?;
                let channel = receiver.http_request_id;
                let stream = session.stream(receiver, usage);
                session.handle.push_message(message, channel).await?;
                Ok::<_, rmcp::transport::streamable_http_server::session::local::SessionError>(
                    stream,
                )
            })
            .await
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        mut message: ClientJsonRpcMessage,
    ) -> Result<(), SessionError> {
        let session = self.session(id)?;
        if let ClientJsonRpcMessage::Notification(notification) = &mut message {
            notification
                .notification
                .extensions_mut()
                .insert(self.admit(&session, None, true, false)?);
        } else {
            return Err(SessionError);
        }
        let initialized = matches!(&message, ClientJsonRpcMessage::Notification(n) if matches!(n.notification, ClientNotification::InitializedNotification(_)));
        session
            .operation(session.handle.push_message(message, None))
            .await?;
        if initialized {
            lock(&session.state).ready = true;
        }
        Ok(())
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, SessionError> {
        let session = self.session(id)?;
        let usage = self.admit(&session, None, false, false)?;
        let receiver = session
            .operation(session.handle.establish_common_channel())
            .await?;
        Ok(session.stream(receiver, usage))
    }

    async fn resume(
        &self,
        _id: &SessionId,
        _last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, SessionError> {
        // Completed channels are reclaimed immediately; no replay store is advertised.
        Err::<SessionStream, _>(SessionError)
    }
}

pub(super) struct SessionStream {
    receiver: Arc<StreamState>,
}

struct StreamState {
    inner: Mutex<Option<(StreamableHttpMessageReceiver, Arc<Usage>)>>,
    waker: AtomicWaker,
}

impl Stream for SessionStream {
    type Item = ServerSseMessage;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.waker.register(cx.waker());
        let mut receiver = lock(&self.receiver.inner);
        let result = match receiver.as_mut() {
            Some((receiver, _)) => receiver.inner.poll_recv(cx),
            None => Poll::Ready(None),
        };
        if matches!(result, Poll::Ready(None)) {
            receiver.take();
        }
        result
    }
}

impl Drop for SessionStream {
    fn drop(&mut self) {
        if let Some((_, usage)) = lock(&self.receiver.inner).take() {
            lock(&usage.abandoned).get_or_insert_with(Instant::now);
            usage.changed.notify_one();
        }
    }
}

/// The SDK spawns its service separately from HTTP. Keep transport retirement
/// tracked even when that service drops a failed/abandoned initialization.
pub(super) struct SessionTransport {
    inner: Option<WorkerTransport<LocalSessionWorker>>,
    stop: CancellationToken,
    retired: CancellationToken,
    tasks: TaskTracker,
    session: Arc<Session>,
}

impl Transport<RoleServer> for SessionTransport {
    type Error = LocalSessionWorkerError;

    fn send(
        &mut self,
        message: ServerJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let id = match &message {
            ServerJsonRpcMessage::Response(response) => Some(response.id.clone()),
            ServerJsonRpcMessage::Error(error) => error.id.clone(),
            _ => None,
        };
        let send = self.inner.as_mut().map(|inner| inner.send(message));
        let session = self.session.clone();
        async move {
            let result = send.ok_or(LocalSessionWorkerError::TransportClosed)?.await;
            if let Some(id) = id {
                session.completed(&id);
            }
            result
        }
    }

    async fn receive(&mut self) -> Option<ClientJsonRpcMessage> {
        let inner = self.inner.as_mut()?;
        let message = tokio::select! {
            biased;
            _ = self.stop.cancelled() => None,
            message = inner.receive() => message,
        };
        // The SDK suppresses terminal replies after cancellation. Mark those
        // requests only when cancellation reaches its service loop, retaining
        // ownership until the handler/notification contexts also finish.
        if let Some(ClientJsonRpcMessage::Notification(notification)) = &message
            && let ClientNotification::CancelledNotification(cancelled) = &notification.notification
            && let Some(id) = &cancelled.params.request_id
        {
            self.session.completed(id);
            for work in &lock(&self.session.state).work {
                if work.request.as_ref() == Some(id)
                    && let Some(usage) = work.usage.upgrade()
                {
                    lock(&usage.abandoned).get_or_insert_with(Instant::now);
                }
            }
        }
        message
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.stop.cancel();
        if let Some(inner) = self.inner.as_mut() {
            retire(inner).await?;
        }
        self.inner.take();
        self.retired.cancel();
        Ok(())
    }
}

impl Drop for SessionTransport {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(mut inner) = self.inner.take() {
            let retired = self.retired.clone();
            self.tasks.spawn(async move {
                let _ = retire(&mut inner).await;
                retired.cancel();
            });
        }
    }
}

async fn retire(
    inner: &mut WorkerTransport<LocalSessionWorker>,
) -> Result<(), LocalSessionWorkerError> {
    inner.cancel_token().cancel();
    // rmcp 3.8.0 waits for the initial handler reply outside its cancellation
    // select. Wake that wait when initialization was abandoned. The HTTP side
    // is already closed; this terminal message is only for worker retirement.
    let wake = inner.send(ServerJsonRpcMessage::error(
        rmcp::ErrorData::internal_error("Transport closed", None),
        None,
    ));
    let (_, ()) = tokio::join!(wake, async {
        // Also release a worker blocked forwarding a queued input to its handler.
        while inner.receive().await.is_some() {}
    });
    inner.close().await
}

#[cfg(test)]
mod tests;
