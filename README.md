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
processing/acknowledgment, modern resource subscriptions, bounded asks and explicit after-send
waits. Replies and the update cursor commit atomically; post-commit signals serve subscribers
and waiters without
consuming durable replies. Bounded periodic cleanup and incremental disk reclamation preserve
valid replies and recovery identities. See [architecture](docs/ARCHITECTURE.md) for implemented
guarantees and protocol limitations.

SIGINT and SIGTERM trigger graceful shutdown, draining admitted sends and retiring service work.
See the [setup guide](docs/SETUP.md) and [copyable agent policy](docs/AGENT_POLICY.md).
The [recorded Linux x86_64 mocked capture](docs/MEASUREMENTS.md#recorded-host-capture) measured
10.07 MB probe idle RSS and a 10.43 MB sampled upload peak for a 49,999,999-byte streamed file.
These are library-composition figures with separate mock/driver accounting. HOST user-unit
verification initially failed because its executable was absent, then
[passed after authorized binary installation](docs/MEASUREMENTS.md#unit-verification-and-remaining-acceptance).
No service unit was installed, enabled or started. Synthetic results and static unit validation
do not establish live Telegram or named-client interoperability.
The specification calls the product **Telegram MCP Bridge**; this repository and binary are
named **telegram-mcp-relay**.

For Linux (kernel 5.6+ for confined file opening), install Rust 1.89 or newer and a native
C build toolchain for bundled SQLite. Build using the committed dependency lock:

```bash
cargo build --locked --release --bin telegram-mcp-relay
```

Follow [setup](docs/SETUP.md) to create a BotFather bot, verify webhook state, select explicit
chat/user IDs and export roots, and prepare private storage and secrets from the
[synthetic configuration](config/relay.example.toml). Then run one shared daemon:

```bash
target/release/telegram-mcp-relay "$HOME/.config/telegram-mcp-relay/config.toml"
```

Connect capable clients to `http://127.0.0.1:8765/mcp` using Streamable HTTP and the configured
bearer header. MCP **2025-06-18** supports initialization, tools and read-only inbox
resources on the same endpoint as **2026-07-28** discovery and `subscriptions/listen`.
Older resource subscriptions are unavailable. Named-client interoperability remains pending
HOST verification; see [client requirements](docs/SETUP.md#connect-mcp-clients).
Optional [systemd user-service and environment examples](docs/SETUP.md#optional-systemd-user-service)
are provided for operator review.

`telegram_send` and `telegram_send_file` return after acceptance and route commit; they do not
arm waiting. Save their `outbound_message_id`, then call
`telegram_wait({session_key, outbound_message_id})` without sending again. Waiting defaults
to 600 seconds and returns `reply_received`, `wait_timeout` or `disabled`, with a nullable
`reply`. A returned claim still requires current-token processing and explicit acknowledgment.
Unknown, foreign or expired targets return `WAIT_TARGET_INVALID`. Timeout/cancellation leave
routes and replies recoverable and grant no permission. See [client deadlines and concurrency](docs/SETUP.md#connect-mcp-clients).

The bounded history/attachment storage foundation is present; automatic capture and public
history/download tools remain pending. History defaults off and requires authenticated opt-in.
Read the [stored-body privacy and limits](docs/CONFIGURATION.md#opt-in-history-storage) before enabling it.

Startup failures return a safe error code and nonzero exit status. No directories, services or
webhook resets are installed automatically. The binary registers shutdown signals before starting services
and awaits accepted send bookkeeping before exit. Library hosts supply cancellation and await
`run_until`; the library installs no process-wide signal handlers. SIGKILL, crashes and external
forced-stop deadlines can still interrupt bookkeeping; see
[shutdown ownership](docs/ARCHITECTURE.md#runtime-and-ownership).

The MVP uses one native process, loopback Streamable HTTP, Telegram long polling and SQLite.
Outgoing text and captions have a strict 1,000 Unicode scalar limit including the session
header, optional directory, separators and body; captions also fit 1,024 UTF-16 units.
The server renders a native bold title and optional italic `directory_label`; callers supply
literal bodies without manually adding headers. Directory context is opt-in display metadata
(up to 100 normalized scalars), never file authorization. Every service notice begins `🔧 `.
Longer reports travel as explicitly prepared files. Local paths refer to the daemon's filesystem
beneath approved roots. Incoming attachment metadata is supported; attachment downloading is deferred.
The shared bearer assumes trusted local clients. Notifications cannot guarantee that an idle agent
host starts a new turn. Unconsumed Telegram updates last at most 24 hours, independently of
the bridge's default 30-day local retention.

Start with the [technical specification](docs/product/TECHNICAL_SPECIFICATION.md), then the
[documentation map](docs/README.md). Contributors and agents should read [AGENTS.md](AGENTS.md),
[conventions](docs/CONVENTIONS.md) and the [development workflow](docs/DEVELOPMENT.md).
