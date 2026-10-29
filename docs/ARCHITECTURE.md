# Architecture

## Current status

The root Rust crate contains a small binary and a shared library entry point. The binary uses
Tokio's current-thread runtime and initializes structured `tracing` diagnostics on stderr.
The library logs the scaffold status and returns without opening sockets or creating files.
No database schema, Telegram client or MCP transport is implemented yet.

The library now provides centralized validated TOML settings and typed, redacted startup
errors in `config`/`error`, with protected Unix secret resolution under `config::secrets`.
Parsing has no side effects; secret loading is explicit. The binary remains a scaffold;
production startup composition is a later step. See [configuration](CONFIGURATION.md).

Pure title normalization and outbound formatting now live in `domain::outbound`, with typed
length errors in `error`. These functions perform no network or database effects. Services
will use them before sends and route mutations; those integrations are not implemented yet.

The complete dependency resolution lives in `Cargo.lock`. The release profile enables thin
LTO, one codegen unit and stripping. Add configuration, persistence, HTTP and test dependencies
only as their implementations land.

The following boundaries are the intended MVP design from
[the specification](product/TECHNICAL_SPECIFICATION.md), especially sections 3–13.
Replace intended claims with verified implementation details as each component lands; record
the actual tested SDK/protocol versions and any compatibility gaps here.

## Dependency baseline

The 2026-10-29 dependency review used cached published crates.io packages and registry metadata.
Archive checksums matched the registry, and the inspected extracted sources matched those
archives. No live upstream fetch or application protocol compatibility test was performed.

