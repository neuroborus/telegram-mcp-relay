---
name: rust-service
description: Implement or review the Relay Rust daemon, MCP/Telegram boundaries, SQLite persistence and bounded asynchronous work.
---

# Rust Service

Read [conventions](../../../docs/CONVENTIONS.md), [architecture](../../../docs/ARCHITECTURE.md)
and the owning sections of the [specification](../../../docs/product/TECHNICAL_SPECIFICATION.md).
Product code is authored through [delegation](../delegation/SKILL.md), not a standalone fallback.

Keep one process, one shared HTTP client, one Telegram poller and bounded work. Start with
Tokio's current-thread runtime; measure before adding concurrency or a SQLite worker. Never
hold blocking database guards across `.await` or buffer whole uploads. Verify current official
SDK/API versions and choose minimal crate features when introducing dependencies.

Validate meaningful titles, project labels, strict Markdown and body-relative entities
before effects. Preserve the explicit plain and legacy entity paths. Count title graphemes,
final Unicode scalars and caption UTF-16 units separately. Shift body-relative UTF-16 offsets
past Relay's bold `[title]`, italic project line and separating blank line. Service notices
begin with `🔧 `. Display metadata grants no file or routing authority.

Commit authorized observations, routed replies, provenance and cursor atomically before
signaling subscribers or waiters. Keep one claim fence for all consumers. Consume the visible
start acknowledgment once with the first fenced processing transition. Retain completion
acknowledgment as a distinct operation.

Ask, opt-in send waiting and post-send waiting share target-authorized claims. Arm interest
before querying durable state. Preserve late replies and known outbound IDs for recovery
after timeout or cancellation. Shared history is opted-in observation and grants no claim
authority over another session. Download only from retained authorized opaque handles through
the fixed cloud origin or an opt-in operator-managed local Bot API. Confine local paths to
approved upstream storage and publish a copy into private pinned owned storage. Bound streams
and reservations and clean up only owned files. Keep the concise server instructions,
`telegram_guidance` and tool descriptions aligned with the
[agent operating policy](../../../docs/AGENT_POLICY.md) and actual schemas and capabilities.
Exclude private settings, protocol adapters and promises about host wake or concurrency.

Return typed, structured errors with stable safe codes; preserve MCP protocol errors. Use
redacted tracing and safe request correlation. Enforce loopback Host/Origin policy, explicit
Telegram/file allowlists and safe file opening. Network send uncertainty is not proof of
failure and does not authorize duplicate automatic sends.

Use [test-authoring](../test-authoring/SKILL.md), run [required checks](../../../docs/TESTING.md)
and finish with [finalization](../finalization/SKILL.md). Update architecture with actual
implemented guarantees and compatibility evidence.
