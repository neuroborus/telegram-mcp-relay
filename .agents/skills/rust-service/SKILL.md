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
and the native caption UTF-16 bound. Native entity offsets also use UTF-16; preserve literal
bodies and server-owned bold title/optional italic directory/service `🔧 ` prefix. Display
metadata never authorizes files or routing. Validate complete visible output before effects.
Persist authorized observations, routed replies/provenance and cursor atomically before
signaling both subscribers and waiters. All reply-claiming consumers share one atomic fence;
processing start never substitutes for completion acknowledgment.

Ask and post-send/file wait share bounded target-authorized claims without resending. Arm
interest before querying durable state; timeout/cancellation preserves late replies and grants
no permission. Shared history is opted-in observation, never another session's claim authority.
Download only from retained authorized opaque handles through the fixed cloud origin into
private pinned owned storage, with bounded streams/reservations and owned-only cleanup.
Keep `telegram_guidance` and tool descriptions aligned with actual schemas/capabilities, without
private settings, protocol adapters or host wake/concurrency promises.

Return typed, structured errors with stable safe codes; preserve MCP protocol errors. Use
redacted tracing and safe request correlation. Enforce loopback Host/Origin policy, explicit
Telegram/file allowlists and safe file opening. Network send uncertainty is not proof of
failure and does not authorize duplicate automatic sends.

Use [test-authoring](../test-authoring/SKILL.md), run [required checks](../../../docs/TESTING.md)
and finish with [finalization](../finalization/SKILL.md). Update architecture with actual
implemented guarantees and compatibility evidence.
