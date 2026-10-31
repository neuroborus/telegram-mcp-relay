//! Explicit credential-free construction for the release resource probe, not production setup.

use std::{future::Future, net::SocketAddr};

use tokio::{net::TcpListener, sync::oneshot};

use super::Application;
use crate::{clients::telegram::TelegramClient, config::Config, error::AppError};

/// Run the actual daemon composition against a separate numeric loopback mock service.
///
/// Load settings with [`Config::load`] first. Use only synthetic IDs and a private temporary
/// database/export root. Credential sources are rejected without reading or creating secrets.
/// The caller retains the bound listener until transfer, avoiding port reservation races;
/// it must match `server.listen`. Readiness follows startup cleanup and service spawning.
/// Await this future after shutdown to drain accepted work, just as with [`super::run_until`].
pub async fn run_until(
    config: &Config,
    mock_address: SocketAddr,
    listener: TcpListener,
    ready: oneshot::Sender<()>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), AppError> {
    if config.telegram.bot_token_env.is_some()
        || config.telegram.bot_token_file.is_some()
        || config.server.auth_token_env.is_some()
        || config.server.auth_token_file.is_some()
        || config.server.create_auth_token_file
    {
        return Err(AppError::InvalidConfig("probe credential sources"));
    }
    if !config.server.listen.ip().is_loopback()
        || listener.local_addr().ok() != Some(config.server.listen)
    {
        return Err(AppError::InvalidConfig("probe listener"));
    }
    let telegram = TelegramClient::for_probe(config, mock_address)?;
    Application::new(config, telegram, None)?
        .serve_ready(listener, shutdown, Some(ready))
        .await
}
