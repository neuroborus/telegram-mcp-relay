# Telegram MCP Relay

A lightweight Rust service connecting several agent sessions to one Telegram bot
through a shared local MCP endpoint. Replies to text and documents return to the original
session; a durable inbox supports subscriptions and an optional bounded wait.

The binary now runs the [shared daemon composition](docs/ARCHITECTURE.md#runtime-and-ownership)
on a current-thread Tokio runtime. It loads validated
[configuration and protected secrets](docs/CONFIGURATION.md), opens private SQLite/export roots,
completes startup retention cleanup and serves one protected loopback MCP endpoint alongside
one Telegram poller, factual notices and shared confirmed-processing typing.

MCP provides typed registration, text/document sends, durable inbox resources, fenced reply
processing/acknowledgment, modern resource subscriptions and bounded asks. Replies and the
update cursor commit atomically; post-commit signals serve subscribers and waiters without
consuming durable replies. Bounded periodic cleanup and incremental disk reclamation preserve
valid replies and recovery identities. See [architecture](docs/ARCHITECTURE.md) for implemented
guarantees and protocol limitations.

Graceful accepted-send draining and OS signal integration remain the next shutdown step.
Setup guidance, full acceptance interactions and resource measurements remain planned work;
this composition does not establish complete MVP acceptance or live Telegram interoperability.
The specification calls the product **Telegram MCP Bridge**; this repository and binary are
named **telegram-mcp-relay**.

The manifest requires Rust 1.89 or newer. Build using its complete dependency lock:

```bash
cargo build --locked --release --bin telegram-mcp-relay
```

Run `target/release/telegram-mcp-relay /absolute/path/to/config.toml` with an explicitly prepared
configuration, protected secrets and existing private storage/export directories. The
[synthetic example](config/relay.example.toml) needs local IDs and paths before use. Startup
failures return a safe error code and nonzero exit status. No directories, services or webhook
resets are installed automatically. Default process termination can interrupt send bookkeeping;
library hosts can cancel service loops through `run_until`.

The MVP uses one native process, loopback Streamable HTTP, Telegram long polling and SQLite.
Outgoing text and captions have a strict 1,000 Unicode scalar limit including the session
header; longer reports travel as files. Notifications cannot guarantee that an idle agent
host starts a new turn. Unconsumed Telegram updates last at most 24 hours, independently of
the bridge's default 30-day local retention.

Start with the [technical specification](docs/product/TECHNICAL_SPECIFICATION.md), then the
[documentation map](docs/README.md). Contributors and agents should read [AGENTS.md](AGENTS.md),
[conventions](docs/CONVENTIONS.md) and the [development workflow](docs/DEVELOPMENT.md).
