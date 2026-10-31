# Configuration and Secrets

For initial installation, Telegram IDs/webhook checks, private directory preparation and
optional systemd examples, follow [setup](SETUP.md). Install the
[agent operating policy](AGENT_POLICY.md) in consuming projects.

The library provides `Config::load`, side-effect-free `Config::parse` and explicit
`Config::load_secrets`, with no network I/O in these operations. The binary takes exactly one
TOML path: `telegram-mcp-relay /absolute/path/to/config.toml`. Startup validates settings and
secrets, opens protected SQLite/export roots and binds the configured listener, then completes
startup cleanup before serving MCP or starting Telegram activity. Start with the
[synthetic example](../config/relay.example.toml);
unlisted settings retain [central defaults](../src/config.rs). Unknown fields, inline
credentials and API overrides fail. TOML input is bounded to 64 KiB.

Specify a nonzero destination `telegram.chat_id` inside nonempty `allowed_chat_ids`, positive
`allowed_user_ids`, and 1–64 export roots. IDs are bounded to 52 bits; ID lists to 256 entries.
Binding requires loopback and a nonzero port; `/mcp` is fixed. Paths must be absolute after
initial `~/` expansion against HOME. Relative paths, parent traversal, `~other-user` and `/`
export roots fail. Parsing does not inspect/create roots or storage. SQLite opening is explicit
through `Database::open`; `OutboundFiles::new` explicitly opens and pins existing export roots.
Root paths and every ancestor must be real directories without symlinks. File opening uses
Linux `openat2` (kernel 5.6+); unavailable or sandbox-denied resolution fails closed. Roots
are pinned until the opener is dropped: replacing a configured root pathname does not grant
access to its replacement. Recreate the opener to adopt an operator-changed root.

Bot secrets default to `TELEGRAM_BOT_TOKEN`; choose `telegram.bot_token_env` or `bot_token_file`,
never both. Optional MCP authentication uses `server.auth_token_env` or `auth_token_file`.
Bearer values need at least 32 ASCII bearer-alphabet characters; all credentials are limited
to 4,096 bytes (trailing CR/LF accepted). Debug and errors redact credentials, input and paths.
Unix secret files must be regular, single-link, effective-user-owned, owner-readable and
inaccessible to group/others (normally 0600/0400). Leaf symlinks and symlinked immediate parents
fail; descriptors are checked before bounded reads. Keep ancestors trusted and secret/database
paths outside exports. Protect environment sources with a mode-0600 service environment file.

`server.create_auth_token_file=true` requires an existing private owned parent (normally 0700).
It creates 64 hex characters from 32 OS-random bytes with exclusive mode-0600 creation and a
synced write. Existing files are validated, never overwritten/repaired; failed writes may leave
an incomplete private file requiring operator removal. No parent directories are auto-created.

