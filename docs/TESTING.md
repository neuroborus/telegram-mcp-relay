# Testing and Required Checks

## Repository gates

Run `just check-repo` (or `python3 tools/check_repository.py`) after documentation, skills,
navigation or repository workflow changes. It checks local Markdown link destinations, required
skill frontmatter, canonical symlinks, the tracked Runner starter configuration and ignored,
untracked local artifacts. It uses only Python 3.9+ and Git, no third-party modules. It checks
workspace files including untracked nonignored files; it does not validate external URLs,
Markdown anchors, full YAML schemas or the installed Runner's complete configuration schema.
Runner itself validates the operational configuration when starting a run.

During skill authoring, also use the available skill-creator `quick_validate.py` if installed.
Its environment-specific path is an operator detail, not a portable required command.
The repository gate is the committed skill/navigation baseline.

After the initial bootstrap commit, also run `git diff --check HEAD`. During the initial
unborn-HEAD bootstrap, inspect all new content and run `git diff --cached --check` only after
authorized staging. `.gitattributes` preserves the original specification's Markdown hard-break
spaces; other files retain normal Git whitespace checks.

## Rust gates

The root application crate makes Rust gates applicable. The complete dependency resolution
is in `Cargo.lock`; source, tests, Cargo/dependency or Rust tooling changes require:

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items
cargo test --locked --workspace
```

`just check-rust` runs these unchanged. `just check` runs the repository gate and, when the
manifest exists, the complete Rust gate; it explicitly reports non-applicability otherwise.
Commit `Cargo.lock` with the first manifest and use locked checks thereafter. Creating/updating
the lock is a deliberate implementation step, not an automatic repair performed by a check.
Run focused affected tests while developing as well; do not repeat broad gates after a clean
result without a subsequent change or unresolved concern.

The dependency-free `tools/runner-check/` crate is a separate workspace with its own lockfile.
Changes to it require all four helper gates as well as the applicable root gates:

```bash
cargo fmt --manifest-path tools/runner-check/Cargo.toml --all -- --check
cargo clippy --manifest-path tools/runner-check/Cargo.toml --locked --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --manifest-path tools/runner-check/Cargo.toml --locked --workspace --no-deps --document-private-items
cargo test --manifest-path tools/runner-check/Cargo.toml --locked --workspace
```

The helper's tests are pure or use private temporary files. They cover the closed mode/flag
mapping, explicit child toolchain/environment, preparation failure propagation, confinement,
private directory cleanup, selective registry provisioning, missing/corrupt archives and
SHA-256 vectors.
They never invoke the launcher recursively, start Cargo checks or run product/socket tests.

For the read-only Runner route, the six selected Cargo clippy/doc/test commands execute only
through Runner's exact persisted vectors during FINALIZE. Each vector builds the standalone
launcher in declared scratch and invokes its corresponding closed mode. The launcher runs
the complete original check with `--offline` and private target output; doc modes retain
`RUSTDOCFLAGS="-D warnings"`. Full root tests still execute the real HTTP assertions. Native
repository, whitespace and formatting checks remain part of FINALIZE's agent portion.
During implementation/review, use source inspection and appropriate no-run compilation;
do not execute or attest the established check inventory or selected logical commands.
See [development](DEVELOPMENT.md#read-only-cargo-validation) for provisioning and isolation.

## Behavioral acceptance

Binary tests in `src/main.rs` launch isolated test subprocesses using the binary's shutdown
signal registration. They send actual SIGINT and SIGTERM after a readiness gate, hold synthetic
work until an explicit parent release, and check that a repeated signal permits completion
before process exit. These tests cover signal delivery and continued execution during draining;
the composition tests below cover actual send/route persistence with mocked Telegram.
Signal-test execution remains pending the dedicated trusted FINALIZE suite; no signal test
installs handlers in the parent test process or uses live Telegram.

`app::tests` exercises the real daemon composition through modern MCP HTTP and a separate
mocked Telegram service. It checks completion of a multi-batch startup cleanup before polling,
shared registration/send/route/ingress/claim/processing/typing/ack state and durable reopen,
recovered factual notices, terminal webhook-conflict supervision and peer listener retirement.
Configuration/secret/bind failures start no services; a controlled SQLite write lock makes
startup cleanup fail before MCP serving or Telegram effects. Fixtures retain bound listeners
without port-reuse races and use private randomized TMPDIR-aware directories and explicit
HTTP response gates. `services::typing::tests` also verifies that worker abortion wakes
supervision without an idle timer. Shutdown cases hold a disconnected send across explicit
shutdown or terminal poll failure, then verify its route commits before return, ingress/cursor
survives reopening, and no reply is acknowledged or service notice generated.

`app::tests::conversations::three_sessions_route_interleaved_old_text_and_document_replies_after_rename`
adds a complete daemon conversation with three independently registered, same-title sessions
and overlapping sends whose HTTP responses are released in reverse request order. Later text and
document sends include a normalized rename; interleaved Replies to the earlier texts/documents
must retain their original session regardless of the latest activity or a misleading title in
the Reply body. Multiple Replies have distinct stable IDs and receipt order. An attachment-only
document and a captioned photo retain metadata through actual getUpdates, read-only MCP resource
snapshots and concurrent per-session inbox claims. Reopened storage retains all six composite
routes and seven unacknowledged Replies. After deleting the original outbound file, the fixture
contains only SQLite, with no outbound bodies, captions, quoted original text, file bytes or
local paths. The bounded mock accepts actual multipart uploads; response gates replace sleeps.
Execution remains pending the complete trusted FINALIZE suite.

`services::outbound::tests` holds admitted document responses across admission closure, proving
the drain waits for success, rejection, uncertainty and failed persistence, with released task/
send/route capacity and no replay. A synthetic post-send panic with a disconnected caller must
wake supervision and fail the drain. `services::typing::tests` closes a stalled worker and
awaits its retirement without advancing the controlled clock, then proves it cannot restart.
Notification tests close populated waiter/listener admission and retire existing interests.
`routes::tests::ask` closes interests before the initial wait recheck and proves a late reply
remains pending instead of being claimed after shutdown. A real HTTP ask and resource listener
retire while clients remain connected, and closed tool admission rejects later sends.
Existing transport shutdown and poller lease/cancellation cases cover stalled clients and
uncommitted input without repeating them.
These cases address shutdown and bounded-resource recovery in specification §§3, 6, 11, 16 and 19.
Execution remains pending the dedicated trusted FINALIZE suite; full-composition subscription,
ask and recovery acceptance interactions remain the next step.

`routes::tests` exercises real loopback HTTP discovery, native/same-origin clients, bearer auth,
hostile/duplicate headers, paths, missing/mismatched `Mcp-Method`, required version/capability
metadata, safe protocol failures and inclusive body bounds with declared/chunked input.
Test-only SDK handlers check shared
handler state across requests, safe correlation extensions, calls beyond 600 seconds, SSE
admission, disconnect release and transport shutdown using explicit gates/controlled time.
Fixtures are private, randomized and TMPDIR-aware. Execution remains pending FINALIZE;
these transport probes do not establish named client compatibility.

`routes::tests::tools` exercises the real shared handler through separate modern MCP HTTP
requests and a separate synthetic Bot API. It covers typed input/output discovery, stable
tool listings, normalized registration, external-ID/saved-handle resume through a new handler,
same-title isolation, text at the final scalar boundary and document captions at the native
boundary (including the default title-only caption). Successful sends return committed
composite text/document routes visible through a reopened database. Semantic validation
failures leave Telegram requests, routes and session activity unchanged. Malformed/unknown
arguments/tools remain JSON-RPC errors; application failures are structured tool errors with
redacted counts/guidance. Tool-call fixtures supply the required matching `Mcp-Name`; missing
and mismatched values retain the SDK header-error code/request ID without effects or input echo.
Rejection, uncertain delivery and post-send persistence failure remain distinct, without replay
or invented routes. Fixtures are private, randomized and TMPDIR-aware;
execution remains pending the dedicated trusted FINALIZE suite.

`routes::tests::inbox` adds actual modern MCP requests for stable template/empty resource
listings across registration, direct reads, tied-receipt pagination, session separation,
attachment-only payloads and delivery-state snapshots without tokens, claims or activity
changes. Concurrent inbox calls return distinct durable claims. Processing start/inactivity,
explicit reconciliation, a lost reconciliation response across a fresh handler/database,
stale/wrong-session fencing and idempotent per-reply completion use the shared transitions.
Invalid URIs, limits, listing cursors and tool arguments return safe errors without claiming.
Fixtures honor TMPDIR and use private randomized directories; no live Telegram calls are made.
Execution remains pending FINALIZE. These cases cover the read/claim/recovery portion of
specification §§6.3, 16 and 19; cross-feature ask interactions are described below.

`routes::tests::subscriptions` defines actual MCP 2026-07-28 POST SSE interactions through
locked rmcp 3.8.0 and the production handler. It covers accepted URI filters, omitted unknown,
malformed/template/paged URIs and unsupported categories, duplicate interests, empty/excessive
filters, SDK acknowledgment/update metadata, same-title fan-out and SQLite-backed reads.
An injected cursor failure proves rollback silence; duplicate, ignored, unauthorized and
unroutable ingress stays silent. Claims, processing start and completion invalidate snapshots;
stale tokens and idempotent retries do not. Explicit gates cover a commit after acknowledgment
but before listener setup, listener admission, disconnect retirement, queue overflow and
recovery of offline commits through a fresh handler/database. Pure queue tests independently
prove coalescing, slow-listener isolation, uninterested-session silence, drop-based admission
release and budget validation. Fixtures are private and TMPDIR-aware; no live Telegram calls are made.
These tests address subscription/reconnect and bounded slow-client criteria in §§6.1, 16 and 19;
their execution remains pending the complete trusted FINALIZE suite. Named-client
interoperability remains later acceptance work.

`routes::tests::ask` defines actual mocked Telegram sends and modern MCP calls alongside
resource subscriptions. It covers exact formatted question boundaries, independent waiter
admission, always-active events with waiting enabled/disabled, one durable reply shared by
ask/inbox/resource consumers, and an inbox claim winning before a later reply wakes the ask.
Manual future polling forces a reply to commit before wait setup through a separate database
connection without a signal, proving initial SQLite catch-up and question-target filtering.
Controlled time checks the default 600-second deadline and late-reply/restart recovery.
Explicit HTTP response gates and completion guards cover disconnect during and after sending,
released waiter admission and detached route bookkeeping. Invalid arguments/questions,
rejections and uncertain sends preserve zero-route/no-replay and safe-error behavior.
Injected claim failure preserves the committed question IDs and recoverable inbox payload,
reports inbox recovery without resend, and releases waiter admission.
These cases cover the ask/event/race, timeout and wait-cancellation portions of §§6.2–6.3,
16 and 19. Fixtures are private and TMPDIR-aware. Execution remains pending FINALIZE;
full-composition acceptance and named-client interoperability remain later work.

`services::typing::tests` uses controlled time for per-chat merging, independent session/reply
stops, cooldowns across stop/start races, the original 600-second cap across repeated starts
and reconciliation, disabled/unconfirmed/unapproved activity, bounded RAM and empty restart.
Controlled pending/ready request futures prove that a stalled chat cannot delay a healthy
chat's first action or refreshes. Wall-clock rollback followed by inactivity/resume or expiry
cannot reset a remembered monotonic cap; completion releases its bounded RAM admission.
`services::outbound::tests::typing` uses the real mocked Bot API and protected SQLite to cover
commit-before-action, failed/stale transition safety, a stalled action stopped by inactivity,
overlapping same-title sessions, accepted text/document sends stopping only their own session,
rejection preserving activity, explicit resume, reconciliation retry after resumed processing,
and expiry without completion or notices.
The client tests also assert single-attempt chat actions with safe rate-limit propagation.
These cases address typing criteria in §§7, 16 and 19. Execution remains pending FINALIZE;
no live Telegram behavior or runtime measurements are claimed.

Use the specification's complete section 16 matrix and [test-authoring](../.agents/skills/test-authoring/SKILL.md).
Pure domain tests cover grapheme title normalization, final scalar text limits and native
caption limits. Integration tests cover the interactions that pure tests cannot prove:

- Three concurrent sessions, same-title isolation and Replies to older text/documents.
- Durable Reply/cursor transactions, restart and duplicate ingestion, claim fencing,
  idempotent acknowledgment, capacity pressure and independent TTL cleanup.
- Resource reads, modern subscribed notifications and an active waiter together; cancellation,
  timeout, reconnect and recovery without duplicate claims or lost pending replies.
- Confirmed-processing typing, its non-resettable maximum and once-only unconfirmed notices
  with controlled time; long processing must not produce a warning.
- Mock Telegram failures/uncertain sends, authorization, safe path opening and near-limit
  streaming with bounded buffers. Preserve attachment metadata without downloading by default.

Default tests use local fake transports, synthetic IDs and temporary SQLite/files. Never require
real bot credentials or send real Telegram messages in CI. Optional live integration needs a
separate explicit user request and must disclose what client/API behavior it actually proves.

Configuration tests cover defaults/examples, bounds, allowlists, home expansion, redaction,
protected files/links/special files, bounded reads and non-overwriting bearer creation. They
inject environment lookups and clean private synthetic fixtures under `std::env::temp_dir()`.
Temporary fixtures must honor `TMPDIR`, use exclusive randomized mode-0700 directories and
clean up on drop. Tests must not write into the source checkout, including ignored directories;
the complete suite must also work from a read-only checkout.

Unit tests in `domain::outbound::tests` cover whitespace/control normalization, blank fallback,
50/51-grapheme titles (including combining marks, ZWJ emoji, flags and Hangul), unchanged
multiline bodies and shared plain headers. They exercise final 1,000/1,001-scalar boundaries
for ASCII, Cyrillic, combining marks and supplementary symbols, native 1,024/1,025-UTF-16-unit
captions including the title, unusually large title graphemes and redacted structured errors.
Text/question/notice formatting and caption validation are pure. Text/document services add
zero-send/zero-route assertions; question integration is covered by `routes::tests::ask`.

Storage tests in `tests/storage.rs` cover private initialization/reopen, persisted records and
cursor, foreign/future schema rejection without mutation, composite routes, distinct replies,
deduplication, valid claim shapes, notice-attempt uniqueness and restrictive foreign keys.
They verify that removing a route preserves replies and removing reply payloads preserves
acknowledgment receipts. Unsafe permissions, links, FIFOs and sidecars fail before database use.
Repository unit tests force mid-migration disk-full rollback and page-limit failure without
loss of committed rows. Fixtures use private randomized, TMPDIR-aware directories.
Storage tests use the locked bundled SQLite build and cover v1–v5-to-v6 migration preserving
replies, processing first start, cursor and uncertain notice attempts.

`tests/retention.rs` covers independent route/reply/session boundaries, processing recovery
after parent expiry and restart, retained valid-token receipts, admitted-send identity pins,
capacity recovery without early eviction, referential integrity and whole-batch rollback.
Large sweeps prove bounded progress past protected sessions and restart after partial cleanup;
queries exclude expired routes/replies/receipts before deletion and preserve the update cursor.
`services::cleanup::tests` uses paused time and explicit future polling to check startup,
the default daily interval, exclusive scheduling, shutdown and cancellation between batches.
Fixtures are private, randomized and TMPDIR-aware. These cases address logical retention in
specification §§10, 16 and 19; execution awaits the dedicated complete trusted FINALIZE suite.
Production startup ordering is covered separately by `app::tests` above.
`services::outbound::tests::typing` also combines actual mocked Telegram HTTP, processing and
cleanup with a single inbox slot: rollback preserves typing refreshes, and committed retention
deletion frees the expired cap so a fresh reply can start typing. Controlled time separates
typing expiry from retention; execution remains pending the complete trusted FINALIZE suite.

`tests/maintenance.rs` checks real file-length/allocated-block reduction in bounded incremental
steps, restart after partial reclamation and preservation of valid processing identity,
first-start time, routes, cursor and acknowledgment receipts. A SQLite main-page-cap failure
rolls back admission without losing a live reply; expiry/reclamation subsequently restores
capacity. Integrity and foreign-key checks accompany recovery. Repository unit tests inspect
an actual active rollback journal, compare descriptor-derived lengths/allocations, verify
removal on rollback and reject unsafe sidecars. They preserve FULL synchronization, DELETE
journaling, incremental auto-vacuum and in-memory temporary work. A held SQLite read lock
with zero busy timeout forces reclamation's commit to fail after page moves; rollback keeps
file size, free-list accounting and valid rows intact before a successful retry/reopen.
Scheduler tests explicitly poll futures to prove yielding, cancellation/restart and preservation of retained payloads
during physical maintenance, without timing sleeps. Fixtures are private, randomized and
TMPDIR-aware. Execution awaits the dedicated complete trusted FINALIZE suite; these cases
do not claim real filesystem ENOSPC, power-loss simulation or measured peak disk usage.
See [physical limits](ARCHITECTURE.md#physical-disk-maintenance).

`tests/notices.rs` covers exact receipt/grace and retention boundaries, ordered bounded batches,
claims without first start, confirmed long work/inactivity, direct completion and notice-first
versus start-first arbitration, including competing SQLite connections. It checks authorized
unroutable decisions, composite redelivery, retained-attempt deduplication, restart without a
success message ID, disabled categories and custom grace. Injected late insert/cursor failures
prove batch/ingress rollback and safe errors; shared receipt/metadata pressure and outstanding
send reservations preserve replies and unresolved input. Fixtures are private, randomized and
TMPDIR-aware, with explicit timestamps/barriers and no network calls. These cases address
the durable notice criteria in §§8, 16 and 19; dispatcher coverage is described below.
Execution remains pending the dedicated complete trusted FINALIZE suite.

`services::notices::tests` adds actual mocked HTTP delivery for both factual notice categories,
successful composite-ID persistence without ordinary routes/activity changes, definitive
rejection, ambiguous responses and failed success bookkeeping. Restart cases prove no replay
and silent service-target Replies using either retained IDs or bot-authored reserved wording.
They preserve ordinary routing even with matching wording, including Replies arriving before
an in-flight route commits, and distinguish human/other bot text and cross-chat ID collisions.
Complete 1,000/1,001-scalar boundaries include unusually large title graphemes, without
truncation or overlength network effects. Confirmed long work/inactivity, completion, disabled
categories and unauthorized/unrelated input remain silent. Explicit response gates and
controlled time check exclusive scheduling, one outstanding attempt, cancellation, lease
release and restart with a still-pending second notice. Fixtures are private, randomized and
TMPDIR-aware. These cases address notice dispatch/loop criteria in §§8, 16 and 19; execution
remains pending the complete trusted FINALIZE suite, with no live Telegram claim.

`tests/inbox.rs` covers read-only pagination, tied receipt ordering, attachment payloads,
session/composite-question isolation, simultaneous SQLite claim consumers, restart/lease
boundaries, fresh fencing tokens, retained processing ownership and independent reply expiry.
It also covers item/payload bounds, token metadata admission with send reservations, whole-batch
rollback and safe errors. Fixtures are private, randomized and TMPDIR-aware; timestamps are
injected and contention uses a barrier without sleeps. Execution remains pending FINALIZE.

The same suite now covers idempotent first processing start, inactivity without releasing
ownership, stale/wrong-session tokens, exact claim/reply/receipt expiry boundaries and direct
valid-claim completion. Concurrent resume reconciliation across SQLite connections has one
compare-and-swap winner and preserves first start/retention through restart and registration.
Lost reconciliation responses recover from the saved request ID and prior token after restart,
even at capacity; mismatched and superseded requests remain fenced. Request IDs are bounded,
and proof replacement/removal participates in metadata admission with outstanding sends.
Lost acknowledgment responses retry from minimal receipts without extending retention;
receipts suppress redelivery after cursor advancement. Shared receipt/metadata pressure and
outstanding send reservations preserve recoverable replies. Injected update/delete failures
roll back first start, token rotation and receipt/payload replacement with redacted errors.
These cases await the dedicated complete trusted FINALIZE suite; no execution is claimed here.

`tests/ingress.rs` covers atomic reply/activity/cursor persistence, same-title cross-chat routes,
multiple replies, attachment-only documents, photo captions, receipt-based TTLs, long incoming
text, redelivery and restart after commit before signaling. Idle lower-ID resets and retained
redeliveries across restart prove exact deduplication without numeric high-water assumptions.
Unauthorized floods compact safely even with full receipt/variable metadata budgets, and a
subsequent authorized reply remains admissible. Tests also check safe unroutable dispositions,
authorization, route expiry, serialized metadata bounds, shared item/payload/
metadata admission including outstanding sends, late transaction failures and actual SQLite
page exhaustion without losing accepted replies or advancing unresolved input. Fixtures are
private, randomized and TMPDIR-aware. Execution remains pending the trusted FINALIZE suite.

`services::polling::tests` combines actual mocked Telegram HTTP and protected SQLite for
ordered batches, default 45-second polling, durable restart offsets, redelivery deduplication,
post-commit change signals and a shared exclusive consumer lease. Explicit HTTP gates cover
cancellation, supervisor loss and Replies arriving before a detached send's route commit or
definitive rejection. A gated regression keeps a newer send outstanding while an earlier send
settles, proving that an unknown Reply is classified and a following routable Reply commits
without waiting for the newer admission. Capacity tests preserve the unresolved suffix across
cancellation and restart, and resume the retained batch after synthetic capacity release. Webhook/409 conflicts
and malformed batch order/IDs leave the cursor unchanged; no destructive API is called.
Controlled-time tests cover bounded jittered backoff, unshortened large `retry_after` delays
and cancellation during retry waits. Fixtures are private, randomized and TMPDIR-aware.
Execution remains pending the complete trusted FINALIZE suite.

`tests/registry.rs` exercises three same-title identities, interleaved text/document routes,
cross-chat message-ID collisions, external-ID and saved-handle resume, normalized renaming and
restart continuity. Explicit timestamps cover session activity and independent route expiry.
Count/UTF-8 metadata limits include in-flight reservations and expired retained rows; rejected
renames leave activity unchanged. Tests also cover cancellation release, route conflicts,
transaction rollback after a route insert, invalid inputs and redacted diagnostics. These are
repository operations; focused MCP registration/send integration is in `routes::tests::tools`.

`services::outbound::tests` exercises the actual HTTP client and SQLite together: unknown
sessions, invalid destinations, exact formatted 1,000/1,001-scalar ASCII/astral boundaries,
route/metadata pressure, definitive rejection, lost responses and failed post-send activity
transactions. It checks no effects on validation failures, no automatic replay or fabricated
routes, redacted errors, atomic rollback, released admission and restart-visible composite
routes with seconds-to-milliseconds conversion. An explicitly gated HTTP response allows
caller cancellation while a send remains admitted; competing sends stay bounded and the
detached task finishes durable bookkeeping. Fixtures honor TMPDIR and use private randomized
directories. These new tests are pending the complete trusted FINALIZE suite; implementation
and source review do not attest their execution.

`services::outbound::tests::documents` adds actual multipart HTTP/SQLite scenarios: complete
caption scalar/native rejection without effects, exact 1,000-scalar/1,024-UTF-16-unit captions,
title-only captions, binary/empty and multichunk documents, growth and replacement after
validated opening, restart-visible document routes, shared storage/task pressure, definitive rate-limit/rejection,
lost/malformed/mismatched responses, post-send rollback, truncation, deadline and cancellation.
The tests check no replay, no invented routes, released permits/reservations and no outbound
bytes/captions in SQLite. Explicit response gates and clock advancement avoid timing sleeps.
`clients::telegram::upload::tests` consumes a sparse near-cloud-limit file incrementally,
checking bounded lazy chunks, changes after the first read, growth exclusion, early EOF,
read errors and empty snapshots without whole-file buffers. These tests await the complete trusted FINALIZE
suite; no implementation-turn execution or resource measurement is claimed.

`services::files::tests` covers descriptor-confined regular/empty/nested files, multiple
roots, exact configured/cloud decimal-byte boundaries, traversal and prefix collisions,
missing/unreadable files, directories, FIFOs, Unix sockets and root/ancestor/leaf symlinks.
It checks root and leaf replacement after opening and concurrent atomic swaps between
regular paths and escaping symlinks: accepted handles always read the approved file.
Filename cases include controls, header punctuation, Unicode, non-UTF-8 and long basenames;
errors and Debug output stay path-free. Fixtures are exclusive, private, randomized and
TMPDIR-aware; size tests use sparse files, without allocating near-limit buffers. These tests
await the dedicated complete trusted FINALIZE suite; no execution is claimed here.

Unit tests in `clients::telegram::tests` use a synthetic loopback HTTP peer with explicit
disconnects, malformed/oversized and chunked bodies, redirects, webhook conflicts, rate limits
and server errors. They cover request JSON, typed reply/attachment metadata, pre-I/O outbound
validation, redaction, bounded retries and no replay of ambiguous sends. A test-only retry
recorder replaces sleep and asserts the actual chosen delays; a paused Tokio clock covers a
response stalled after headers. Mock tasks are canceled on fixture drop. The complete,
unfiltered `cargo test --locked --workspace` suite passed in Runner FINALIZE through the
read-only trusted route, exercising all seven HTTP tests and the pure retry-policy test
without skips. Earlier Native attempts failed before HTTP requests at loopback `bind` with
`Operation not permitted`. Live Telegram behavior remains untested. See
[architecture](ARCHITECTURE.md#telegram-http-client).

## Specification §§16/19 coverage map

This maps observable contracts to source-defined proofs, not check receipts. Current changes
await the complete trusted FINALIZE suite. Focused proofs remain in their owning suites;
the conversation scenario adds their full-daemon interaction without repeating boundary cases.

| Acceptance / traceability contract | Owning proof |
| --- | --- |
| Three concurrent sessions; same-title identity; normalized rename; old text/document Replies; multiple distinct ordered Replies (§§4.1–4.3, 16, 19) | [Daemon conversation](../src/app/tests/conversations.rs), `three_sessions_route_interleaved_old_text_and_document_replies_after_rename`; [registry tests](../tests/registry.rs) retain cross-chat message-ID collision and identity/restart proofs |
| Attachment-only document and photo-caption metadata; resource reads and inbox claims preserve original-session routing (§§6, 9, 16, 19) | The daemon conversation above; [ingress](../tests/ingress.rs) and [MCP inbox](../src/routes/inbox.rs) retain authorization, rollback, pagination and fencing boundaries |
| Title graphemes, final scalar/native caption limits, structured rejection without splitting/truncation or effects (§§4.2, 9, 16, 19) | [Outbound domain tests](../src/domain/outbound/tests.rs), [MCP tools](../src/routes/tools.rs) and [ask tests](../src/routes/ask.rs) |
| Streamed, confined, Reply-routable files without retained outbound history/copies (§§9, 12, 16, 19) | Daemon conversation checks actual multipart contents, routes and persisted files; [file tests](../src/services/files/tests.rs), [document tests](../src/services/outbound/documents.rs) and [upload tests](../src/clients/telegram/upload.rs) retain path-race, mutation and bounded-read proofs |
| Unknown/expired routes never reassigned; unauthorized input excluded; durable atomic cursor (§§4.3, 10–12, 16, 19) | [Ingress](../tests/ingress.rs), [retention](../tests/retention.rs) and [poller tests](../src/services/polling/tests.rs) |
| Always-active events, optional bounded wait, shared reply/claim ownership, timeout/disconnect safety (§§5–6, 16, 19) | [Subscription tests](../src/routes/subscriptions.rs) and [ask tests](../src/routes/ask.rs); remaining full-composition race/recovery acceptance is the next step |
| Durable claims, first processing start, reconciliation, acknowledgment and reconnect recovery (§§6, 10–11, 16, 19) | [Inbox storage tests](../tests/inbox.rs) and [MCP inbox tests](../src/routes/inbox.rs); daemon shutdown tests retain accepted ingress and detached-send proofs |
| Confirmed capped typing, no idle refresh, factual once-only notices without loops (§§7–8, 16, 19) | [Typing tests](../src/services/typing/tests.rs), [notice storage tests](../tests/notices.rs) and [notice dispatch tests](../src/services/notices/tests.rs) |
| Independent TTLs, bounded cleanup and physical reclamation without losing valid replies (§§10–11, 16, 19) | [Retention](../tests/retention.rs), [maintenance](../tests/maintenance.rs) and [cleanup scheduler tests](../src/services/cleanup/tests.rs) |
| Protected single-process composition and local-only transport (§§1–3, 12, 16, 19) | [Application tests](../src/app/tests.rs) and [HTTP transport tests](../src/routes/tests.rs); measured resource targets and operational/client evidence remain pending |

## Measurements and check ownership

Release acceptance records Linux target/toolchain, stripped locked release binary size,
dependency profile, startup time, idle RSS/CPU and peak RSS during a near-limit streamed upload.
Use an identified reproducible workload and state whether Telegram is mocked. The sub-30 MB
idle target is best effort, not an unmeasured guarantee or a brittle unit-test assertion.
Add the measurement command and owning document when the implementation provides them.

Native Runner commands use the accepted staging-independent inventory and
[Runner finalization](../.agents/skills/finalization/agent-runner.md). Staged inspection belongs
only at COMMIT/HANDOFF. Assign unavailable HOST measurements explicitly in task gates; they
remain mandatory final acceptance evidence after the run and never become fabricated passes.
Observe actual network/cache/socket failures before classifying a capability blocker. Report
the command, cause and required owner; do not weaken a check or substitute an unrelated test.
