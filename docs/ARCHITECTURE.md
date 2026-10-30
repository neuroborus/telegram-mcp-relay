# Architecture

## Current status

The root Rust crate contains a small binary and a shared library entry point. The binary uses
Tokio's current-thread runtime and initializes structured `tracing` diagnostics on stderr.
The library logs the scaffold status and returns without opening sockets or creating files.
The library provides explicit SQLite initialization and session/route operations, but the
binary does not open it.
The library provides a bounded Telegram HTTP client and text/document delivery with durable
reply routes. No MCP transport or production relay wiring exists yet. The accepted HTTP
adapter's synthetic tests passed in Runner's complete workspace test suite,
as described below.
Library reply ingestion authorizes input and commits replies/dispositions with the cursor.
A cancellable single-consumer long poller now drives it; production startup and MCP
notification delivery remain later work.

The library now provides centralized validated TOML settings and typed, redacted startup
errors in `config`/`error`, with protected Unix secret resolution under `config::secrets`.
Parsing has no side effects; secret loading is explicit. The binary remains a scaffold;
production startup composition is a later step. See [configuration](CONFIGURATION.md).

Pure title normalization and outbound formatting now live in `domain::outbound`, with typed
length errors in `error`. These functions perform no network or database effects. Text delivery
and document delivery use them before admission and sends.

The complete dependency resolution lives in `Cargo.lock`. The release profile enables thin
LTO, one codegen unit and stripping.
Add dependencies only as their implementations land.

The following boundaries are the intended MVP design from
[the specification](product/TECHNICAL_SPECIFICATION.md), especially sections 3–13.
Replace intended claims with verified implementation details as each component lands; record
the actual tested SDK/protocol versions and any compatibility gaps here.

## Dependency baseline

The 2026-10-29 dependency review used cached published crates.io packages and registry metadata.
Archive checksums matched the registry, and the inspected extracted sources matched those
archives. Telegram documentation was reviewed from a saved official API reference; no live
Telegram requests or MCP interoperability tests have been performed.

