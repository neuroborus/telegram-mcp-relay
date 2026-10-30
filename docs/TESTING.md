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
zero-send/zero-route assertions; question service integration remains pending.

Storage tests in `tests/storage.rs` cover private initialization/reopen, persisted records and
cursor, foreign/future schema rejection without mutation, composite routes, distinct replies,
deduplication, valid claim shapes, notice-attempt uniqueness and restrictive foreign keys.
They verify that removing a route preserves replies and removing reply payloads preserves
acknowledgment receipts. Unsafe permissions, links, FIFOs and sidecars fail before database use.
Repository unit tests force mid-migration disk-full rollback and page-limit failure without
loss of committed rows. Fixtures use private randomized, TMPDIR-aware directories.
Storage tests use the locked bundled SQLite build and cover v1-to-v2 migration preserving
replies/cursor. Claim arbitration and cleanup tests land with their services.

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
repository operations; MCP send integration remains later work.

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
