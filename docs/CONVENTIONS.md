# Repository Conventions

## Current structure

One application crate at the repository root provides the native relay daemon and shared
library, with a complete `Cargo.lock`. Keep the lockfile with manifest changes and use locked
builds/checks.
Use `telegram-mcp-relay` for the package/binary and `telegram_mcp_relay` for its Rust library.
Keep `main.rs` to bootstrap and shutdown; `lib.rs` exposes the shared application entry point.
The dependency-free development helper at `tools/runner-check/` has its own explicit workspace
and lockfile. Root workspace checks do not cover it; see [testing](TESTING.md).

| Path | Responsibility |
| --- | --- |
| `src/main.rs`, `src/lib.rs` | Binary bootstrap and testable library surface |
| `src/app.rs`, `src/app/` | Daemon composition, startup, supervision/shutdown and full-composition tests |
| `src/app/probe.rs`, `tools/resource_probe.rs` | Credential-free loopback probe construction and explicit library-linked release example |
| `src/config.rs`, `src/config/`, `src/error.rs` | Validated configuration, protected secrets and typed application errors |
| `src/dto.rs` | Typed tool requests/results, inbox resource snapshots and SDK-generated schemas |
| `src/handlers.rs`, `src/handlers/`, `src/routes.rs`, `src/routes/` | Shared SDK handler state, thin typed tools/read-only inbox resources, bounded modern subscriptions, protected HTTP transport and its tests |
| `src/domain.rs`, `src/domain/` | Pure title normalization, final outbound Unicode rules and their tests |
| `src/services.rs`, `src/services/` | Text/document delivery, bounded question waits, confined outbound file handles, authorized reply ingestion, one restart-safe poller, bounded inbox claims, durable processing/completion, shared confirmed-processing typing, factual notice dispatch and bounded retention scheduling |
| `src/repositories.rs`, `src/repositories/` | SQLite initialization/migrations, transactions, queries and bounded post-commit inbox interests |
| `src/clients.rs`, `src/clients/` | Bounded direct Telegram HTTP, streamed multipart uploads, transport DTOs and synthetic tests |
| `tests/` | Cross-module storage, ingress, claim/recovery, notice, retention and maintenance tests |
| `config/` | Nonsecret example configuration |
| `deploy/` | Example `systemd --user` unit and protected secret-file guidance |
| `tools/` | Small justified development/measurement helpers |
| `tools/measure_resources.py`, `tools/measure_collectors.py`, `tools/measure_mock.py`, `tools/measure_driver.py`, `tools/tests/test_measure*.py` | HOST measurement controller, bounded Linux collectors, separate synthetic loopback Bot API and MCP driver processes, and deterministic Python tests |

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
- Keep MCP handlers thin and compose the SDK/Hyper HTTP endpoint centrally. Generate safe
  request correlation without trusting client-supplied identifiers or sensitive metadata.
  Preserve protocol errors and MCP structured tool errors rather than wrapping everything
  in an HTTP error.
- Use `AppError` or equivalent typed application error mapping at boundaries. Stable codes and
  safe messages belong in client results; private causes belong only in redacted diagnostics.
- No `unwrap`/`expect` on untrusted/runtime input. Explain exceptional invariant assertions.
- Prefer a Tokio current-thread runtime, one reusable HTTP client and bounded tasks/buffers.
  Do not hold synchronous database locks across `.await`; measure SQLite impact before adding
  a dedicated bounded worker. Avoid loading whole files or histories into RAM.
- Default to plain Telegram text, loopback binding and explicit allowlists. Configuration is
  TOML plus environment secrets, validated once at startup. Never commit actual bot tokens.

Verify exact versions/features against official upstream sources when the Cargo manifest is
introduced. Prefer `rmcp`, Tokio, the existing Hyper/Tower transport integration, reqwest with
rustls, rusqlite, serde and tracing where needed. Do not inherit OpenAPI/Swagger, broad CORS,
compression or a multi-thread runtime
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
