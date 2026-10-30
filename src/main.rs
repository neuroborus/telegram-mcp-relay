//! Binary bootstrap for the shared relay application.

#![forbid(unsafe_code)]

use std::{future::Future, process::ExitCode};
use tracing_subscriber::{filter::Targets, prelude::*};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // SDK diagnostics can include raw request metadata; emit our safe boundary events.
    if tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_writer(std::io::stderr)
                .with_filter(
                    Targets::new()
                        .with_default(tracing::Level::INFO)
                        .with_target("rmcp", tracing_subscriber::filter::LevelFilter::OFF),
                ),
        )
        .try_init()
        .is_err()
    {
        eprintln!("Failed to initialize diagnostics");
        return ExitCode::FAILURE;
    }

    let mut arguments = std::env::args_os().skip(1);
    let Some(path) = arguments.next().filter(|_| arguments.next().is_none()) else {
        eprintln!("Usage: telegram-mcp-relay <config.toml>");
        return ExitCode::FAILURE;
    };
    // Register both signals before starting services. Keep global handlers out of the
    // library entry points, and await the application after a signal so it can drain.
    let shutdown = match shutdown_signal() {
        Ok(shutdown) => shutdown,
        Err(_) => {
            tracing::error!(
                code = "SHUTDOWN_SIGNAL_UNAVAILABLE",
                "Cannot register shutdown signals"
            );
            return ExitCode::FAILURE;
        }
    };
    match telegram_mcp_relay::run_until(std::path::Path::new(&path), shutdown).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(code = error.code(), "Relay stopped");
            ExitCode::FAILURE
        }
    }
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
    use rustix::process::{Pid, Signal, kill_process};
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::Duration,
    };

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn interrupt_and_terminate_allow_work_to_drain_before_process_exit() {
        for signal in [Signal::INT, Signal::TERM] {
            let mut child = ChildGuard(
                Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "tests::signal_child", "--nocapture"])
                    .env("RELAY_TEST_SIGNAL_CHILD", "1")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            let output = child.0.stdout.take().unwrap();
            let (sent, received) = mpsc::channel();
            let reader = std::thread::spawn(move || {
                for line in BufReader::new(output).lines() {
                    if sent.send(line.unwrap()).is_err() {
                        break;
                    }
                }
            });
            let await_line = |expected: &str| {
                loop {
                    let line = received.recv_timeout(Duration::from_secs(10)).unwrap();
                    if line.ends_with(expected) {
                        break;
                    }
                }
            };
            await_line("SIGNAL_READY");
            let pid = Pid::from_raw(child.0.id() as i32).unwrap();
            kill_process(pid, signal).unwrap();
            await_line("DRAINING");
            assert!(child.0.try_wait().unwrap().is_none());
            // Repeated shutdown signals must not interrupt admitted bookkeeping.
            kill_process(pid, signal).unwrap();
            writeln!(child.0.stdin.take().unwrap(), "finish").unwrap();
            await_line("DRAINED");
            assert!(child.0.wait().unwrap().success());
            reader.join().unwrap();
        }
    }

    #[test]
    fn signal_child() {
        if std::env::var_os("RELAY_TEST_SIGNAL_CHILD").is_none() {
            return;
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let shutdown = shutdown_signal().unwrap();
                println!("SIGNAL_READY");
                std::io::stdout().flush().unwrap();
                shutdown.await;
                println!("DRAINING");
                std::io::stdout().flush().unwrap();
                // Synthetic admitted work stays held until the parent releases it.
                let mut finish = String::new();
                std::io::stdin().read_line(&mut finish).unwrap();
                assert_eq!(finish, "finish\n");
                println!("DRAINED");
            });
    }
}
