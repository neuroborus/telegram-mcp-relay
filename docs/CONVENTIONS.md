# Repository Conventions

## Current and intended structure

One application crate at the repository root provides the initial runtime scaffold, with a
complete `Cargo.lock`. Keep the lockfile with manifest changes and use locked builds/checks.
Use `telegram-mcp-relay` for the package/binary and `telegram_mcp_relay` for its Rust library.
Keep `main.rs` to bootstrap and shutdown; `lib.rs` exposes the shared application entry point.
The dependency-free development helper at `tools/runner-check/` has its own explicit workspace
and lockfile. Root workspace checks do not cover it; see [testing](TESTING.md).

| Current or intended path | Responsibility |
| --- | --- |
| `src/main.rs`, `src/lib.rs` | Binary bootstrap and testable library surface |
| `src/app.rs`, `src/state.rs` | Router/runtime composition and shared handles |
| `src/config.rs`, `src/error.rs` | Validated configuration and typed application errors |
| `src/dto.rs` | Typed MCP tool requests/results and their SDK-generated schemas |
| `src/handlers.rs`, `src/handlers/`, `src/routes.rs`, `src/routes/` | Shared SDK handler state, thin typed tools/read-only inbox resources, bounded modern subscriptions, protected HTTP transport and its tests |
| `src/domain/` | Session/title/text rules, reply state and retention invariants |
| `src/services.rs`, `src/services/` | Text/document delivery, bounded question waits, confined outbound file handles, authorized reply ingestion, one restart-safe poller, bounded inbox claims and durable processing/completion |
| `src/repositories.rs`, `src/repositories/` | SQLite initialization/migrations, transactions, queries and bounded post-commit inbox interests |
| `src/clients/` | Direct Telegram HTTP client and transport DTOs |
| `tests/`, `tests/fixtures/` | Cross-module/MCP/mock-Telegram tests and sanitized inputs |
| `config/` | Nonsecret example configuration |
| `deploy/` | Example `systemd --user` unit and protected secret-file guidance |
| `tools/` | Small justified development/measurement helpers |

These are ownership slots, not a demand to create empty modules. Collapse trivial layers into
focused files until separation improves clarity. MCP SDK handlers may live together when the
SDK owns route wiring; do not duplicate an HTTP server to imitate this tree.

## Rust and boundaries

- Use conventional `snake_case` modules/functions, `PascalCase` types and explicit error enums.
- Separate domain values from protocol/database representations where their invariants differ.
  Keep session identity distinct from MCP transport identity and title labels.
- Dependencies flow from transport/adapters into services and domain. Domain logic must not
  require HTTP, SQLite or an agent vendor. Add traits only for real effect boundaries needed
  by tests; avoid speculative plugin/repository frameworks.
- Keep Axum handlers thin and compose the router centrally. Carry `x-request-id` correlation
  where applicable without trusting arbitrary sensitive values for logging. Preserve protocol
  errors and MCP structured tool errors rather than wrapping everything in an HTTP error.
- Use `AppError` or equivalent typed application error mapping at boundaries. Stable codes and
  safe messages belong in client results; private causes belong only in redacted diagnostics.
- No `unwrap`/`expect` on untrusted/runtime input. Explain exceptional invariant assertions.
- Prefer a Tokio current-thread runtime, one reusable HTTP client and bounded tasks/buffers.
  Do not hold synchronous database locks across `.await`; measure SQLite impact before adding
  a dedicated bounded worker. Avoid loading whole files or histories into RAM.
- Default to plain Telegram text, loopback binding and explicit allowlists. Configuration is
  TOML plus environment secrets, validated once at startup. Never commit actual bot tokens.

Verify exact versions/features against official upstream sources when the Cargo manifest is
introduced. Prefer `rmcp`, Tokio, Axum/Tower, reqwest with rustls, rusqlite, serde and tracing
where needed. Do not inherit OpenAPI/Swagger, broad CORS, compression or a multi-thread runtime
merely because they occur in a service template.

## Tests, documents and local state

Put pure unit tests beside their owning module and interaction/restart tests under `tests/`.
Keep clocks/effects injectable where necessary; use synthetic chats, users and paths.
See [testing](TESTING.md) and [test-authoring](../.agents/skills/test-authoring/SKILL.md).

Write repository documentation in English. Keep current guarantees in owning documents and
index new durable documents in [docs/README.md](README.md). Use relative links and named
symbols/sections rather than line-number citations that drift after edits.

`LOCAL_ARTIFACTS/` is ignored, untracked and machine local. Runner state itself must live
outside the repository and task directory. Keep local config/task/plan/report files here;
never commit or automatically clean them. Build output, secret files and runtime SQLite files
are also ignored. Application configuration examples must contain no real IDs or credentials.

## Reference projects

The structure borrows thin handlers, central configuration/errors and tracing from
[axum-template](https://github.com/neuroborus/axum-template); documentation ownership,
canonical skills and explicit finalization from
[ward-pulse](https://github.com/neuroborus/ward-pulse); and reviewed plan/execution boundaries
from [agent-runner](https://github.com/neuroborus/agent-runner).
Their unrelated product rules, platform checks and local environment assumptions do not apply.
