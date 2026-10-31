//! Release measurement entry point; workloads and the Telegram mock run separately.

#![forbid(unsafe_code)]

use std::{
    ffi::OsString, future::Future, io::Write, net::SocketAddr, path::PathBuf, process::ExitCode,
};

use telegram_mcp_relay::{app::probe, config::Config, error::AppError};
use tokio::{net::TcpListener, sync::oneshot};

struct Options {
    config: PathBuf,
    mock: SocketAddr,
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Option<Self> {
        let config = arguments.next()?.into();
        let mock: SocketAddr = arguments.next()?.to_str()?.parse().ok()?;
        if arguments.next().is_some() || !mock.ip().is_loopback() || mock.port() == 0 {
            return None;
        }
        Some(Self { config, mock })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let Some(options) = Options::parse(std::env::args_os().skip(1)) else {
        eprintln!("Usage: resource_probe <synthetic-config.toml> <loopback-mock-ip:port>");
        return ExitCode::FAILURE;
    };
    match run(options).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Resource probe stopped: {}", error.code());
            ExitCode::FAILURE
        }
    }
}

async fn run(options: Options) -> Result<(), AppError> {
    let config = Config::load(&options.config)?;
    // Install both signal listeners before constructing services; repeated signals leave
    // the admitted-send drain intact. No global handlers are added by the library.
    let signal = shutdown_signal().map_err(|_| AppError::ServiceStopped)?;
    let (stop, stopping) = oneshot::channel();
    let shutdown = async {
        tokio::select! {
            _ = signal => (),
            _ = stopping => (),
        }
    };
    let listener = TcpListener::bind(config.server.listen)
        .await
        .map_err(|_| AppError::McpUnavailable)?;
    let (ready, startup) = oneshot::channel();
    let running = probe::run_until(&config, options.mock, listener, ready, shutdown);
    tokio::pin!(running);
    tokio::select! {
        biased;
        result = &mut running => return result,
        result = startup => {
            if result.is_err() {
                let _ = stop.send(());
                return running.await;
            }
        }
    }
    // A single bounded machine-readable line. No credentials, paths or file contents.
    // Failure to report readiness requests a graceful drain instead of dropping the app.
    if writeln!(
        std::io::stdout(),
        "{{\"event\":\"ready\",\"mcp_address\":\"{}\"}}",
        config.server.listen
    )
    .and_then(|_| std::io::stdout().flush())
    .is_err()
    {
        let _ = stop.send(());
        let _ = running.await;
        return Err(AppError::ServiceStopped);
    }
    running.await
}

fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => (),
            _ = terminate.recv() => (),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_require_exactly_a_config_and_numeric_loopback_peer() {
        for arguments in [
            vec![],
            vec!["synthetic.toml"],
            vec!["synthetic.toml", "localhost:9000"],
            vec!["synthetic.toml", "192.0.2.1:9000"],
            vec!["synthetic.toml", "0.0.0.0:9000"],
            vec!["synthetic.toml", "127.0.0.1:0"],
            vec!["synthetic.toml", "127.0.0.1:9000", "extra"],
        ] {
            assert!(Options::parse(arguments.into_iter().map(OsString::from)).is_none());
        }
        for address in ["127.0.0.1:9000", "[::1]:9000"] {
            let options =
                Options::parse(["synthetic.toml", address].into_iter().map(OsString::from))
                    .unwrap();
            assert_eq!(options.config, PathBuf::from("synthetic.toml"));
            assert_eq!(options.mock, address.parse().unwrap());
        }
    }
}
