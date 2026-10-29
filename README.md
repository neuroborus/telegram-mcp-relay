# Telegram MCP Relay

A planned lightweight Rust service connecting several agent sessions to one Telegram bot
through a shared local MCP endpoint. Replies to text and documents return to the original
session; a durable inbox supports subscriptions and an optional bounded wait.

This checkout contains the project specification, repository conventions, Agent Runner
workflow and an initial Rust application scaffold. The binary initializes structured
diagnostics on a current-thread Tokio runtime, logs its scaffold status and exits. The MCP
endpoint, Telegram client, storage and runnable relay setup are not implemented yet.
The library also implements validated [configuration and protected secrets](docs/CONFIGURATION.md);
the binary does not load configuration yet.
The specification calls the product **Telegram MCP Bridge**; this repository and binary are
named **telegram-mcp-relay**.

The manifest requires Rust 1.89 or newer. Build the scaffold using its complete dependency lock:

```bash
cargo build --locked --release --bin telegram-mcp-relay
```

The resulting `target/release/telegram-mcp-relay` currently logs its scaffold status and exits.
See [architecture](docs/ARCHITECTURE.md) for dependency choices and current compatibility limits.

The MVP targets one native process, loopback Streamable HTTP, Telegram long polling and SQLite.
Outgoing text and captions have a strict 1,000 Unicode scalar limit including the session
header; longer reports travel as files. Notifications cannot guarantee that an idle agent
host starts a new turn. Unconsumed Telegram updates last at most 24 hours, independently of
the bridge's default 30-day local retention.

Start with the [technical specification](docs/product/TECHNICAL_SPECIFICATION.md), then the
[documentation map](docs/README.md). Contributors and agents should read [AGENTS.md](AGENTS.md),
[conventions](docs/CONVENTIONS.md) and the [development workflow](docs/DEVELOPMENT.md).