Wait/grace settings accept 1–86,400 seconds; claims 1–600; independent typing duration 1–600
with refresh 1–5 seconds no longer than that duration. Long polling defaults to 45 (1–50).
Independent session/route/reply TTLs default to 30 days (1–3,650); cleanup to 24 hours (1–168).
The library cleanup scheduler sweeps on startup and after each configured interval, yielding
between indexed bounded transactions. Expired records count against capacity until deleted;
valid children, completion receipts and admitted sends protect session identity. See
[retention cleanup](ARCHITECTURE.md#retention-cleanup) for independent expiry and recovery.
Library notice arbitration uses `delivery.processing_start_grace_seconds` (default 60) from
durable receipt and honors `notifications.notify_unconfirmed_processing` and
`notifications.notify_unroutable` (both default true). Disabled categories consume no attempts.
The library dispatcher sweeps once per second, sends one admitted attempt at a time and
rechecks the current chat allowlist. Daemon composition owns this scheduler and cleanup,
and shares one typing scheduler between processing, sends and retention deletion.

`telegram_ask` honors `delivery.wait_fallback_enabled` (default true) and
`delivery.wait_timeout_seconds` (default 600), starting its deadline after send bookkeeping.
`telegram_wait` uses the same settings and shared `limits.waiters` budget (default 16, at most
`limits.concurrent_requests`). Asks reserve before sending; after-send waits authorize and arm
the retained target before querying replies, independently of subscriptions. Disabled asks
return after sending; disabled after-send waits validate the route but never send or claim.
Events and inbox recovery remain active. See [bounded asks](ARCHITECTURE.md#bounded-asks)
for cancellation, errors and client timeout requirements.

Uploads accept 1–50 decimal MB, with an inclusive ceiling of 50,000,000 bytes or the lower
configured limit. Opening rejects relative paths, parent traversal, symlinks (including ones
whose targets remain inside a root), nonregular and unreadable files. Keep exported directories
and their contents trusted; confinement does not prevent same-user hard links, mounted content
or in-place writes. Files with all read bits cleared are rejected even under elevated privileges.
Only sanitized, at-most-128-byte ASCII basenames are prepared for upload; errors omit paths.
Document streaming enforces the opening size during mutation; see
[document delivery](ARCHITECTURE.md#document-delivery).
Final outbound text/captions always have the fixed 1,000-scalar policy, counting native headers,
the optional directory, separators, service prefix and body; captions also fit 1,024 UTF-16 units.
Registration's opt-in `directory_label` is normalized display metadata, omitted/cleared when
absent or blank and rejected above 100 normalized scalars. Its UTF-8 bytes count toward
`limits.metadata_bytes`; it never authorizes file access. Positive request/task/subscription/inbox/payload/metadata budgets reject excessive values
and inconsistent capacities. SQLite's physical budget must exceed payload plus metadata budgets;
later components enforce actual usage without early eviction. The library MCP endpoint enforces
`limits.request_bytes` and `limits.concurrent_requests`, expected Host/Origin and optional bearer
authentication; see [transport policy](ARCHITECTURE.md#protected-mcp-transport). Native clients
may omit Origin. A shared bearer assumes trusted local clients, not per-agent security isolation.
The internal SDK session adapter also uses `limits.concurrent_requests` for transport-session
admission and, separately, outstanding requests/streams, plus one reserved notification slot.
Its queues contain at most 2–16 messages each, derived from that limit. Fixed internal
lifetimes are ten seconds for initialization/control operations, five minutes for idle
sessions, and thirty seconds for abandoned calls, with a one-second reclamation interval.
Attached long-running calls retain their service deadlines. These are centralized constants,
not new TOML settings, and do not change `limits.sessions` or durable retention.

At saturated HTTP/request-work admission, one additional bounded recovery slot at each layer
keeps registration, inbox/processing/ack, resource reads and ping usable. HTTP also accepts
legacy DELETE and cancellation/initialized notifications; SDK notification admission remains
separate. Parsed bodies select the recovery lane; sends, waits and subscriptions cannot use it.
Legacy HTTP routing is enabled for MCP 2025-06-18 and uses these bounds independently
of modern stateless HTTP admission. Transport IDs do not replace durable application keys. See
[SDK session ownership](ARCHITECTURE.md#sdk-transport-session-bounds) for reclamation and shutdown.
Library resource subscriptions enforce `limits.subscriptions` (default 64) for both active
listeners and URI entries per request, and `limits.subscription_queue` (default 16) per listener.
Overflow or five seconds of SDK sink backpressure closes the affected stream; reconnect and
read the durable inbox. See [subscription behavior](ARCHITECTURE.md#mcp-resource-subscriptions).

SQLite initialization requires an existing owned private parent directory (normally 0700)
and trusted ancestors. It creates only the database, with mode 0600; existing files and sidecars
must be owned regular single-link files with owner read/write access and no group/other access.
Parent/leaf symlinks and special files fail; existing permissions are never changed. The main
database file is page-limited by `storage.max_database_bytes`. Session/route admission enforces
`limits.sessions`, `limits.routes` and logical `limits.metadata_bytes`, including in-flight route
reservations. Ingestion enforces `limits.inbox_items`, `limits.reply_bytes` and
`limits.inbox_bytes` over retained UTF-8 text and serialized attachment metadata. Attachment
JSON also counts toward shared metadata; unroutable dispositions and notice attempts share
`limits.receipts` with acknowledgment receipts. Each notice attempt adds a fixed metadata row;
its source record remains retained independently. Ignored input compacts into one fixed
bookkeeping marker outside these variable admission budgets, bounded by the physical database
cap. Pressure rejects reply/unroutable admission without eviction or cursor advancement.
Notice batches contain at most 100 decisions, with whole-batch rollback on capacity failure;
unattempted sources remain eligible. Attempts retain the original source expiry, and consumed
attempts never become retryable after cancellation, uncertain delivery or restart.
Inbox pages and claim batches contain at most 100 items and `limits.reply_bytes` combined
payload bytes. Claims enforce `delivery.claim_lease_seconds` (default 60, maximum 600) before
processing starts; processing work is not lease-expired. Tokens count toward shared metadata
capacity, including outstanding send reservations; rotation replaces the previous token charge.
Processing start removes the pre-processing lease. Explicit reconciliation rotates its token
without resetting first start or reply retention. Its bounded latest-request/prior-token proof
counts toward shared metadata and lets identical retries recover a lost response after restart.
Acknowledgment atomically removes payloads and that proof
and retains a minimal receipt for `storage.reply_ttl_days` from completion. Receipts consume
the shared receipt/metadata budgets; capacity failure leaves the reply recoverable. Repeating
an acknowledgment with the same valid token consumes no new capacity or retention time.
The main-file cap defaults to 64 MiB and is rounded down to whole SQLite pages; it includes
reusable pages and excludes the transient DELETE rollback journal. Provision journal headroom
in addition to that cap: expiry of large payloads can journal much of the database, plus record
and header overhead. `journal_size_limit=0` does not cap an active transaction's journal.
Query work and statement subjournals stay in memory; incremental reclamation creates no full
database copy or other temporary disk file. The startup/daily library scheduler reclaims at
most 32 free-list pages per transaction, yielding between commits under a fixed sweep budget.
No new configuration setting is required. See
[disk maintenance](ARCHITECTURE.md#physical-disk-maintenance) for usage accounting, recovery,
filesystem bounds and limitations.
See [architecture](ARCHITECTURE.md#session-and-route-persistence).

The binary registers SIGINT and SIGTERM before service startup; either signal stops admission
and drains accepted sends and route bookkeeping before exit. Registration failure logs
`SHUTDOWN_SIGNAL_UNAVAILABLE` and exits unsuccessfully before starting services. No shutdown
setting is required.
Allow the existing network deadlines (15 seconds for text, 600 seconds for uploads), followed
by SQLite bookkeeping, to finish before any external forced-stop deadline. Repeated SIGINT or
SIGTERM does not abort the drain. SIGKILL, crashes and forced termination can still interrupt
bookkeeping after a send is accepted; do not automatically resend an uncertain message.

The library installs no process-wide handlers. Embedders supply cancellation and await
`app::run_until` to drain accepted work; dropping its future does not await detached sends.
`app::run` waits for a supervised service failure. See
[runtime and ownership](ARCHITECTURE.md#runtime-and-ownership) for shutdown ordering and limits.