| Dependency | Selected version | Direct features (defaults disabled) | Purpose |
| --- | --- | --- | --- |
| [rmcp](https://docs.rs/crate/rmcp/3.8.0/features) | 3.8.0 | `server`, `transport-streamable-http-server` | Official MCP server, schema support and shared HTTP transport foundation |
| [Tokio](https://docs.rs/crate/tokio/1.54.1/features) | 1.54.1 | `macros`, `rt` | Current-thread async binary entry point |
| [tracing](https://docs.rs/crate/tracing/0.1.44/features) | 0.1.44 | `std` | Structured diagnostic events |
| [tracing-subscriber](https://docs.rs/crate/tracing-subscriber/0.3.23/features) | 0.3.23 | `fmt`, `std` | Fallible stderr diagnostic initialization with an INFO ceiling |
| [serde](https://docs.rs/crate/serde/1.0.229/features) | 1.0.229 | `derive`, `std` | Typed settings deserialization; already in the SDK graph |
| [toml](https://docs.rs/crate/toml/1.1.3/features) | 1.1.3+spec-1.1.0 | `parse`, `serde`, `std` | TOML input without display/serialization or ordering features |
| [rustix](https://docs.rs/crate/rustix/1.1.4/features) | 1.1.4 | `std`, `fs`, `process` | Safe descriptor opening and effective-owner checks without application unsafe code |
| [getrandom](https://docs.rs/crate/getrandom/0.4.3/features) | 0.4.3 | none | OS randomness for bearer-file creation; already in the SDK graph |
| [unicode-segmentation](https://docs.rs/crate/unicode-segmentation/1.13.3/features) | 1.13.3 | none | Unicode extended grapheme boundaries for title truncation, without runtime dependencies |

Configuration dependency versions/features were reviewed from cached published registry metadata.
Live DNS and new crate archives were unavailable for API/compilation verification in that turn.

The title dependency's version, checksum, features and Rust 1.85 minimum were selected from
cached published registry metadata. Its cached archive now matches the checksum in
`Cargo.lock`; source inspection confirms `UnicodeSegmentation::grapheme_indices(true)`,
the declared minimum and the absence of runtime dependencies. Initial live docs.rs and
Telegram Bot API checks failed at DNS resolution; an upstream limits recheck remains pending.
Native bounds below follow specification §9.3 with conservative UTF-16 counting; no live
Telegram behavior has been tested.

Feature unification also enables Tokio I/O utilities, synchronization and time, and tracing
attributes through the SDK. It does not enable Tokio's multi-thread runtime or the SDK's
client, OAuth, tool macros or child-process transport. The diagnostics formatter omits ANSI,
JSON, environment filters and the log adapter. The locked graph requires Rust 1.89 because
of `uuid` 1.29.0; rmcp itself declares Rust 1.88. The minimum toolchain is not separately tested.

The rmcp source defines `ProtocolVersion::V_2026_07_28`, `server/discover` and
`subscriptions/listen`. `ServerHandler::accepted_subscription_filter` and `listen` accept
subscription interests; `SubscriptionSink::send` enforces the accepted resource URI filter.
Modern `StreamableHttpService` dispatch creates a handler for each request, so later
application state must be shared through its factory. These are verified SDK APIs, not
implemented relay capabilities. No client/version interoperability is claimed yet.

## Outbound formatting

`SessionTitle` collapses Unicode whitespace to one ASCII space, trims edges, removes other
Unicode control scalars and truncates at 50 extended grapheme clusters. Blank titles become
`Agent Session`. Emoji joiners and combining sequences remain intact. Titles are display
labels; their debug representation is redacted.

`format_text` handles messages/questions, `format_caption` handles documents and
`format_notice` handles notices with an optional known-session title. All share the plain
`[Title]` header with one newline before a nonempty body; an empty description leaves only
the header. Bodies remain verbatim, with no markup parsing, splitting or truncation. An
unprefixed empty notice is rejected with `TEXT_EMPTY` to respect Telegram's nonempty text bound.

Before allocating the final output, these functions count the normalized title, brackets,
separator and full body. They enforce the fixed 1,000 Unicode scalar cap and conservatively
check native UTF-16 bounds of 4,096 for text/notices and 1,024 for captions. A very large
single title grapheme can therefore cause rejection even when the body is empty.

`TEXT_TOO_LONG` carries serializable details: `limit=1000`, `actual_characters`,
`native_limit_utf16`, `actual_utf16_units` and file-delivery `guidance`. Native caption
rejections retain the actual scalar count even when it is within 1,000. Error details and
diagnostics contain counts and static guidance, never the supplied title or body. Callers
must prepare the file themselves; this layer performs no send or route registration.

## Runtime and ownership

One Rust binary serves concurrent MCP clients at a loopback Streamable HTTP endpoint and
owns one Telegram long-poll consumer, one reusable HTTP client and one embedded SQLite store.
Use the official MCP Rust SDK rather than inventing JSON-RPC or maintaining parallel HTTP stacks.
Subscriptions and bounded waits are complementary delivery mechanisms. Do not introduce
vendor-specific wake adapters, stdio process-per-client deployment or agent lifecycle control.

Services own registration, send/upload, durable ingress, reply claims and acknowledgment.
Domain rules own normalized titles, Unicode limits, identity, state transitions and expiry.
The SQLite repository owns atomicity and fencing; Telegram and MCP adapters own serialization,
network/protocol behavior and cancellation. Shared state contains handles, not duplicate history.

## Durable delivery invariants

- An agent session is an explicit persisted identity, independent of transport connections.
  Titles are display labels, normalized and limited to 50 grapheme clusters.
- Validate final outgoing text/captions at 1,000 Unicode scalars including the header before
  any send/route mutation. Captions also obey the conservative 1,024 UTF-16-unit native bound.
  Overlength returns `TEXT_TOO_LONG`; no splitting, truncation or implicit file creation.
- Persist `(chat_id, message_id) → session_key` for successful text and document sends. Never
  route by title or last activity. Lost send responses yield uncertainty rather than blind retry.
- Accept only approved sender/chat pairs. Commit the accepted Reply and Telegram cursor
  atomically before confirming a later polling offset, then notify subscribers and waiters.
- Resource reads are read-only. Claiming `telegram_inbox` calls and wait results share one
  atomic claim path. Rotate fencing tokens on new claims, reject stale tokens, and require
  explicit completion acknowledgment. Beginning
  processing is not completion; disconnection must not delete accepted replies.
- Emit subscribed resource notifications even while a waiter is armed. Reads recover durable
  state after reconnect; notification delivery is not a replay log or an LLM wake guarantee.
- Start typing only after confirmed processing, cap refresh at ten minutes and keep idle free
  of typing activity. Warn once after the grace period only if processing never started.
- Bound task counts, timers, parser sizes, upload buffers and inbox capacity. Stream files
  through approved roots with safe opening; never persist outbound file contents or chat bodies.

## Retention and security

Routes expire 30 days after their original message, sessions after last activity and unacked
replies 30 days after receipt by default. Active session traffic does not extend old routes.
Cleanup must preserve still-valid replies and referential integrity, work in bounded batches
and account for actual disk reclamation. Capacity pressure cannot silently evict valid replies.
Telegram keeps unconsumed updates for at most 24 hours; the bridge cannot reconstruct a longer
offline gap from conversation history.

Bind loopback by default; validate Host/Origin against DNS rebinding and support a protected
bearer token. Require explicit Telegram allowlists and file roots, redact secrets and treat
incoming text/files as untrusted data. Opaque session keys and claim tokens reduce accidental
cross-session consumption; a shared local bearer token is not strong isolation between
mutually hostile local clients. Document that trust assumption in installed-user guidance.

## Protocol compatibility gate

The supplied specification requests MCP `2026-07-28` semantics and compatible older clients.
Verify official `rmcp` capabilities and current API behavior at implementation time. Record the actual
negotiated version, supported subscription mechanisms and tested clients; explicitly document
any unavailable upstream feature. Do not fabricate protocol support or silently replace
event delivery with agent polling. See specification sections 5–6 and 18 for the source links.
