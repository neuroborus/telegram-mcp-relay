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

Protect distinct Unicode contracts: title graphemes, final outgoing Unicode scalar count,
and the native caption UTF-16 bound. Validate before effects. Persist reply plus cursor before
signaling both subscribers and waiters. All consumers use the same atomic claim/fencing path;
processing start never substitutes for completion acknowledgment.

Return typed, structured errors with stable safe codes; preserve MCP protocol errors. Use
redacted tracing and safe request correlation. Enforce loopback Host/Origin policy, explicit
Telegram/file allowlists and safe file opening. Network send uncertainty is not proof of
failure and does not authorize duplicate automatic sends.

Use [test-authoring](../test-authoring/SKILL.md), run [required checks](../../../docs/TESTING.md)
and finish with [finalization](../finalization/SKILL.md). Update architecture with actual
implemented guarantees and compatibility evidence.
