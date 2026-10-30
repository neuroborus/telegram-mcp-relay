# Architecture

## Current status

The binary now loads one TOML path and runs the shared library composition on Tokio's
current-thread runtime, with structured `tracing` diagnostics on stderr. It wires protected
SQLite, the bounded Telegram client, text/document delivery, the shared protected MCP endpoint,
one reply poller, typing, notices and retention/reclamation. Startup and supervision are
described under [runtime and ownership](#runtime-and-ownership), including graceful accepted-send
draining after SIGINT/SIGTERM in the binary or host cancellation through the embedding API.
The accepted HTTP adapter's synthetic tests passed in Runner's complete workspace test suite,
as described below. New composition tests await FINALIZE.
Reply ingestion authorizes input and commits replies/dispositions with the cursor.
A cancellable single-consumer long poller drives it.
Bounded library inbox snapshots, pre-processing claims, durable processing-start,
explicit reconciliation and acknowledgment operations are available. MCP registration and
outbound tools, read-only inbox resources and inbox/processing/acknowledgment tools are
available. Modern resource subscriptions deliver bounded post-commit inbox invalidations;
bounded asks use the common claim path without disabling events. Confirmed processing now drives
one shared, capped typing scheduler in the library.
Durable notice-attempt arbitration and one bounded factual-notice dispatcher are available
in the library, including service-Reply loop suppression across restart and uncertain sends.
Indexed bounded retention cleanup, incremental disk reclamation and a startup/daily library
scheduler are included in daemon composition.

The library now provides centralized validated TOML settings and typed, redacted startup
errors in `config`/`error`, with protected Unix secret resolution under `config::secrets`.
Parsing has no side effects; startup resolves secrets before opening storage or listeners.
See [configuration](CONFIGURATION.md).

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
| [Tokio](https://docs.rs/crate/tokio/1.54.1/features) | 1.54.1 | `fs`, `io-util`, `macros`, `net`, `rt`, `signal`, `sync`, `time` | Current-thread runtime, loopback listener, bounded I/O/admission, deadlines and Unix shutdown signals |
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
| `http`, `http-body-util`, `hyper`, `hyper-util`, `tokio-util` | 1.5.0, 0.1.5, 1.12.0, 0.1.21, 0.7.20 | `std`; none; `http1`, `server`; `tokio`; `rt` | Existing HTTP types/body adapters, bounded HTTP/1 serving and SDK cancellation without a second MCP stack |

Dev-only Tokio `test-util` supports controlled deadlines. Direct `sync` use supplies owned
send-admission permits; it was already
enabled in the locked SDK graph and adds no packages or lockfile changes.

Tokio's `signal` feature adds the locked transitive `signal-hook-registry` 1.4.8, reusing
existing `errno` and `libc` dependencies. Its version and checksum come from the cached Tokio
1.54.1 upstream lockfile. The cached Tokio source confirms synchronous registration through
`signal(SignalKind)` and asynchronous receipt through `Signal::recv`; handlers remain installed
after listeners drop. Only the binary registers them. Signal regression execution remains
pending FINALIZE; no new runtime evidence is claimed here.

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
application state must be shared through its factory. The protected transport below uses this
lifecycle and the subscription API below. No external client interoperability is claimed.

## Protected MCP transport

`routes::McpEndpoint` serves only the configured loopback listener and fixed `/mcp` path.
The SDK owns parsing, negotiation, dispatch and SSE. Each factory call clones one shared
`handlers::McpHandler`, including its SQLite handle; HTTP identity never creates an agent session.
The checksum-verified locked rmcp 3.8.0 sources confirm `server/discover`, HTTP request parts
in context extensions and modern stateless negotiation. Only MCP 2026-07-28 is advertised:
requests require matching `Mcp-Method` and `MCP-Protocol-Version` headers and `_meta`
protocolVersion plus clientCapabilities entries. Tool calls also require `Mcp-Name` to match
`params.name`; the SDK rejects missing or mismatched headers before handler dispatch.
Legacy initialize sessions are unsupported.
Discovery advertises tools and resources with `subscribe=true`.

Host must be the configured numeric authority or `localhost`, including its port. Present
Origin must use HTTP with either authority; missing Origin supports native clients. Duplicate
Host/Origin values and unapproved absolute authorities fail closed. Configured bearer auth
requires exactly one matching Authorization header before SDK dispatch. Request bodies are
bounded incrementally by the SDK, including chunked bodies. Admission lasts through response
completion/drop, including SSE; saturation returns 503. Connections are limited to twice request
admission, with 32 headers, 16 KiB header/buffer bounds and a ten-second header deadline. There
is no global call timeout. Shutdown closes admission, connections and pending body reads before
draining accepted work; stalled clients cannot extend that drain.

Correlation uses fresh random IDs, never client-supplied IDs. Authorization, proxy authorization,
cookies and client correlation headers are removed before SDK dispatch. HTTP rejection text is
static; JSON-RPC failures retain codes, IDs and structured negotiation data with safe messages.
Binary diagnostics suppress rmcp targets, whose upstream logs can contain raw metadata;
library embedders must retain this filter. New synthetic HTTP tests await FINALIZE; no named
client compatibility or live Telegram behavior is claimed.

## MCP registration and outbound tools

`McpHandler::new` shares one SQLite handle, outbound service and pinned file-root opener
across per-request SDK handlers. `telegram_register` accepts a required title and optional
`external_session_id`/saved `session_key`, using the existing transactional registration path.
It returns `session_key`, normalized `title` and stable readable `inbox_uri`.
Missing both identities creates a fresh session. Supplying
both requires an existing match. Renaming affects future sends only, and equal titles never
merge identities. Registration does not change resource listings or processing ownership.

`telegram_send` accepts `session_key`/`text`; `telegram_send_file` accepts `session_key`, an
absolute approved local `path` and an optional `caption` (empty by default). The handlers
delegate to confined opening and existing delivery services without retries or body/file
persistence. Success returns `telegram_chat_id` and `outbound_message_id` only after the
composite reply route commits. Empty captions retain the session header. Formatting, native
caption bounds, send admission and cancellation-safe bookkeeping remain service-owned.

The locked rmcp 3.8.0 `Tool` input/output schema builders use typed Serde DTOs and the SDK's
already-enabled schemars support; no new crate/features are needed. `tools/list` is stable,
unpaginated, private and immediately stale (`ttlMs=0`); supplied cursors are invalid.
Unknown tools and malformed/missing/extra argument fields return safe JSON-RPC invalid-params
errors. Application failures return `isError=true` with structured `code`, safe `message`
and `details`, mirrored in text content. Length errors retain exact counts/guidance; file-size
errors retain byte limits/counts; Telegram rejections retain numeric code/retry delay.
Uncertain delivery and confirmed-send bookkeeping failure remain distinct and explicitly
forbid automatic resending. Supplied paths, keys, bodies and upstream descriptions are never
included in errors. Synthetic MCP-to-mocked-Telegram tests await FINALIZE; no external client
interoperability is claimed.

## MCP inbox resources and reply tools

`resources/templates/list` exposes the stable
`telegram-bridge://sessions/{session_key}/inbox` template. `resources/list` stays empty,
including after registration: it never enumerates session keys. Direct `resources/read`
requires a generated session handle in that URI, independent of any connection registration.
The shared local bearer token and session handles assume trusted local clients, as described
under [security](#retention-and-security); they are not separate per-client identities.

Snapshots contain JSON `replies` and nullable `next_uri`. Optional `?limit=1..100` defaults
to 100; follow `next_uri` unchanged for keyset pagination in receipt-time/update-ID order.
Each page also respects the configured reply payload budget. Continuations use
`?limit=N&after=created_at,telegram_update_id`, scoped by the URI's session key. Pages reflect
current durable state, not a frozen history. All delivery states, text/caption, attachment
metadata and routing IDs are visible; tokens are never present. Timestamps are Unix
milliseconds. Reads never claim, acknowledge or refresh session activity. Lists and reads
are private and immediately stale (`ttlMs=0`). Malformed URIs/cursors and unknown sessions
return safe protocol errors; rmcp maps missing resources to `-32602` for MCP 2026-07-28.
The locked SDK also requires `Mcp-Name` to match `params.uri` on resource reads.

`telegram_inbox({session_key, limit?})` uses the common ordered claim transaction and returns
`replies` with fresh `claim_token` values. It claims only pending or lease-expired
pre-processing replies and never completes them. Preserve each reply ID and current token.
`telegram_processing({session_key, reply_id, claim_token, active})` records the first start
when active, or preserves ownership when inactive; neither operation acknowledges a reply.
Typing remains pending. For explicit processing recovery, omit `active` and instead supply
`reconcile_request_id`: save a fresh random 1–64-byte request ID with the current token before
dispatch. Exactly one action is required. The result returns the reply ID, current token and
original first start. Save the rotated token; retry a lost response with the identical saved
request ID and prior token. Registration alone never rotates ownership.

`telegram_ack({session_key, reply_id, claim_token})` removes the successfully handled payload
through the existing bounded receipt transaction and returns `reply_id`/`acknowledged=true`.
Identical valid-token retries succeed while the receipt remains valid; stale or wrong-session
claims return `CLAIM_INVALID`. New MCP interaction tests await FINALIZE; named-client
interoperability is not established by these resources.

## MCP resource subscriptions

The library implements MCP 2026-07-28 `subscriptions/listen` through locked rmcp 3.8.0.
Supply `params.notifications.resourceSubscriptions` with concrete canonical inbox URIs
returned by registration, plus the usual required request metadata and matching `Mcp-Method`.
The SDK acknowledges the accepted filter with `notifications/subscriptions/acknowledged`;
`SubscriptionSink::notify_resource_updated` emits `notifications/resources/updated` and attaches
`params._meta["io.modelcontextprotocol/subscriptionId"]` matching the listen request ID.
This is a POST SSE stream; closing it cancels that request, without changing durable replies.

Only known session keys authorize interests, using the same saved-handle trust model as reads.
Unknown/malformed URIs, templates, query pages and unsupported notification categories are
omitted from the accepted filter. Empty acceptance completes immediately. Requests exceeding
`limits.subscriptions` URI entries (including duplicates) accept no interests. Duplicate
accepted URIs share one delivery interest. Resource listings remain stable and private.
Legacy subscription methods and older protocol revisions are unsupported.

`repositories::notifications` holds at most `limits.subscriptions` listeners (default 64),
each with at most that many inbox keys and `limits.subscription_queue` pending signals
(default 16). Successful reply, claim, first processing-start, reconciliation and acknowledgment
transactions publish after commit; rejected/rolled-back operations, redelivery and idempotent
retries do not. Signals contain only a session key, never reply payloads or tokens. Dispatch
uses nonblocking bounded queues and coalesces already queued changes for the same inbox.
A full queue closes only that listener. Each SDK sink send also has a five-second backpressure
deadline. Cancellation, disconnect, overflow and sink
failure release interests/admission. SDK transport queues and HTTP response admission supply
additional independent bounds; no per-session task or idle inbox polling is introduced.

The SDK acknowledges before entering the application listener. The listener arms its queue,
then sends one initial invalidation per accepted inbox to cover commits during that setup gap.
Clients read SQLite-backed resources or claim through `telegram_inbox`, including after
reconnect/restart; signals are neither persistent nor replayed and never claim or acknowledge.
There is no guarantee that a host wakes an idle LLM. Actual HTTP protocol tests are defined in
[testing](TESTING.md); their execution remains pending FINALIZE. No named client or live
Telegram interoperability is claimed.

## Bounded asks

The seventh tool, `telegram_ask`, shares `telegram_send`'s `session_key`/`text` schema,
validation and cancellation-safe bookkeeping. Server-configured waiting defaults to 600 seconds
(1–86,400). Results include the committed `telegram_chat_id`/`outbound_message_id`, `status`
and nullable `reply`: `sent` means waiting is disabled, `reply_received` includes one fenced
claim, and `wait_timeout` means no claim was acquired before the deadline. An inbox consumer
winning a claim leaves the ask waiting for another eligible reply to that question.

`services::ask` reserves and arms a one-slot coalescing interest before sending, then rechecks
SQLite after route commit and every signal. Claims match session and composite question
identity through `InboxService`. `limits.waiters` (default 16) is independent of listener
admission; exhaustion returns `WAIT_CAPACITY` before Telegram effects. One finite deadline
starts after send bookkeeping, without inbox polling or extra workers. Resource events
continue when an ask claims a reply, waiting is disabled or admission is full.

The handler observes SDK cancellation, including HTTP disconnects. Dropping the ask releases
admission; admitted sends still finish bookkeeping. Wait termination never deletes routes/replies,
acknowledges work, starts processing, interrupts agents or emits Telegram notices. Claims lost
with responses recover under the existing lease. Post-send `ASK_WAIT_FAILED` includes committed
outbound IDs and inbox recovery guidance, never permission to resend. Clients may need tool
timeouts exceeding wait plus send time; the bridge cannot configure them or guarantee a new
LLM turn. New tests in [testing](TESTING.md) remain pending FINALIZE.

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
Version 3 adds a session/receipt-time/update-ID index for ordered inbox access.
Version 4 adds nullable recovery-proof columns for the latest processing reconciliation;
existing claims and first-start timestamps survive the transactional migration.
Version 5 adds an unroutable-attempt flag and indexes for receipt-ordered notice eligibility
and composite trigger deduplication, preserving existing attempts and receipt times.
Version 6 extends the session expiry index with its key for bounded cleanup pagination.

Initialization enables incremental auto-vacuum before the schema transaction. DDL, initial
cursor, application ID and version commit together under an immediate transaction; errors
roll back the schema. Foreign/unversioned nonempty databases and unknown versions are rejected.
Foreign keys restrict session deletion while children survive; replies have no route foreign
key, so an expired route can later be removed independently. Indexes cover identity, ordered
inboxes, question matching, notice eligibility and each record's expiry.

Connections use DELETE rollback journals, FULL synchronization, a 100 ms busy timeout, a
512 KiB suggested page cache, eight cached statements, disabled mmap and in-memory temporary
work. `max_page_count` limits main-file growth to the configured budget rounded down to pages;
an already oversized database is rejected without eviction. The cap covers the main file,
including reusable pages, rather than the whole filesystem allocation. Rollback journals
require additional transient headroom; see [disk maintenance](#physical-disk-maintenance).
No synchronous database guard may cross an await.

rusqlite 0.32.1 embeds SQLite 3.46.0 through libsqlite3-sys 0.30.1; `bundled` also enables
`modern_sqlite` and pregenerated bindings. Cached published sources match checksum-verified
archives, and the used APIs were inspected. Focused storage tests use this embedded build.
Live registry/SQLite documentation requests failed at DNS resolution; no latest-version claim
or live Telegram behavior is implied by the pinned dependency verification.

## Retention cleanup

`Database::cleanup_batch` uses one immediate transaction to delete at most 100 expired rows
from each of replies, acknowledgment receipts, notice attempts, update dispositions and routes.
It then examines at most 100 expired sessions after expired child batches have drained.
Expiry-index scans and a session `(expires_at, session_key)` cursor bound candidate selection;
protected sessions cannot stall later candidates. Failed transactions advance no sweep cursor
and publish no inbox invalidations. Successful reply deletion signals only after commit.
Each batch returns bounded committed reply-deletion identities to the cleanup service.
The durable Telegram cursor is never removed or reset, including when its last reply expires.

Independent default 30-day deadlines remain anchored to original route creation, reply receipt
and last session activity. Processing replies expire only at reply retention, regardless of
claim, typing or wait deadlines. Route, inbox, claim, processing and acknowledgment queries
exclude expired records before physical deletion. A session identity remains resumable while
retained: cleanup preserves it while any route, reply, acknowledgment receipt or admitted send
needs it. This includes expired children awaiting a later bounded batch. Registration can
refresh a retained identity; after deletion an old saved handle fails, and an external ID can
register a fresh session. Cleanup never refreshes activity or evicts valid records for capacity.

`services::cleanup::CleanupService` performs a startup sweep, yields between committed batches,
then sleeps for `storage.cleanup_interval_hours` (default 24). One shared-database admission
permit prevents duplicate schedulers. Shutdown or cancellation stops between transactions;
restart safely scans durable state again. No record has its own timer.
`CleanupService::with_typing` attaches the same scheduler as inbox/outbound services and
releases deleted replies' remembered caps after commit, under the shared database guard.
Rollback preserves both durable replies and RAM activity/caps.
After deletion, the scheduler snapshots the free-list count and yields before each separate
reclamation transaction. This finite initial page budget prevents concurrent churn from
extending the sweep indefinitely. Remaining free pages can be reused by ordinary writes and
are reconsidered next sweep. Daemon startup waits for the entire first sweep before starting
polling, notices or MCP serving, retaining the same cleanup lease for periodic maintenance.

## Physical disk maintenance

`Database::reclaim_batch` drains every result row from `PRAGMA incremental_vacuum(32)` inside
one immediate transaction, then commits. Each SQLite step removes at most one free-list page;
pointer-map boundary pages may also disappear. The locked SQLite 3.46.0 source confirms the
step limit and row-per-step behavior: executing only the first result would reclaim one page.
There is no full `VACUUM`, second database copy, unbounded reclamation transaction or per-record
timer. This does not compact partially occupied pages, so file size need not equal live payload
size. A failed maintenance transaction leaves valid records recoverable, and cannot undo
earlier logical cleanup, inbox notifications or typing-cap release. Cancellation between
transactions preserves committed reductions; restart discovers remaining work from SQLite.

`Database::disk_usage` reports main-file and DELETE-journal lengths, allocated bytes derived
from Unix 512-byte block counts, page size, reusable free-list pages and the effective whole-page main
cap. The main file remains pinned by its validated descriptor; journal inspection uses the
pinned private directory, rejects symlinks/special/unprotected files and treats only absence
as zero. It never scans arbitrary directories or includes local paths in diagnostics.
`temp_store=MEMORY` keeps query work and statement subjournals off disk; no attached databases
or persistent upload copies exist. Temporary disk usage is therefore zero for these operations,
while temporary query/subjournal **memory** remains additional to the suggested page cache.

The main-file cap defaults to 64 MiB (configurable from 1 MiB to 1 GiB, rounded down to pages).
It is not a total-disk quota: allow extra space for the rollback journal's original modified
pages, per-page records and sector-aligned headers. A large expiry batch may journal much of
the database even though it deletes at most 100 rows per table. FULL synchronization, DELETE
journaling and `journal_size_limit=0` remain intact; completed transactions remove their
journal, while interrupted ones may leave a protected hot journal needed for recovery.
Bounded reclamation also needs journal headroom and can fail on an exhausted filesystem;
never delete a hot journal or evict valid replies to make it succeed. Restore space and reopen
or retry maintenance. SQLite page-cap failures map to `STORAGE_CAPACITY`; other filesystem/I/O
failures retain safe storage errors. Physical-filesystem ENOSPC/crash fault injection remains
an operational limitation of the synthetic tests.

Usage is a snapshot rather than peak telemetry or reserved disk space, and excludes filesystem
metadata, snapshots, logs and user-owned outbound sources. Filesystem allocation rounding can
exceed file lengths. The private directory and same-user single-process ownership remain trust
requirements; concurrent external writers/replacement invalidate accounting assumptions.
Measured runtime/peak disk behavior remains pending.

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
reports failed bookkeeping distinctly from an unknown send outcome. Physical maintenance
remains a later step.

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
finish. A shared `TaskTracker` retires completed task storage immediately; daemon shutdown closes
send admission and drains tracked work, including disconnected callers. Tracking is armed before
permit acquisition so concurrent close cannot miss an admitted send. Crashes and forced process
termination can still interrupt bookkeeping. No outbound body is written to SQLite.

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
bookkeeping completion. Graceful daemon shutdown drains the same tracked work. New tests await
FINALIZE; no live upload or runtime memory measurement is claimed.

## Telegram HTTP client

`clients::telegram::TelegramClient` owns one reusable reqwest/rustls pool; clones share it.
The production constructor fixes `https://api.telegram.org`, disables proxy discovery,
redirects and reqwest retries, and retains no transport errors or Telegram descriptions.
Only crate tests can construct a mock endpoint; TOML has no API override. Production
startup shares that client across polling, delivery, typing and notices. The delivery service
owns title formatting,
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
decoding. Reads allow three attempts with exponential 250/500 ms
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
Only a successful commit returns `IngestOutcome::ReplyStored` and publishes an inbox signal.
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
may be ignored again without retaining contents or generating effects. Cleanup and downloads
remain later work.

## Durable notice arbitration

`NoticePolicy` uses `delivery.processing_start_grace_seconds` (default 60) and the two
`notifications` switches. `Database::take_notice_attempts` selects at most 100 unexpired
eligible records in receipt-time/update-ID order through partial eligibility indexes.
Unconfirmed eligibility starts at durable receipt plus grace; claims do not suppress it.
A recorded first processing start excludes the reply permanently, including after inactivity,
reconciliation or typing expiry. Completed replies have no pending notice obligation.

An immediate transaction serializes first start and notice admission across connections.
If first start commits first, no attempt is returned. If notice admission commits first,
a later start remains valid but cannot undo that attempt. Each returned attempt already has
a durable record and source flag; a crash before dispatch or an ambiguous send consumes it.
It is an attempt decision, not proof of delivery, and never authorizes automatic retries.

Authorized unroutable dispositions commit with ingress and deduplicate by composite trigger
as well as update ID. Retained attempts also suppress redelivery after disposition removal.
Notice attempts share the receipt count and metadata budgets with acknowledgments and
unroutable dispositions, including outstanding send reservations. Any batch admission/SQL
failure rolls back every decision without removing replies or advancing ingress. Attempt
expiry stays anchored to the source reply/disposition expiry; attempts have no payload or
reply/session foreign key. Cleanup remains later work.

`services::notices::NoticeService` holds one scheduler lease per shared database handle.
One global one-second sweep admits at most one eligible attempt, with one bounded HTTP send
at a time and no per-reply tasks/timers. Startup recovers eligibility from SQLite. The current
chat allowlist and complete final Unicode limits are checked before sending. Known sessions
use their normalized title and factual unconfirmed-processing wording; unknown routes use
the concise instruction to Reply to a newer agent message. Invalid final lengths consume the
attempt without sending, splitting or truncating. Disabled categories remain untouched.

Success records only the composite notice message ID on the existing attempt, never an
ordinary route or session activity. Rejection, ambiguous delivery, cancellation and failed
success bookkeeping cannot authorize replay. Rate-limit delays postpone other notice attempts
without retrying the consumed one. Shutdown, loss of the supervisor or dropping the run future
cancels the current request and releases admission. Daemon supervision owns this scheduler.

Ingress silently commits Replies to retained composite notice IDs as ignored. If the success
ID was lost, bot-authored quoted text matching the reserved service wording (including an old
normalized title) also suppresses a loop across restart or attempt cleanup. Ordinary live
routes take precedence, and the wording fallback waits for already-pending ordinary sends to
settle. The fallback deliberately treats matching bot text without a live route as service
text; human-authored matches and other unknown bot messages remain unroutable. The saved
official Bot API reference above confirms optional `Message.from`/`text` in the original
`reply_to_message`; parsing retains only these shallow fields and never nested Replies.
No service-Reply contents are persisted or published to inbox subscribers. Routine activity,
confirmed long work, wait/typing expiry and reconnects introduce no notice trigger.

## Inbox snapshots and claims

`Database::inbox_snapshot` reads unexpired replies in receipt-time/update-ID order, preserving
ordered Telegram batch ties. Keyset pagination covers every delivery state without acquiring
ownership, exposing claim tokens or refreshing session activity. Pages and claims are limited
to 100 items and `limits.reply_bytes` of combined UTF-8 text/attachment payload. If a newly
lowered limit cannot fit the first retained reply, return capacity pressure without skipping it.

`services::inbox::InboxService` validates the configured 1–600-second claim lease (default 60).
Inbox and future ask consumers share one immediate SQLite transaction for ordered selection,
metadata admission and claims. Optional question filtering includes both chat and outbound
message ID within the session. Each acquisition uses a fresh 32-byte OS-random fencing token;
only a committed transaction returns tokens. Admission counts token bytes and outstanding send
reservations, replacing an old token's charge during recovery. Failure rolls back the batch.
Pre-processing claims become eligible exactly at lease expiry, including after restart, with
the same reply ID and a rotated token. Leases never extend reply retention. Processing replies
are excluded from automatic recovery; reads/claims never acknowledge, delete or start typing.

`InboxService::processing` validates the session/reply/current token and unexpired reply.
Pre-processing claims must also have an unexpired lease. The first active call atomically
records processing start and removes the claim lease; repeated calls preserve that first
timestamp. Inactivity preserves durable ownership and does not complete or requeue a reply.
The repository transaction has no network effects or typing timers. The shared inbox service
updates optional RAM typing activity only after a successful fenced transition.

Explicit `reconcile_processing` requires the saved current token as a compare-and-swap fence
for a processing reply and a fresh, unguessable client request ID of 1–64 UTF-8 bytes
(for example, a random UUID). Clients save both before dispatch. It rotates the token and
acquisition time while preserving the
reply ID, first-start timestamp, payload and retention deadline. The same transaction retains
the request ID and prior-token proof: an identical retry recovers the committed result after
response loss/restart without changing timestamps or requiring fresh capacity. Only the latest
proof survives; a later rotation, completion or reply expiry prevents its replay. Registration
does not reconcile claims. Distinct competing resume requests have one winner; stale tokens
cannot start/stop processing, change ownership or acknowledge. Clients preserve returned tokens.
Proof bytes count toward shared metadata, replace the prior proof and disappear with the reply.

`InboxService::acknowledge` accepts a valid claimed or processing reply and atomically replaces
its payload with a minimal valid-token receipt. Repeated acknowledgment with the same session,
reply ID and token succeeds while that receipt remains valid, including after restart or at
capacity. Receipt retention uses the configured reply TTL from completion; retries never
refresh it. Receipt counts share `limits.receipts` with unroutable dispositions. Metadata
admission accounts for replacement bytes and outstanding send reservations. Admission or
transaction failure preserves the reply and ownership; no valid records are evicted.

## Confirmed-processing typing

`OutboundService::typing` exposes one shared `services::typing::TypingService` for sender clones,
`InboxService::with_typing` and `CleanupService::with_typing`. `McpHandler` connects sender/inbox
activity once for all request factories; daemon cleanup attaches that same typing handle.
Only a successful durable `processing(active=true)` enables activity. Receipt, claims, reads,
waits and registration never start typing. A new reconciliation rotation stops the previous
RAM activity; an explicit active call with the rotated token resumes within the original
first-start cap. Recovering an identical reconciliation result leaves resumed activity intact.

One lazy task merges active replies per chat, sends immediately when the chat is eligible and
refreshes every configured four seconds by default. Each request is attempted once and bounded
by the refresh interval and remaining chat activity; rate-limit cooldowns honor `retry_after`.
The single task polls bounded concurrent request futures, at most one per allowlisted chat,
admitting all due chats in stable order. A stalled request cannot delay another chat. One
coordinated timer cancels requests whose chat becomes inactive or reaches its deadline.
A request already delivered to Telegram cannot be recalled; its visible status lasts at most
about five seconds and may clear on a bot message.
Cooldowns survive quick stop/start races, limiting each chat to one attempt per interval.

The durable first-start timestamp bounds activity at 600 seconds (or a lower configured cap).
Monotonic caps survive inactivity, reconciliation and expiry, so a backwards wall-clock change
cannot extend them on a later active call. Valid inactivity or acknowledgment stops that reply;
a known accepted text/document send stops only its session,
including when detached bookkeeping later fails. Rejected/uncertain sends preserve activity.
Other active sessions keep the merged chat eligible. Failed transitions have no typing effect.

Active and inactive cap entries contain only IDs/deadlines/flags and together are bounded by
`limits.inbox_items`; acknowledgment or committed retention cleanup releases an entry.
Admission never evicts remembered caps.
Cooldowns and outstanding request futures are bounded by the allowlisted chats.
At idle the task exits, with no periodic timer for retained caps. The last shared handle
cancels the worker; a new service starts empty. Typing expiry affects no durable processing/ack state,
wait or agent execution, and sends no notice. Daemon supervision observes unexpected worker
abort/panic or scheduler-state failure through a bounded watch signal, without an idle timer.

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
restart. These internal watch signals remain separate from the shared database's bounded
post-commit subscription interests; neither path competes for or consumes replies.

After the client's finite read retries, transient failures use quiet exponential backoff with
jitter (one to 30 seconds locally); `retry_after` can extend that delay and is never shortened.
Storage pressure uses the same bounded local backoff. Shutdown, loss of the supervisor's watch
sender or dropping the run future cancels HTTP and waits, releases the lease and preserves
committed replies/cursor. The next consumer resumes from SQLite. Daemon supervision stops peer
services on terminal poll failure, then drains already admitted sends before returning the failure.
New poller tests await FINALIZE.

## Runtime and ownership

The binary registers SIGINT and SIGTERM before starting services, then passes their shutdown
future to `app::run_until` and awaits completion. Registration failure emits the safe static
`SHUTDOWN_SIGNAL_UNAVAILABLE` code and exits before service startup. `app::run_until` loads the
supplied configuration path, resolves protected secrets, opens one SQLite connection, pins
export roots and constructs all services before binding the loopback listener.
The same `Arc<Mutex<Database>>` reaches MCP handlers, outbound delivery, ingress, cleanup and
notices. All Telegram adapters clone one reusable client. Post-commit subscription/wait
signals remain database-owned; no extra relay or periodic inbox reader is needed.

One fixed supervisor owns cleanup, polling, notices, MCP serving and an observer for typing
and admitted-send failures.
Cleanup retains its exclusive lease and completes startup deletion/reclamation before the
other loops start. Each loop remains bounded by its existing admission policy; these fixed
supervisors are additional to `limits.background_tasks` admitted sends. The typing scheduler
still starts lazily on confirmed processing, merges chats and retires when idle. Restart
constructs empty RAM typing state rather than replaying durable processing into activity.

An unexpected service exit or panic fails the composition, cancels its peers and awaits their
retirement. Safe diagnostics include only static service/error codes and package version;
join panic payloads, settings, secrets and paths are never formatted by supervision.
Construction/bind failure starts no worker; failed startup cleanup starts no serving or
Telegram activity. Initialization may leave a private database/token file; startup never
deletes or repairs operator files to roll back an error.

`app::run_until` accepts a host cancellation future. Shutdown first fences tool/send admission
and closes bounded subscription/wait interests, then cancels MCP connections and service loops.
Synchronous ingress transactions finish before yielding; unresolved updates remain unconfirmed
and recover from the durable cursor. Handler/listener futures and every admitted send are tracked
and drained, including sends whose callers disconnected. No short shutdown timeout aborts route
bookkeeping: existing 15-second text and 600-second upload deadlines bound network attempts,
followed by synchronous SQLite bookkeeping. Failure, uncertainty and failed bookkeeping never
authorize replay or create a guessed route. Service errors survive draining; individual send
failures retain their safe diagnostics. Detached send panics wake supervision and fail draining.

Typing admission closes too: the lazy worker wakes, drops pending best-effort actions and is
awaited without waiting for its refresh timer. Shutdown never acknowledges replies or resets
processing identity, and sends no shutdown/timeout notice. Completed task results are not
accumulated in a growing supervisor collection. Diagnostics retain static service/error codes;
no additional log queue or per-agent task is introduced.

Both library entry points remain free of process-wide signal handlers. `app::run` waits for a
supervised service failure; embedding hosts that need graceful shutdown supply cancellation
and await `app::run_until`. Dropping its future only fences admission, cancels the SDK and aborts
the supervisor task set, without awaiting detached sends. In the binary, either SIGINT or SIGTERM
starts the drain; repeated signals do not force an early exit. Allow pending uploads and their
bookkeeping to finish before an external service-manager deadline. SIGKILL, process crashes and
forced termination can bypass draining and interrupt bookkeeping after Telegram accepts a send;
they do not justify automatically resending. Composition tests use the same private constructor
with a mocked client; production exposes no endpoint override.

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
Library cleanup preserves still-valid replies and referential integrity in bounded batches.
Bounded incremental reclamation follows cleanup; deletion alone need not shrink the file.
Capacity pressure cannot silently evict valid replies.
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
