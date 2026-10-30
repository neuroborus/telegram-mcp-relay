//! Protected `/mcp` HTTP/1 transport; rmcp owns MCP parsing, dispatch and SSE.

use std::{
    convert::Infallible,
    fmt::Write as _,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use http::{
    Request, Response, StatusCode,
    header::{self, HeaderValue},
};
use http_body_util::{BodyExt, Full, Limited, combinators::BoxBody};
use hyper::{
    body::{Body, Bytes, Frame, SizeHint},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use rmcp::{
    ServerHandler,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio::{
    net::TcpListener,
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::{
    config::{Config, Secret},
    error::AppError,
    handlers::McpHandler,
};

type ResponseBody = BoxBody<Bytes, Infallible>;

/// Generated locally; arbitrary client correlation values never enter diagnostics.
#[derive(Clone, Debug)]
pub struct RequestCorrelation(String);

impl RequestCorrelation {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Clones share the SDK factory, application handler and request admission.
pub struct McpEndpoint<S = McpHandler> {
    inner: Arc<Endpoint<S>>,
}

struct Endpoint<S> {
    sdk: StreamableHttpService<Arc<S>, LocalSessionManager>,
    listen: std::net::SocketAddr,
    hosts: [String; 2],
    auth: Option<HeaderValue>,
    admission: Arc<Semaphore>,
    concurrency: usize,
    request_bytes: usize,
    shutdown: CancellationToken,
}

impl<S> Clone for McpEndpoint<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S: ServerHandler> McpEndpoint<S> {
    /// Supply resolved secrets and one shared handler, never a connection-local registry.
    /// The production handler advertises only the modern per-request lifecycle.
    pub fn new(
        config: &Config,
        auth: Option<&Secret>,
        handler: Arc<S>,
        shutdown: CancellationToken,
    ) -> Result<Self, AppError> {
        let listen = config.server.listen;
        if !listen.ip().is_loopback() || listen.port() == 0 {
            return Err(AppError::InvalidConfig("server.listen"));
        }
        if !(1..=1_024).contains(&config.limits.concurrent_requests)
            || !(1..=1_048_576).contains(&config.limits.request_bytes)
        {
            return Err(AppError::InvalidConfig("limits HTTP admission"));
        }
        if (config.server.auth_token_env.is_some() || config.server.auth_token_file.is_some())
            != auth.is_some()
        {
            return Err(AppError::SecretUnavailable("server.auth_token"));
        }
        let auth = auth
            .map(|secret| {
                let mut value = HeaderValue::from_str(&format!("Bearer {}", secret.expose()))
                    .map_err(|_| AppError::SecretUnavailable("server.auth_token"))?;
                value.set_sensitive(true);
                Ok::<_, AppError>(value)
            })
            .transpose()?;
        let hosts = [listen.to_string(), format!("localhost:{}", listen.port())];
        let sdk_config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_stateless_protocol_metadata_required(true)
            .with_json_response(true)
            .with_allowed_hosts(hosts.clone())
            .with_allowed_origins(hosts.iter().map(|host| format!("http://{host}")))
            .with_max_request_body_bytes(config.limits.request_bytes as usize)
            .with_cancellation_token(shutdown.clone());
        Ok(Self {
            inner: Arc::new(Endpoint {
                sdk: StreamableHttpService::new(
                    move || Ok(handler.clone()),
                    Arc::new(LocalSessionManager::default()),
                    sdk_config,
                ),
                listen,
                hosts,
                auth,
                admission: Arc::new(Semaphore::new(config.limits.concurrent_requests as usize)),
                concurrency: config.limits.concurrent_requests as usize,
                request_bytes: config.limits.request_bytes as usize,
                shutdown,
            }),
        })
    }

    /// Serve only the configured loopback listener. Connections and header parsing are
    /// bounded independently of request/SSE admission; no whole-request timeout is imposed.
    pub async fn serve(self, listener: TcpListener) -> Result<(), AppError> {
        if listener
            .local_addr()
            .map_err(|_| AppError::McpUnavailable)?
            != self.inner.listen
        {
            return Err(AppError::InvalidConfig("MCP listener"));
        }
        let mut connections = JoinSet::new();
        let result = loop {
            tokio::select! {
                biased;
                _ = self.inner.shutdown.cancelled() => break Ok(()),
                result = connections.join_next(), if !connections.is_empty() => {
                    if result.is_some_and(|result| result.is_err()) {
                        break Err(AppError::McpUnavailable);
                    }
                }
                accepted = listener.accept(), if connections.len() < self.inner.concurrency * 2 => {
                    let (socket, _) = match accepted {
                        Ok(accepted) => accepted,
                        Err(_) => break Err(AppError::McpUnavailable),
                    };
                    let endpoint = self.clone();
                    connections.spawn(async move {
                        let shutdown = endpoint.inner.shutdown.clone();
                        let service = service_fn(move |request| {
                            let endpoint = endpoint.clone();
                            async move { Ok::<_, Infallible>(endpoint.handle(request).await) }
                        });
                        let mut builder = http1::Builder::new();
                        builder.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(10))
                            .max_headers(32).max_header_size(16_384).max_buf_size(16_384);
                        let connection = builder.serve_connection(TokioIo::new(socket), service);
                        tokio::pin!(connection);
                        tokio::select! {
                            biased;
                            // Do not wait on a client stalled in a partial body or SSE.
                            _ = shutdown.cancelled() => (),
                            _ = &mut connection => (),
                        }
                    });
                }
            }
        };
        self.inner.shutdown.cancel();
        while connections.join_next().await.is_some() {}
        result
    }

    /// Also usable by an embedding HTTP server; admission lives until body completion/drop.
    pub async fn handle<B>(&self, mut request: Request<B>) -> Response<ResponseBody>
    where
        B: Body<Data = Bytes> + Send + 'static,
        B::Error: std::fmt::Display,
    {
        if self.inner.shutdown.is_cancelled() {
            return rejection(StatusCode::SERVICE_UNAVAILABLE);
        }
        if request.uri().path() != "/mcp" || request.uri().query().is_some() {
            return rejection(StatusCode::NOT_FOUND);
        }
        if !self.valid_headers(&request) {
            return rejection(StatusCode::FORBIDDEN);
        }
        if let Some(expected) = &self.inner.auth
            && (request
                .headers()
                .get_all(header::AUTHORIZATION)
                .iter()
                .count()
                != 1
                || request.headers().get(header::AUTHORIZATION) != Some(expected))
        {
            let mut response = rejection(StatusCode::UNAUTHORIZED);
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            return response;
        }
        let Ok(permit) = self.inner.admission.clone().try_acquire_owned() else {
            return rejection(StatusCode::SERVICE_UNAVAILABLE);
        };
        let mut random = [0; 16];
        if getrandom::fill(&mut random).is_err() {
            return rejection(StatusCode::SERVICE_UNAVAILABLE);
        }
        let mut correlation = String::with_capacity(random.len() * 2);
        for byte in random {
            if write!(correlation, "{byte:02x}").is_err() {
                return rejection(StatusCode::SERVICE_UNAVAILABLE);
            }
        }
        let correlation = RequestCorrelation(correlation);
        request.headers_mut().remove(header::AUTHORIZATION);
        request.headers_mut().remove(header::PROXY_AUTHORIZATION);
        request.headers_mut().remove(header::COOKIE);
        request.headers_mut().remove("x-request-id");
        request.extensions_mut().insert(correlation.clone());
        let response = tokio::select! {
            biased;
            _ = self.inner.shutdown.cancelled() => return rejection(StatusCode::SERVICE_UNAVAILABLE),
            response = self.inner.sdk.handle(request)
                .instrument(tracing::info_span!("mcp_request", request_id = correlation.as_str())) => response,
        };
        let mut response = safe_response(response, self.inner.request_bytes).await;
        tracing::debug!(
            request_id = correlation.as_str(),
            status = response.status().as_u16(),
            "MCP HTTP request completed"
        );
        // Generated hex is always a valid header; fallible construction still fails closed.
        if let Ok(value) = HeaderValue::from_str(correlation.as_str()) {
            response.headers_mut().insert("x-request-id", value);
        }
        response.map(|body| {
            AdmittedBody {
                body,
                permit: Some(permit),
            }
            .boxed()
        })
    }

    fn valid_headers<B>(&self, request: &Request<B>) -> bool {
        let headers = request.headers();
        let allowed_host = |value: &str| {
            self.inner
                .hosts
                .iter()
                .any(|host| value.eq_ignore_ascii_case(host))
        };
        if headers.get_all(header::HOST).iter().count() != 1
            || !headers
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .is_some_and(allowed_host)
            || request
                .uri()
                .authority()
                .is_some_and(|authority| !allowed_host(authority.as_str()))
        {
            return false;
        }
        let origins = headers.get_all(header::ORIGIN);
        origins.iter().count() <= 1
            && origins.iter().all(|value| {
                value
                    .to_str()
                    .ok()
                    .and_then(|value| value.strip_prefix("http://"))
                    .is_some_and(allowed_host)
            })
    }
}

fn rejection(status: StatusCode) -> Response<ResponseBody> {
    let mut response =
        Response::new(Full::new(Bytes::from_static(b"MCP HTTP request rejected")).boxed());
    *response.status_mut() = status;
    response
}

/// SDK plain HTTP failures may contain parser input. Keep JSON-RPC codes and IDs intact.
async fn safe_response(response: Response<ResponseBody>, limit: usize) -> Response<ResponseBody> {
    if response.status().is_success() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    parts.headers.remove(header::CONTENT_LENGTH);
    let json = parts
        .headers
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value == "application/json");
    let bytes = if json {
        Limited::new(body, limit.saturating_add(4_096))
            .collect()
            .await
            .ok()
            .map(|body| body.to_bytes())
    } else {
        None
    };
    let body = bytes
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|mut message| {
            let error = message.get_mut("error")?.as_object_mut()?;
            error.insert("message".into(), "MCP protocol request rejected".into());
            // Version/capability data is the SDK's structured negotiation contract.
            serde_json::to_vec(&message).ok()
        });
    let body = body
        .map(Bytes::from)
        .unwrap_or_else(|| Bytes::from_static(b"MCP HTTP request rejected"));
    Response::from_parts(parts, Full::new(body).boxed())
}

struct AdmittedBody {
    body: ResponseBody,
    permit: Option<OwnedSemaphorePermit>,
}

impl Body for AdmittedBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let result = Pin::new(&mut self.body).poll_frame(cx);
        if matches!(result, Poll::Ready(None)) || self.body.is_end_stream() {
            self.permit.take();
        }
        result
    }
    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
mod tests;
