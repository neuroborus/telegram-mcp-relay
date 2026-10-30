//! Startup ordering and supervision for the single-process relay.

use std::{
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
};

use tokio::{
    net::TcpListener,
    sync::{oneshot, watch},
    task::{JoinError, JoinSet},
};
use tokio_util::sync::CancellationToken;

use crate::{
    clients::telegram::TelegramClient,
    config::{Config, Secret},
    error::AppError,
    handlers::McpHandler,
    repositories::Database,
    routes::McpEndpoint,
    services::{
        cleanup::CleanupService, notices::NoticeService, polling::PollingService,
        typing::TypingService,
    },
};

/// Load validated settings/secrets and run until a supervised service fails.
pub async fn run(config_path: &Path) -> Result<(), AppError> {
    run_until(config_path, std::future::pending()).await
}

/// The embedding host supplies cancellation without installing global signal handlers.
/// This stops service loops; it does not yet drain detached outbound bookkeeping.
pub async fn run_until(
    config_path: &Path,
    shutdown: impl Future<Output = ()>,
) -> Result<(), AppError> {
    let config = Config::load(config_path)?;
    let secrets = config.load_secrets()?;
    let telegram = TelegramClient::new(&config, &secrets.bot_token)?;
    let app = Application::new(&config, telegram, secrets.auth_token.as_ref())?;
    let listener = TcpListener::bind(config.server.listen)
        .await
        .map_err(|_| AppError::McpUnavailable)?;
    app.serve(listener, shutdown).await
}

struct Application {
    endpoint: McpEndpoint,
    polling: PollingService,
    notices: NoticeService,
    cleanup: CleanupService,
    typing: TypingService,
    stop: CancellationToken,
}

impl Application {
    fn new(
        config: &Config,
        telegram: TelegramClient,
        auth: Option<&Secret>,
    ) -> Result<Self, AppError> {
        let database = Arc::new(Mutex::new(Database::open(&config.storage, &config.limits)?));
        let handler = Arc::new(McpHandler::new(database.clone(), telegram.clone(), config)?);
        let typing = handler.typing();
        let cleanup =
            CleanupService::new(database.clone(), &config.storage)?.with_typing(typing.clone());
        let polling = PollingService::new(database.clone(), telegram.clone(), config)?;
        let notices = NoticeService::new(database, telegram, config)?;
        let stop = CancellationToken::new();
        let endpoint = McpEndpoint::new(config, auth, handler, stop.clone())?;
        Ok(Self {
            endpoint,
            polling,
            notices,
            cleanup,
            typing,
            stop,
        })
    }

    async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), AppError> {
        let Self {
            endpoint,
            polling,
            notices,
            cleanup,
            typing,
            stop,
        } = self;
        // Drop/cancellation unwinds the SDK and every owned service task too.
        let _cancel = stop.clone().drop_guard();
        let (stopping, receive) = watch::channel(false);
        let (ready, startup) = oneshot::channel();
        let mut workers = JoinSet::new();
        let cleanup_stop = receive.clone();
        workers.spawn(async move {
            let result = cleanup.run_ready(cleanup_stop, Some(ready)).await;
            ("cleanup", result)
        });
        tokio::pin!(shutdown);
        let result = async {
            tokio::select! {
                biased;
                _ = &mut shutdown => return Ok(()),
                result = workers.join_next() => return service_exit(result),
                ready = startup => { ready.map_err(|_| AppError::ServiceStopped)?; }
            }
            let poll_stop = receive.clone();
            workers.spawn(async move { ("polling", polling.run(poll_stop).await) });
            workers.spawn(async move { ("notices", notices.run(receive).await) });
            workers.spawn(async move { ("mcp", endpoint.serve(listener).await) });
            let typing_stop = stop.clone();
            workers.spawn(async move {
                (
                    "typing",
                    tokio::select! {
                        biased;
                        _ = typing_stop.cancelled() => Ok(()),
                        _ = typing.failed() => Err(AppError::ServiceStopped),
                    },
                )
            });
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "Relay started");
            tokio::select! {
                biased;
                _ = &mut shutdown => Ok(()),
                result = workers.join_next() => service_exit(result),
            }
        }
        .await;
        stop.cancel();
        stopping.send_replace(true);
        let mut result = result;
        while let Some(exited) = workers.join_next().await {
            // During requested shutdown, normal exits are expected; failures still surface.
            let failure = match exited {
                Ok((_, Ok(()))) => continue,
                other => service_exit(Some(other)),
            };
            if result.is_ok() {
                result = failure;
            }
        }
        result
    }
}

type ServiceResult = Result<(&'static str, Result<(), AppError>), JoinError>;

fn service_exit(result: Option<ServiceResult>) -> Result<(), AppError> {
    match result {
        Some(Ok((service, result))) => {
            let error = result.err().unwrap_or(AppError::ServiceStopped);
            tracing::error!(service, code = error.code(), "Supervised service stopped");
            Err(error)
        }
        // JoinError may contain a private panic payload. Never format it.
        _ => Err(AppError::ServiceStopped),
    }
}

#[cfg(test)]
mod tests;