| Dependency | Selected version | Direct features (defaults disabled) | Purpose |
| --- | --- | --- | --- |
| [rmcp](https://docs.rs/crate/rmcp/3.8.0/features) | 3.8.0 | `server`, `transport-streamable-http-server` | Official MCP server, schema support and shared HTTP transport foundation |
| [Tokio](https://docs.rs/crate/tokio/1.54.1/features) | 1.54.1 | `fs`, `io-util`, `macros`, `rt`, `sync`, `time` | Current-thread entry point, bounded file reads/admission and client deadlines/backoff |
| [tracing](https://docs.rs/crate/tracing/0.1.44/features) | 0.1.44 | `std` | Structured diagnostic events |
| [tracing-subscriber](https://docs.rs/crate/tracing-subscriber/0.3.23/features) | 0.3.23 | `fmt`, `std` | Fallible stderr diagnostic initialization with an INFO ceiling |
| [serde](https://docs.rs/crate/serde/1.0.229/features) | 1.0.229 | `derive`, `std` | Typed settings deserialization; already in the SDK graph |
| [toml](https://docs.rs/crate/toml/1.1.3/features) | 1.1.3+spec-1.1.0 | `parse`, `serde`, `std` | TOML input without display/serialization or ordering features |
| [rustix](https://docs.rs/crate/rustix/1.1.4/features) | 1.1.4 | `std`, `fs`, `process` | Safe descriptor opening and effective-owner checks without application unsafe code |
| [getrandom](https://docs.rs/crate/getrandom/0.4.3/features) | 0.4.3 | none | OS randomness for bearer-file creation; already in the SDK graph |
| [unicode-segmentation](https://docs.rs/crate/unicode-segmentation/1.13.3/features) | 1.13.3 | none | Unicode extended grapheme boundaries for title truncation, without runtime dependencies |
| [rusqlite](https://docs.rs/crate/rusqlite/0.32.1/features) | 0.32.1 | `bundled` | Embedded SQLite with fixed source/bindings, strict tables and JSON validation |
| [reqwest](https://docs.rs/crate/reqwest/0.13.5/features) | 0.13.5 | `json`, `multipart`, `rustls`, `stream` | Reusable direct Telegram HTTPS client with streamed multipart uploads, without default features |
| [serde_json](https://docs.rs/crate/serde_json/1.0.154/features) | 1.0.154 | `std` | Bounded Bot API envelope decoding; already locked through rmcp |
| [futures-util](https://docs.rs/crate/futures-util/0.3.34/features) | 0.3.34 | `std` | Lazy fallible file-chunk stream; already in the locked SDK graph |

Dev-only Tokio `net` and `test-util` support synthetic HTTP peers and
controlled deadlines. Direct `sync` use supplies owned send-admission permits; it was already
enabled in the locked SDK graph and adds no packages or lockfile changes.
The reqwest archive checksum matches published registry metadata;
its inspected client, chunked-response and retry sources match the archive. Its `json` and
`rustls` features enable JSON, rustls with AWS-LC and platform certificate verification;
default charset, HTTP/2, proxy discovery and compression features are unnecessary. Source
inspection confirms `Response::chunk`, `ClientBuilder::retry`, `retry::never`, `https_only`,
timeouts and redirect/proxy controls. No MCP client feature is added.

Document delivery adds reqwest `multipart`/`stream` and Tokio filesystem/I/O support.
The same checksum-verified reqwest archive confirms `Part::stream_with_length`,
`Body::wrap_stream` and streaming multipart construction. The additional MIME and WASM-stream
lock entries come from that archive's upstream lockfile, preserving existing pinned versions.
Their locally provisioned archives match the committed lockfile checksums. This is source/API
inspection; compilation and required-check evidence remain pending FINALIZE.

Offline lock generation resolved the HTTP/TLS/platform dependency closure without changing
previously locked versions. The manifest and lock additions accompany the adapter to preserve
locked builds.

Configuration dependency versions/features were reviewed from cached published registry metadata.
Live DNS and new crate archives were unavailable for API/compilation verification in that turn.

The title dependency's version, checksum, features and Rust 1.85 minimum were selected from
cached published registry metadata. Its cached archive now matches the checksum in
`Cargo.lock`; source inspection confirms `UnicodeSegmentation::grapheme_indices(true)`,
the declared minimum and the absence of runtime dependencies. Native bounds below match
specification §9.3 and the saved official Telegram API reference, with conservative UTF-16
counting. No live Telegram behavior has been tested.

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

## SQLite initialization

`repositories::Database::open` explicitly opens one synchronous private connection. It requires
an existing effective-user-owned private parent and a regular, single-link, owned mode-0600
database; creation is exclusive and does not repair existing permissions. Descriptor checks
reject leaf/parent symlinks, unsafe existing journal/WAL sidecars and special files before
SQLite opens the path with `SQLITE_OPEN_NOFOLLOW`. Ancestors and same-user access are trusted.
Failures retain only safe typed classifications, without SQLite SQL, paths or row values.

The version 1 baseline uses strict tables for sessions, composite chat/message routes, incoming
replies, the singleton cursor, minimal valid-token acknowledgment receipts and notice attempts.
Only incoming text/captions and attachment metadata have payload columns. No table holds
outbound bodies, full Telegram updates, file bytes or typing leases. A NULL initial cursor
means no update has been processed. Timestamps use Unix milliseconds. Schema version 2 adds
minimal update dispositions, including one compact ignored marker, through a transactional
migration preserving existing records.

Initialization enables incremental auto-vacuum before the schema transaction. DDL, initial
cursor, application ID and version commit together under an immediate transaction; errors
roll back the schema. Foreign/unversioned nonempty databases and unknown versions are rejected.
Foreign keys restrict session deletion while children survive; replies have no route foreign
key, so an expired route can later be removed independently. Indexes cover identity, ordered
inboxes, question matching, notice eligibility and each record's expiry. These are schema
constraints; claiming and cleanup services are not implemented yet.

Connections use DELETE rollback journals, FULL synchronization, a 100 ms busy timeout, a
512 KiB suggested page cache, eight cached statements, disabled mmap and in-memory temporary
work. `max_page_count` limits main-file growth to the configured budget rounded down to pages;
an already oversized database is rejected without eviction. Transient rollback journals and
temporary query memory still require bounded service transactions and physical-use accounting
in the later cleanup/runtime steps. No synchronous database guard may cross an await.

rusqlite 0.32.1 embeds SQLite 3.46.0 through libsqlite3-sys 0.30.1; `bundled` also enables
`modern_sqlite` and pregenerated bindings. Cached published sources match checksum-verified
archives, and the used APIs were inspected. Focused storage tests use this embedded build.
Live registry/SQLite documentation requests failed at DNS resolution; no latest-version claim
or live Telegram behavior is implied by the pinned dependency verification.

## Session and route persistence

`Database::register_session` uses an immediate transaction to register/resume by external ID
or a saved handle and rename future messages. New handles contain 32 OS-random bytes encoded
as 64 hex characters. Identical titles never share identity; absent external ID and handle
means a new session. An unknown supplied handle fails without creating a replacement. When
both identifiers are supplied they must already match. Registration does not touch replies
or processing ownership. Results include the effective title and a stable inbox URI helper.

Registration and successful route bookkeeping refresh session activity/expiry monotonically.
Read-only session and composite chat/message route lookups do not refresh activity. Text and
document routes use the same transaction path; conflicts never overwrite an existing route.
Route expiry is fixed to the original Telegram message creation time, independently of session
activity. Expired routes are excluded at lookup even before cleanup; a retained session remains
resumable while its valid children await later cleanup/recovery.

`reserve_route` admits an in-flight send against route count and logical metadata capacity;
its owned reservation crosses awaits without a database guard and releases capacity on drop.
All runtime admissions must share one `Database`. Registration/rename includes outstanding
reservations in its budget. Retained expired rows still count until cleanup; pressure never
evicts data. Logical session/route charges are 128 bytes per row plus UTF-8 identity/title
bytes (route rows include the session key). SQLite pages have their separate physical cap.
Reservations live only in RAM and do not guarantee post-send disk success: the sending service
reports failed bookkeeping distinctly from an unknown send outcome. Received-Reply activity
updates, cross-component metadata accounting and physical maintenance remain later steps.

## Text delivery

`services::outbound::OutboundService` clones share the application's `Arc<Mutex<Database>>`,
Telegram client and bounded send admission. Construct one service with the configured,
allowlisted destination; tool callers cannot select another chat. It resolves the session
and validates the complete normalized-title text before acquiring a nonwaiting permit from
`limits.background_tasks` and reserving route metadata. Saturation returns `SEND_CAPACITY`;
storage pressure returns `STORAGE_CAPACITY`, without sending or refreshing activity.

After admission, an owned Tokio task attempts the send once and commits its composite route
and session activity through `persist_route`. No database guard crosses an await. Telegram
message dates are converted from seconds to repository milliseconds. Dropping/canceling the
caller leaves this bounded task and reservation alive until the HTTP result and bookkeeping
finish. This protects against caller cancellation, not process loss; supervised daemon
shutdown/draining is a later step. No outbound body is written to SQLite.

A consistent 4xx rejection remains definitive. A lost/invalid response remains
`TELEGRAM_DELIVERY_UNCERTAIN`, with no fabricated route. A confirmed accepted send whose
route transaction fails returns `SEND_BOOKKEEPING_FAILED`, explicitly warning that Telegram
accepted it but reply routing could not be saved. Neither outcome authorizes automatic
resending. Failures from detached work log only a static error code. Successful completion
requires a committed route; reservations and task permits release on completion/failure.
New service tests await execution in the dedicated Runner FINALIZE gate.

## Confined outbound files

`services::files::OutboundFiles` opens up to 64 approved roots once and pins their directory
descriptors. Linux `openat2` with `BENEATH`, `NO_SYMLINKS` and `NO_MAGICLINKS` confines each
lookup in the kernel, including during directory replacement. Root opening also rejects
symlinks in every ancestor. Linux 5.6+ is required; unsupported or denied resolution fails
closed without a canonicalize/open fallback. All symlinks are rejected, including internal
ones. A root pathname replacement cannot redirect the pinned authority to a new directory.

Caller paths must be absolute, without parent traversal, and component-prefix matched to an
approved root. Read-only, nonblocking opening avoids FIFO hangs; metadata from the returned
descriptor must identify a regular file with read permission bits. OS permissions apply.
The inclusive opening-size ceiling is the lower configured decimal-MB limit, never above
50,000,000 bytes. `FILE_UNAVAILABLE` and `FILE_TOO_LARGE` retain no paths or OS error strings;
the latter contains only actual/limit byte counts. Debug output redacts file/root metadata.

`ValidatedFile` carries that original descriptor, opening size and a sanitized ASCII basename
of at most 128 bytes. It copies no file content and performs no Telegram or SQLite operation.
Replacing the source pathname cannot change the opened inode. In-place writes, truncation,
hard links and mounted content remain within the trusted-local-file model; this is not an
immutable content snapshot. The uploader below bounds reads to the opening size, uses this
handle directly and handles truncation.

The saved official [Bot API](https://core.telegram.org/bots/api#senddocument) reference fetched
on 2026-10-29 was rechecked against its recorded SHA-256 during file-boundary implementation.
Both `sendDocument` and multipart file-sending guidance document a 50 MB cloud ceiling.
Live DNS was unavailable for a fresh fetch; no live upload or new platform-limit experiment
is claimed. Decimal MB is the conservative interpretation used by configuration and opening.

## Document delivery

`OutboundService::send_file` consumes a `ValidatedFile` from the confined opener and formats
the complete plain caption with the current session title before admission. Both the final
1,000-scalar and 1,024-UTF-16-unit limits apply; an empty description leaves only `[Title]`.
Text and documents share send admission, metadata reservations, detached task ownership and
the successful-send transaction. A confirmed document commits a composite `Document` route
and activity atomically; no caption, file contents or source path is stored.

The private client uploader sends multipart `chat_id`, `caption` and `document` fields from
the original descriptor with its sanitized filename and opening size. It neither reopens a
path nor copies a file. Lazy reads and Tokio's file buffer are each capped at 64 KiB; total
reads stop at the opening size even after growth. Early EOF fails the body instead of
finishing a partial upload. Pathname replacement leaves the original inode selected; in-place
writes remain observable, so this is not a content snapshot. No whole-file allocation occurs.

One attempt has a finite 600-second deadline covering upload and bounded response decoding,
with the existing five-second connect limit. Consistent 4xx rejection, ambiguous delivery and
post-send bookkeeping failure retain the text service's distinct errors. Read failure,
truncation or timeout during an attempted upload is conservatively uncertain; none authorizes
automatic replay or a fabricated route. Caller cancellation leaves admitted work running to
bookkeeping completion. Daemon shutdown draining remains a later step. New tests await FINALIZE;
no live upload or runtime memory measurement is claimed.

## Telegram HTTP client

`clients::telegram::TelegramClient` owns one reusable reqwest/rustls pool; clones share it.
The production constructor fixes `https://api.telegram.org`, disables proxy discovery,
redirects and reqwest retries, and retains no transport errors or Telegram descriptions.
Only crate tests can construct a mock endpoint; TOML has no API override. No production
startup path contacts Telegram yet. The delivery service owns title formatting,
destination authorization, capacity reservations and durable routes for text/document sends.

The adapter provides `getUpdates`, `getWebhookInfo`, plain `sendMessage`, streamed
`sendDocument` and typing `sendChatAction`. Updates retain
only message/reply IDs, sender/chat, text/caption and document/photo metadata; quoted messages
are not recursively decoded. DTO and client Debug output is redacted. Outbound text passes
the existing final scalar/native validator before I/O. No route or cursor changes occur here.

Connection establishment has a five-second limit. Each attempt covers headers and the whole
response body within 15 seconds, configured poll seconds plus ten for `getUpdates`, or
600 seconds for documents including upload.
Advertised and actual body sizes are capped by `limits.telegram_response_bytes` before JSON
decoding. Reads and idempotent chat actions allow three attempts with exponential 250/500 ms
backoff plus less than that interval of jitter. Telegram `retry_after` takes precedence;
delays above 30 seconds are returned to the caller without shortening or sleeping. There
are at most two retry sleeps, so each invocation remains finite. Cancellation drops the call.

Message/document sends are attempted once. A consistent API rejection with a 4xx code is definitive;
transport loss, timeout, malformed/oversized results, server errors or inconsistent message
identity yield `TELEGRAM_DELIVERY_UNCERTAIN`. Callers must not automatically resend. Safe
errors keep only classifications, numeric codes and retry delays. Webhook conflicts (409)
remain explicit errors; the adapter never deletes webhooks or drops pending updates.

The official [Telegram Bot API](https://core.telegram.org/bots/api) snapshot fetched on
2026-10-29 at 18:34 UTC was checked against its recorded SHA-256. It documents text 1–4,096
characters, captions 0–1,024 after entity parsing, cloud document uploads up to 50 MB and
optional downloads up to 20 MB. The bridge uses a conservative decimal-MB upload ceiling
and UTF-16 caption count; this does not claim a live Unicode boundary experiment.

The same reference documents unique update IDs that can restart randomly after a week without
updates; they are not a global numeric high-water mark. It confirms `getUpdates` batches of
1–100, positive long-poll timeouts,
confirmation by a higher offset, destructive negative offsets (rejected here), and up to
24 hours of update retention. Webhooks and long polling are mutually exclusive;
`getWebhookInfo.url` is empty when unset. Switching requires an operator-authorized
`deleteWebhook(drop_pending_updates=false)`, never an automatic reset. `retry_after` is a
minimum number of seconds before repeating a rate-limited request; typing lasts at most
five seconds and can clear on a new bot message. These are documented API contracts,
not live bot observations.

The complete, unfiltered `cargo test --locked --workspace` suite passed in Runner FINALIZE
through the read-only trusted route, including the synthetic HTTP client tests. Earlier
Native attempts failed at loopback `bind` with `Operation not permitted`; that limitation
did not affect the successful Runner execution. No socket tests were skipped. Live Telegram
behavior remains untested, and production configuration exposes no endpoint override.

## Reply ingestion

`services::ingress::IngressService` accepts only allowlisted chats and non-bot senders, and
explicit Replies containing text/captions or document/photo metadata. Unsupported/unapproved
updates compact into a single ignored marker; authorized unknown/expired routes retain only
an unroutable disposition with the triggering chat/message IDs. No full update is retained.
The repository resolves composite routes and commits a distinct pending reply, receipt-based
expiry, monotonic session activity and cursor together. Redelivery produces no new reply.
Only a successful commit returns `IngestOutcome::ReplyStored` for later change signaling.
Exact retained update IDs, composite message identities and acknowledgment receipts suppress
duplicates across restart; an unseen lower ID can start a new Telegram sequence. Redelivering
a retained update leaves the current cursor unchanged, including after a sequence reset.

Callers must submit ordered Telegram batches and stop at errors; gaps cannot reveal omitted input.
Capacity/disk failures preserve cursor/replies. UTF-8 text plus bounded attachment JSON count
toward inbox bytes; identities, attachment JSON, unroutable dispositions, receipts and notices join shared
variable metadata accounting including send reservations. Unroutable dispositions share the
receipt count budget and reply TTL. The fixed ignored marker is bookkeeping like the singleton
cursor: it consumes no receipt or variable metadata admission and remains under the physical
database cap. Compacting it removes no reply, receipt or notice obligation; older ignored input
may be ignored again without retaining contents or generating effects. Cleanup, downloads,
notices and notification delivery remain later work.

## Telegram long polling

`services::polling::PollingService` holds one lease shared by the application's database
handle, including across separate service instances. A competing consumer fails before HTTP.
Startup inspects `getWebhookInfo`; a nonempty webhook or a 409 polling conflict stops with a
safe rejection. It never deletes webhooks or pending updates. The existing verified Bot API
reference above supplies the offset, batch, timeout and conflict contracts; no live bot was used.

Each sequential `getUpdates` requests at most 100 messages with the configured timeout
(45 seconds by default). The existing client bounds response bytes and parsing. The poller
validates strict batch ordering and nonnegative, incrementable IDs before any batch effects;
gaps and lower IDs from a new sequence remain valid. Each new offset comes only from the
durable cursor plus one. An unresolved update holds its bounded batch locally, retries that
update and prevents another poll from confirming past it. Capacity pressure never evicts a
reply. Receipt timestamps remain fixed across local retries.

At batch receipt, ingress captures a bounded snapshot of outstanding route reservations,
including detached text/document sends. Unknown authorized Replies defer only while sends in
that snapshot can still produce a route; later admissions cannot extend the wait. Known routes
and ignored input do not wait. Completion or failure releases the reservation before waking
ingress, which rechecks routing under the same database guard used for admission and persistence.
The wakeup is armed before checking, and no database guard crosses an await. New committed
replies emit coalesced in-memory watch revisions; consumers must read SQLite after a wakeup or
restart. These are internal signals, not implemented MCP subscriptions.

After the client's finite read retries, transient failures use quiet exponential backoff with
jitter (one to 30 seconds locally); `retry_after` can extend that delay and is never shortened.
Storage pressure uses the same bounded local backoff. Shutdown, loss of the supervisor's watch
sender or dropping the run future cancels HTTP and waits, releases the lease and preserves
committed replies/cursor. The next consumer resumes from SQLite. Production supervision and
shutdown draining of admitted sends remain later steps. New poller tests await FINALIZE.

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
