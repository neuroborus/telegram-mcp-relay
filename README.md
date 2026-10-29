# Telegram MCP Relay

A planned lightweight Rust service connecting several agent sessions to one Telegram bot
through a shared local MCP endpoint. Replies to text and documents return to the original
session; a durable inbox supports subscriptions and an optional bounded wait.

This checkout currently contains the project specification, repository conventions and
Agent Runner workflow. The daemon, Cargo manifest and runnable setup are not implemented yet.
The specification calls the product **Telegram MCP Bridge**; this repository and intended
binary are named **telegram-mcp-relay**.

The MVP targets one native process, loopback Streamable HTTP, Telegram long polling and SQLite.
Outgoing text and captions have a strict 1,000 Unicode scalar limit including the session
header; longer reports travel as files. Notifications cannot guarantee that an idle agent
host starts a new turn. Unconsumed Telegram updates last at most 24 hours, independently of
the bridge's default 30-day local retention.

Start with the [technical specification](docs/product/TECHNICAL_SPECIFICATION.md), then the
[documentation map](docs/README.md). Contributors and agents should read [AGENTS.md](AGENTS.md),
[conventions](docs/CONVENTIONS.md) and the [development workflow](docs/DEVELOPMENT.md).
