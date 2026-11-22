# Testing and Required Checks

## Agent-experience composition validation

Accepted commit `e7575f2636480b35fa1da75134e337fadb9a4a00` completes the source/composition
portion of the agent-experience continuation: bounded cloud downloads, confined owned storage,
authorized attachment access, current guidance and extended mixed-protocol fixtures. Its
fingerprint-bound FINALIZE and terminal confirmation accepted the unchanged committed content.
This record covers the versions of those suites at that commit. It does not inherit a PASS
from the historical `968802` benchmark or require a fresh benchmark.

| Executed gate | Accepted result and boundary |
| --- | --- |
| C1 `python3 tools/check_repository.py`, C2 `git diff --check HEAD`, C3 `cargo fmt --all -- --check` | Native commands exited 0 for the composition candidate |
| C4 `cargo clippy --locked --workspace --all-targets -- -D warnings` | Runner-trusted `rust-workspace-clippy`, exit 0 |
| C5 `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items` | Runner-trusted `rust-workspace-doc`, exit 0 |
| C6 `cargo test --locked --workspace` | Runner-trusted `rust-workspace-tests`, exit 0, complete unfiltered workspace suite |
| R1 `cargo build --locked --release --bin telegram-mcp-relay` | Native locked daemon release build, exit 0 |
| R2 `cargo build --locked --release --example resource_probe` | Native locked library-linked probe release build, exit 0 |

At that baseline both advertised protocols exercised the eleven-tool inventory and DTO schemas
then in use, original security/lifecycle assertions, exact modern `resultType:"complete"`
versus legacy omission, native framing, wait/event/inbox/ask claims, recovery/fences and
authorized shared/own downloads. The [extended composition fixture](../src/app/tests/extended.rs)
describes the test structure. The recorded synthetic results apply to the baseline cases.
Named-client, live Telegram, deployed-filesystem, idle-wake and fresh resource-use evidence
have separate owners. The later documentation steps in that continuation have their own
recorded check outcomes below.

## Agent-experience continuation status

On **2026-11-01**, the final-eight execution reached actual **DONE**, revision **1374**, with
idle execution, no lease owner and no process record. Starting from overall **7/15** at
`7412bf7851f8ea8ff8f4f3af0a8d88c948b2981c`, all **8/8** continuation commits are accepted:
overall source progress **15/15**, ending at `70fedf8471b8981a802fa0085d527f6bd5d35645`.
The finalization/confirmation fingerprint is
`89644d77e3b18f0fcf0492632d3ca4a060105ecca05efd62153d95b6bc24b8a2`.
Source completion and released ownership are separate from the dated HOST observations and
post-release documentation/workflow acceptance below.

| Step | Accepted source commit | Recorded FINALIZE result |
| --- | --- | --- |
| 1. Bounded cloud downloads | `cabc612c2816671e84ec4a381e95ae52b76e757f` | C1–C3 Native and C4–C6 selected Runner checks exited 0 |
| 2. Confined owned storage | `6ada6dbe5bcf00405fbef416cc316cdfb0a2eea3` | C1–C3 Native and C4–C6 selected Runner checks exited 0 |
| 3. Authorized attachment tool | `cbea9f1d1a9ca602f91ee8e45090617d60e86f99` | C1–C3 Native and C4–C6 selected Runner checks exited 0 |
| 4. Current interaction guidance | `3489a604af7e55f402b20e2d158944b0158d2188` | C1–C3 Native and C4–C6 selected Runner checks exited 0 |
| 5. Mixed-protocol composition | `e7575f2636480b35fa1da75134e337fadb9a4a00` | C1–C6 plus both Native locked release builds exited 0, as detailed above |
| 6. Public workflow documentation | `a4aea0e4c6a64cbe593ac9c5a4fb37b9a978879a` | Native C1/C2 exited 0 |
| 7. Inline Runner setup | `9f45f3e96090a3e24378ad7d3eb4395f7fc1c55f` | Native C1/C2 and Python AST parsing exited 0 |
| 8. Self-contained evidence documentation | `70fedf8471b8981a802fa0085d527f6bd5d35645` | Native C1/C2 exited 0; lazy finalization/confirmation accepted the exact committed content |

These are the supplied accepted per-step FINALIZE/terminal-confirmation records, followed by
the constrained executor's commits. They are not new check runs in this documentation turn
or inherited benchmark PASS claims. Step 5's actual receipt IDs **C7/C8** denote R1/R2
(global **C11/C12**); step 7's actual **C3** denotes P1 (global **C13**), not Rust formatting.
Keep the historical independent capture's different receipt mapping below scoped to that run.

The final-eight schedule retains exactly the existing commands and applicability:

| Established catalog checks | Applicable continuation steps | FINALIZE route |
| --- | --- | --- |
| C1 repository gate; C2 `git diff --check HEAD` | 1–8 | Native |
| C3 Rust formatting | 1–5 | Native |
| C4 Clippy; C5 rustdoc; C6 complete unfiltered workspace tests | 1–5 | Unchanged selected `rust-workspace-clippy`, `rust-workspace-doc`, `rust-workspace-tests` vectors |
| C11/R1 daemon release; C12/R2 probe release | 5 | Native locked builds |
| C13/P1 Python AST parsing | 7 | Native |

P2 and H1–H4 remain conditional and inactive in this continuation. No helper or measurement
Python implementation changes are assigned. Discovery rows contain only `id`/`command`;
separate applicability assignments contain only `checkId` and nonempty `steps` in the supplied
unit. Preserve the full accepted schedule; a current-step amendment cannot erase future gates.
Run established gates only in FINALIZE. IMPLEMENT/fresh read-only confirmation permits source
inspection and appropriate no-run compilation, not those checks or socket tests. Do not skip,
filter, replace or weaken a gate, execute a selected vector inside an agent turn, or substitute
HOST results. Content repairs invalidate earlier candidate evidence.

This explicitly lazy continuation uses installed convergence and fresh read-only confirmation
at each step. Lazy confirmation is not independent code review and does not replace full
applicable FINALIZE or terminal confirmation. Staging/index hygiene and the subject-only commit
belong to the constrained COMMIT executor. The completed-run results do not validate later
documentation edits or imply unobserved live behavior. The dated
[HOST record](MEASUREMENTS.md#agent-experience-host-acceptance) and post-release workflow have
their own evidence and acceptance boundary.

### Release follow-ups

After actual DONE and released/quiescent ownership, the coordinator installed the accepted
release, restarted one shared process with preserved SQLite/private settings and narrowly
enabled the requested history/downloads. The
[2026-11-01 HOST record](MEASUREMENTS.md#agent-experience-host-acceptance) owns the actual
inventory/protocol, native-send, wait, shared-history and attachment observations and their
limits. Retain canonical-unit hardening and unresolved `218/CAPABILITIES`; static verification
is a separate historical fact. Neither old text trials nor synthetic checks establish these
later outcomes or general host concurrency/idle wake.

Run-frozen guidance remains untouched while Runner owns the workspace. This authorized
post-release documentation follow-up reconciles `AGENTS.md`'s map/policy, current modules,
guidance-first interaction, native-caption limits, waits, observational history and confined
attachments. [Delegation](../.agents/skills/delegation/SKILL.md),
[finalization](../.agents/skills/finalization/SKILL.md) and supporting skills/references preserve
independent/current/auto defaults while honoring explicit modes, including lazy, without false
independent-review claims. Discovery and per-unit applicability retain their separate schemas.
Portable Runner setup is inline; no tracked JSON template is restored. This follow-up needs
its own applicable documentation/skill checks and required review/confirmation before commit;
it does not inherit the completed product run's checks or add per-step product review.
Finish required HOST/evidence/workflow acceptance before publication. Never edit frozen guidance
or active configuration/state during ownership.
Historical canceled/paused runs, receipts
and counters remain intact; do not resume them, reset budgets or manually edit state/leases.

The [dated baseline HOST record](MEASUREMENTS.md#dated-baseline-host-observations) publishes
the limited pre-extension text trial without private receipts. The
[extension HOST record](MEASUREMENTS.md#agent-experience-host-acceptance) owns installation,
bounded probes and sanitized outcomes. After actual DONE and completed HOST/
follow-ups, the coordinator reports final evidence and publishes the related compatibility,
all fifteen experience and authorized cohesive correction commits. Workers never push; no
early/force push, new branch, remote/identity change or history rewrite is authorized here.

## Native header and service-prefix regressions

The composition baseline covered bold/italic UTF-16 spans, literal bodies, optional
100-scalar directory metadata, complete 1,000-scalar/1,024-unit caption limits,
metadata admission and v1–v6 migration/restart continuity. Domain tests own Unicode/entity
boundaries; registry/storage tests own persistence; MCP and outbound/document fixtures inspect
actual JSON/multipart entity fields. Notice fixtures retain durable grace/processing arbitration,
exact `🔧 ` framing, known-ID suppression and lost-ID recognition of both new and legacy notices.
The bounded Python mock now accepts the ordered `caption_entities` part and validates native
span boundaries without buffering documents. The accepted composition C6 above covers the
original cases. The current sources define the required project line for text, questions and
captions, with the same final limits. Live rendering remains unverified.

The RELAY-003/004 cases in `src/routes/tools.rs` exercise registration and recovery without
fetching guidance on both MCP revisions. They cover blank and placeholder titles, path prefixes
after opening wrappers with or without closing delimiters and trailing prose, paths later in
valid titles, required project labels, private retention of invalid saved labels on omission
and effect-free legacy rejections before file opening or waiter admission. Recovery
under either stable identity keeps routes and pending replies and preserves the existing claim.
Domain cases cover invisible formatting, supported Unicode wrappers, relative labels and
bracketed placeholders, including legacy cutoffs and placeholders exposed by title truncation.
The notice fixture checks safe framing, native spans, journal admission and authorized reads of
new incoming and service snapshots on synthetic routes. The earlier native-framing scope
assigned P1/P2 to Native FINALIZE for its Python measurement fixture changes. The
[interaction acceptance record](#interaction-rules-acceptance) records C1–C6 for the updated
RELAY-005 framing fixtures.

## Server instructions and restored framing regressions

`src/routes/guidance.rs` checks standard server instructions in actual legacy initialization
and modern discovery before any guidance/tool call: matching policy, 500-scalar bound, essential
recommendations within 512 UTF-16 units, unchanged SQLite and zero Telegram requests.
Domain cases protect bold brackets, separate italic directory, exactly one blank line before
nonempty bodies, empty captions, cutoff ellipsis inside 50 whole graphemes and fitting-title
round trips. `tests/registry.rs` reopens new ellipsis and old already-truncated labels without
inventing lost input. Notice cases retain old plain/bracketed/native shapes and route precedence.
Actual text/question JSON and document multipart fixtures include bracketed astral headers,
shifted body entities, exact 1,000/1,001-scalar and 1,024/1,025-UTF-16 boundaries, final-text
journaling and mixed-protocol document/wait composition. The recorded mixed-protocol
composition baseline included the C11/R1 and C12/R2 locked release builds. The
[interaction acceptance record](#interaction-rules-acceptance) records the accepted integration and
the operator's release-build diagnostics for the changed server-instruction and framing
fixtures.

## Explicit native body-entity regressions

Domain cases cover supported kinds, metadata bounds, surrogate/range/overflow failures,
containment/overlap and code/pre/quote restrictions. MCP fixtures on both revisions define
rich send/ask JSON and document multipart entities, body-relative UTF-16 shifts with astral
headers/directories, exact protocol envelopes, nonempty legacy-array compatibility and
visible-text journaling. Invalid schemas, metadata, ranges and nesting leave HTTP sends,
routes, journal and activity unchanged; saturated waiter capacity cannot mask invalid questions.
The existing cancellation/rename/restart journal fixture also sends explicit entities.
The [interaction acceptance record](#interaction-rules-acceptance) records their C1–C6 results.

## Strict Markdown regressions

The [parser tests](../src/domain/outbound/markup.rs) define escapes, exact star-run matching,
whitespace and delimiter rejections, emphasis in link labels, UTF-16 spans with astral
characters and literal list markers. Link cases cover mixed-case schemes, escaped
parentheses and rejection of missing authorities or extra slashes immediately after `://`.
The [MCP fixtures](../src/routes/tools.rs) use clients that skip guidance and an isolated
synthetic Telegram peer on both revisions. They inspect rendered status text, questions and
file captions, including bold spans separated by punctuation and rejection of shared
delimiters. They cover explicit plain text and byte-identical legacy entity sends, including
the prior behavior for native link URLs. Invalid Markdown returns a redacted scalar offset
before file opening, waiter admission, Telegram, route, journal or activity effects. The
missing-path and saturated-waiter cases require `TEXT_TOO_LONG` for overlong rendered
captions and questions, including the native caption limit. The modern boundary fixtures
check the maximum accepted lengths.

The [interaction acceptance record](#interaction-rules-acceptance) records their C1–C6 results.
The composition baseline covers its original fixtures.
After DONE and workspace release, the supervising operator owns pending HOST acceptance of
formatted output in a real agent workflow. The resource probe driver caption
`Synthetic near-limit upload.` is unchanged by default Markdown.

## After-send wait regressions

After-send wait coverage in `src/routes/wait.rs` defines text/document catch-up without signals
or resending across both protocols, shared ask/wait admission and single claims, target/chat/
ownership/expiry rejection, disabled outcomes, and cancellation/600-second timeout/shutdown with
late-reply restart recovery. The saturation fixture holds the Telegram response to a first
processing start while independent processing inactivity and acknowledgment calls complete.
Forged headers cannot route an ordinary send through reserved capacity. Existing ask/event
arming-race cases exercise the shared wait loop. The accepted composition C6 covers the
original cases.

The combined send-and-wait fixtures define both protocols' file approval, late inbox recovery,
disabled waiting, post-send admission failure with recovery using the saved outbound ID, and
the original one-way result when the flag is omitted. After waiter admission, the file approval
fixture checks the `sendDocument` request and multipart document bytes, then injects the reply.
The timeout fixture advances Tokio's clock after admission.
The modern disconnect regression waits for admission before closing the socket, then checks
that the slot is available and the late reply stays pending. The legacy cancellation regression
holds Telegram's document response until MCP work admission is released, then checks that the
route commits without arming a waiter or resending. The
[interaction acceptance record](#interaction-rules-acceptance) records their execution. Real-agent
acceptance remains operator-owned after DONE and workspace release.

## History storage regressions

History storage fixtures in `tests/history.rs` define authenticated opt-in, stable opaque
handles/display snapshots, redaction, deduplication/restart, independent expiry, source
provenance after disabling/acknowledgment, admission rollback and bounded cleanup/reclamation.
Storage migration coverage preserves v1–v8 recovery data through v9 and checks transactional
rollback of failed migrations. Configuration tests reject unsafe/inconsistent history
budgets. Authorized attachment access has separate source-defined coverage below.

Incoming observation cases in `tests/ingress.rs` define authorized standalone text/document/
photo capture, absence of unauthorized/unsupported content, unchanged notice/loop/route
arbitration, failure at journal/attachment/cursor boundaries, whole-transaction capacity rollback,
and source deduplication across restart/lower IDs and completion with history disabled.
Existing photo fixtures now include actual photo dimensions; v1–v8 migration fixtures preserve
older recovery state through v9. The accepted composition C6 covers the original cases.
Live bot coverage remains separate.
Reused-ID regressions cover distinct standalone/routed/unroutable sources, tied inbox pages,
separate notice decisions and acknowledgment receipts, rollback and restart. Actual v8-schema
fixtures preserve reconciliation/token grants and attachment references through the key rebuild,
including late-DDL rollback. Retention fixtures bound equal-ID deletion without removing live rows.

Retention recovery assertions track private attachment provenance with history disabled: its
source/ownership and independent reply deadline, orphan removal after acknowledgment, capacity
recovery, and exact accounting/foreign-key rollback across restart. Scheduler coverage awaits
complete startup readiness before advancing paused time, including any yielding reclamation.
The accepted composition C6 covers the original cases.

## Confirmed outbound journal regressions

`src/services/outbound/history.rs` defines opted-in text/document capture, admission-time display
snapshots during rename/cancellation/draining, returned metadata, no source paths/bytes, deduplication,
restart and independent expiry. Its rejection/uncertainty/SQL/metadata-failure cases preserve route,
activity, coverage and capacity rollback without replay. Four-budget races cover other sends and
incoming observation/cursor rollback. Notice cases define service context, atomic success-ID/journal
rollback, pressure before attempt consumption, cancellation and restart-safe no-replay behavior.
The accepted composition C6 covers the original cases. Live acceptance remains separate.

## Shared history read regressions

`tests/history/read.rs` defines cross-session observation with unchanged processing/fencing/
reconciliation, session, usage and coverage state, private-field exclusion, current-policy
revocation, tied keysets, sparse continuations, expiry/cleanup, filter-bound/expired/malformed
bookmarks, complete response-budget pagination and bounded search work. Authenticated HTTP
cases in `src/routes/tools.rs` require bearer access through both protocols and preserve literal
bodies. Result assertions require equal structured payload, content and error state, plus the
modern `resultType: "complete"` and its legacy omission, rejecting other envelope differences.
Catalog fixtures bind the complete expected inventory to full DTO-generated input/output
schemas while retaining their authorization and lifecycle checks. The accepted composition C6
covers the original cases. Live retrieval remains separate.

## Private cloud download regressions

`src/clients/telegram/download/tests.rs` defines synthetic getFile/GET workflow proofs for
retained stable identity, renewed file IDs and safe path/origin validation, bounded metadata,
declared and actual size limits, exact 20,000,000-byte streaming, lower configured bounds,
absent/false sizes, unavailable/expired paths, redirects, partial/encoded responses and truncated
bodies. Writer failure/backpressure, cancellation and whole-operation deadlines preserve safe
errors without automatic replay. Generated chunked bodies use bounded buffers; no live credentials or API
requests are needed. The [cloud composition baseline](#agent-experience-composition-validation)
records the complete C6 result for its version of this suite. The separate
[2026-11-01 HOST trial](MEASUREMENTS.md#agent-experience-host-acceptance) verified one
authorized same-host 44-byte cloud round trip. Owned-file cleanup and MCP authorization have
separate coverage below.

## Private download storage regressions

`src/services/downloads/tests.rs` defines private-root/permission/lock checks, real adapter-to-
reserved-inode composition, byte/file/concurrency/disk reservations, partial-write and size
failure, caller cancellation, pending blocking-I/O drainage, shutdown, admission-time deadlines,
bounded maintenance and expiry. Confinement cases replace the root during a write and preserve
foreign symlinks, hard links and colliding
names. Restart cases cover reservation-only, unnamed inode, linked partial and retiring-name
crash points while preserving valid completed files and unknown files. Configuration cases
reject inconsistent budgets and source/secret/storage overlap. Storage migration cases preserve
older recovery data across schema upgrades, including v9 rollback after late DDL failure. The
[cloud composition baseline](#agent-experience-composition-validation) records complete C6 for
the pre-RELAY-001 versions of these suites. The separate
[2026-11-01 HOST trial](MEASUREMENTS.md#agent-experience-host-acceptance) verified one authorized
same-host 44-byte cloud round trip. The resulting mode-0600 file on the deployed pinned root
matched the returned device and inode.

### RELAY-001 download reservation accounting

The deterministic regression feeds the reported 20,004,864-byte ext4 allocation into the
writer's allocation check. It exceeds the old 20,000,768-byte ceiling and fits the revised
metadata-inclusive reservation. The storage tests cover an allocation above the revised ceiling,
exact logical quota with `max_total_bytes == max_file_bytes`, completed payload accounting,
disk headroom including pending reservations, inode and file-count limits, and concurrency.
Pure sizing cases cover gigabyte-scale values and overflow without creating large files.
The [interaction acceptance record](#interaction-rules-acceptance) records the passing
full workspace suite. After DONE and workspace release, the supervising operator owns the
pending HOST MCP attachment download on ext4 under conditions that reproduced the old
accounting failure.

### RELAY-002 local Bot API downloads

Synthetic fixtures cover path confinement, root replacement, rejection of special files,
size mismatches, a sparse file above 1 GiB and bounded copy chunks. A controlled peer completes
the original getFile response after the normal request timeout and tests that a stalled response
closes at the transfer deadline without a retry. Owned-store tests
cover partial-copy cancellation and expiry, admission held through blocking source I/O,
reservation sizing and v10-to-v11 migration with populated ownership records at the SQLite
page cap. Configuration and guidance tests cover backend-specific ceilings and the effective
download limit. Retention tests exercise local-sized accounting without a large allocation.
The attachment MCP fixture exercises local retrieval on both protocol revisions without
Telegram credentials. It rejects a file that exceeds the retained reservation even when
getFile supplies no size. The [interaction acceptance record](#interaction-rules-acceptance)
records C1–C6 for the integrated result. After DONE and workspace release, the supervising
operator owns the pending HOST transfer of an incoming gigabyte-scale file through MCP and an
authorized live local Bot API. Record the streamed size and memory use separately from the
synthetic results.

## Authorized attachment tool regressions

Attachment authorization regressions in `src/services/attachments/tests.rs` cover own versus
shared policy, forged/foreign/expired handles, insufficient legacy provenance, source changes,
queued-worker revocation, observational projections and revoked chat/user/history authorization.
`src/routes/attachments.rs` defines actual tool/resource/claim workflows over both protocols,
strict arguments and the complete independent DTO catalog, confined generated file identities,
untrusted display names, exact result envelopes and durable-data preservation after an upstream
failure. Storage tests retain bounded cancellation/shutdown/restart cleanup and add unreceived
completed-result cancellation and writer-panic supervision/drainage. The recorded attachment
C6 and locked release results cover the prior cloud baseline identified in the
[composition validation record](#agent-experience-composition-validation). The preceding
RELAY-002 section and the [interaction acceptance record](#interaction-rules-acceptance)
cover synthetic local retrieval and its executed validation.

## Current interaction guidance regressions

`src/routes/guidance.rs` defines pre-registration guidance over both actual protocols, complete
catalog/DTO bindings, guidance-first descriptions, read-only annotations, strict empty arguments,
exact modern/legacy envelopes, unchanged SQLite bytes and zero Telegram effects. A fresh client
can still register without a handshake; later guidance discloses no new private session data.
`services::guidance` tests nondefault lower limits, disabled features, database history opt-in
and exclusion of synthetic private settings/paths/allowlist identities. Self-contained policy
content follows specification §§5.4/14. The earlier version-1 cases passed in accepted
complete C6 at the [composition baseline](#agent-experience-composition-validation).
Version 6 adds assertions for compact instructions, tool and field descriptions, and full
policy examples. The [interaction acceptance record](#interaction-rules-acceptance) records their
execution at `e3c7fab`. Version 7 adds source-defined assertions that guidance names the
configured start acknowledgment in each mode; they await their Runner FINALIZE receipts.
Source inspection does not establish named-client, live Telegram, host concurrency or idle wake.

## Interaction rules acceptance

| Evidence | Status and scope |
| --- | --- |
| Executed synthetic composition baseline | The [accepted baseline](#agent-experience-composition-validation) ran the original daemon cases with ordinary MCP HTTP clients, an isolated synthetic Telegram peer and both protocol revisions. Its C6 and locked release results cover those baseline cases. |
| Executed locked release builds | On the Rust source at `e3c7fab523fd2c485cb842e16ecec388438698f4`, the operator ran the two locked release builds after the integration commit. `cargo build --locked --release --bin telegram-mcp-relay` and `cargo build --locked --release --example resource_probe` both exited 0 outside Runner. They are not Runner C11/C12 receipts. |
| Executed RELAY-001–007 integration | Run `1e2601a4-8210-41b3-a961-ab53a6d08e6e` reached DONE at revision 229 for `e3c7fab523fd2c485cb842e16ecec388438698f4`, with accepted FINALIZE at revision 201 and reviewer confirmation at revision 210 bound to finalized/reviewed fingerprint `d0232e890221`. The accepted FINALIZE covers C1–C3 for repository, whitespace and formatting and C4–C6 for Clippy, rustdoc and the full locked workspace suite through the Runner-trusted aliases. The suite exercises both MCP protocol revisions against the synthetic peer. The release commands have separate operator results above. |
| Current synthetic coverage | The [daemon composition test](../src/app/tests/extended.rs) uses both protocol revisions without fetching guidance. It exercises display repair under the same identity, effect-free Markdown rejection and explicit plain recovery, formatted file approval, timeout and late inbox recovery, known-ID catch-up, stale-token rejection, one start acknowledgment per reply, token-free resource reads of started replies and explicit completion. [After-send wait cases](#after-send-wait-regressions) cover on-time file approval and recovery after post-send wait admission failure. The [ext4 accounting regression](#relay-001-download-reservation-accounting) models the extra allocation block deterministically. The C6 result above covers these cases with the threaded default at `e3c7fab`; the revised 👀 reaction expectations for the omitted-message start are source-defined until their FINALIZE receipts. |
| Pending operator HOST acceptance | Download through MCP on ext4 under conditions that reproduced the old allocation rejection. Transfer an incoming gigabyte-scale file through MCP with an authorized live local Bot API and record bytes and memory use. Use a real agent to verify formatting, an on-time reply, late recovery, file approval, and the start acknowledgment (reaction by default) before longer work finishes. This trial is not required for the processing-start reaction revision. The supervising operator owns these trials after DONE and workspace release. |

The integration plan assigned C1–C3 and C11/C12 to Worker FINALIZE and C4–C6 to the
selected Runner-trusted commands. The synthetic accounting case and the HOST ext4 download
exercise different evidence boundaries. Whether an agent can wait in the background or wake
on a notification depends on its host.

## Final mixed-protocol composition regressions

The original `app::tests::extended` case composes authenticated 2025-06-18 and 2026-07-28
clients with the real daemon, one poller, SQLite, outbound files, history, guidance and private
downloads at the recorded cloud baseline. Both clients
reuse the separately maintained complete expected catalog and full DTO assertions from
`routes::tests::tools`; production registration is never used as its own expected inventory.
Every tool result retains exact JSON-RPC/payload/content/error assertions, modern
`resultType:"complete"` and legacy omission. Existing security and lifecycle assertions remain.

Explicit synthetic response gates cover a native-caption document followed by a legacy wait,
a modern ask, two modern subscriptions and an inbox claimant, with three distinct durable
claims. Shared-history observation and shared/own attachment downloads preserve reply fences;
shared visibility cannot process/ack the owner's reply. Shutdown with connected subscriptions
retires an abandoned pending download while preserving completed owned copies and durable
recovery. Fresh transport/daemon construction resumes the same application session and saved
reconciliation pair. Disabling history revokes shared downloads before Telegram effects while
own-reply access remains usable; no cached result bypasses current authorization.

Feature-local suites remain the owners of text/file-then-wait catch-up and no resend,
saturated-wait control capacity and forged-header rejection (`routes::tests::wait`), active-call
and idle/abandonment lifetimes (`routes::sessions::tests`), mixed question cancellation and
single-claim recovery (`routes::tests::mixed`), and path/size/expiry boundaries in the attachment
and storage suites. The original composition scenario combines these components. Complete
unfiltered C6 and both locked daemon/resource-probe release builds
(R1/C11 and R2/C12 in the global inventory) passed for the original composition cases.
Current interaction-rule cases and builds have their own
[acceptance record](#interaction-rules-acceptance).
Named-client/HOST, live API and wake outcomes remain separate; source inspection or no-run
compilation alone would not establish execution. Historical benchmark evidence stays separate.

## Repository gates

Run `just check-repo` (or `python3 tools/check_repository.py`) after documentation, skills,
navigation or repository workflow changes. It checks local Markdown link destinations, required
skill frontmatter, canonical symlinks, the marked inline Runner configuration in DEVELOPMENT
and ignored, untracked local artifacts. It uses only Python 3.9+ and Git, no third-party modules. It checks
workspace files including untracked nonignored files; it does not validate external URLs,
Markdown anchors, full YAML schemas or the installed Runner's complete configuration schema.
Runner itself validates the operational configuration when starting a run.
The checker requires the former root JSON template to remain absent. It validates the portable
example's schema/artifact root, current inherited settings, independent defaults, automatic
finalization and empty trusted grants without reading private ignored configurations.

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

For the read-only Runner route, root/helper Cargo clippy/doc/test commands execute only when
selected, through Runner's exact persisted vectors during FINALIZE. This continuation selects
only root C4–C6; helper gates remain conditional/inactive. Each vector builds the standalone
launcher in declared scratch and invokes its corresponding closed mode. The launcher runs
the complete original check with `--offline` and private target output; doc modes retain
`RUSTDOCFLAGS="-D warnings"`. Full root tests still execute the real HTTP assertions. Native
repository, whitespace and formatting checks remain part of FINALIZE's agent portion.
During implementation/review, use source inspection and appropriate no-run compilation;
do not execute or attest the established check inventory or selected logical commands.
See [development](DEVELOPMENT.md#read-only-cargo-validation) for provisioning and isolation.

## Release build and probe gates

The explicit root example `resource_probe` lives under `tools/` and links the real application
library. Its `test = true` target includes argument tests in the complete unfiltered C6 suite;
C4's `--all-targets` also covers it. Root Cargo changes introduce no new dependency resolution.
The mandatory release gates retain these exact identities:

```text
R1: cargo build --locked --release --bin telegram-mcp-relay
R2: cargo build --locked --release --example resource_probe
```

These are Runner-managed FINALIZE obligations, not implementation-turn commands or HOST
measurements. Keep their declared Native route; the standalone helper remains unchanged with
only its six closed clippy/doc/test modes. Actual build results and runtime capability evidence
must come from their execution, not source inspection or no-run development compilation.
H1–H4 above apply when that separate helper changes; this probe does not extend it.
HOST capture requires both release gates plus the applicable tooling gates and a verified
quiescent ownership boundary. See [probe interface and evidence limits](MEASUREMENTS.md).

## Python measurement tooling gates

The standard-library [HOST controller](../tools/measure_resources.py),
[collectors](../tools/measure_collectors.py), [loopback mock](../tools/measure_mock.py),
[MCP driver](../tools/measure_driver.py) and their
controlled tests under `tools/tests/`
use this Native FINALIZE inventory:

```text
P1: python3 -B -c "import ast,pathlib; [ast.parse(p.read_text(encoding='utf-8'), filename=str(p)) for p in pathlib.Path('tools').rglob('*.py')]"
P2: python3 -B -m unittest discover -s tools/tests -p 'test_measure*.py'
```

`-B` prevents checkout bytecode writes. No additional Python lint gate is established.
Tests cover Linux counter parsing/units, sampled RSS and CPU aggregation, process identity and
time/counter rejection, kernel lifetime high water, bounded observation windows, startup timing,
ELF stripping/hash/size and mutation checks, clean source/build/feature identities, stale or
tampered settings/workload bindings, missing commands, output/deadline failures and private
TMPDIR-aware cleanup. Clocks, proc files, binaries and command processes are controlled synthetic
substitutes; these tests bind no sockets, invoke no Cargo/Git commands and measure no real daemon.
Recorded P1/P2 outcomes are described below. Source/unit gates never replace the assigned HOST
workload.

[Mock tests](../tools/tests/test_measure_mock.py) use split input streams, generated near-limit
zero runs, substituted clocks/events and server lifecycle substitutes. They cover exact binary
document-byte accounting with bounded reads, malformed/oversized HTTP and multipart framing,
JSON/result construction, controlled Unix response timestamps, caption bounds,
45-second long-poll cancellation, deadline expiry,
loopback binding policy, worker/request caps and shutdown-before-close on failure. They bind
no sockets, install no signal handlers and establish no real resource measurements. Actual
probe/Telegram HTTP and routing assertions remain owned by the complete C6 suite; the Python
tests do not duplicate those proofs. The separate HOST capture is recorded in
[measurements](MEASUREMENTS.md#recorded-host-capture).

[Driver tests](../tools/tests/test_measure_driver.py) substitute MCP transports, HTTP peers,
split control streams, clocks and phase controls. They cover modern per-request headers/metadata,
the exact registration identity, title and project label, discovery/registration/send result
rejection, bounded fixed/chunked response parsing, direct
numeric-loopback policy, cancellation and deadlines, control EOF/ordering, partial/blocked output
and redacted CLI failures. Private-file cases check the exact sparse workload size, exclusive
creation, confined mode-0700 roots, symlink/mutation rejection and descriptor closure on failed
preparation. Files remain controller-owned until all peers retire; no test starts a child, binds
a socket, installs signal handlers or samples a daemon. The changed driver requires P1/P2 in
FINALIZE. Actual MCP upload/committed-route assertions remain in C6. Quantitative evidence remains
bound to the separate recorded HOST capture.

## Recorded validation evidence

The coordinator export for independent run `5852c1b2-e9ae-4736-a7fd-c1a541cfacf9` records
aggregate finalization success at public revision 889, independent confirmation at 898 and
verified commit `968802371f94f51eedab66998eff6031762db5ea` at the settled pause, revision 948.
The source/tree, manifest/lock and toolchain agree with the
[bound HOST record](MEASUREMENTS.md#provenance-and-release-identity). This resolves the earlier
pending-execution statements for tests at that baseline. The evidence is a
trusted coordinator export of public aggregate observations, not fresh check execution,
cryptographic attestation, per-test counts or separate per-command logs/timings.

| Prior step-3 obligation | Recorded outcome / owner |
| --- | --- |
| C1/C2 repository and whitespace; C3 formatting | Successful Native FINALIZE, aggregate revision 889 |
| C4 Clippy, C5 rustdoc, C6 complete unfiltered workspace tests | Successful selected Runner-trusted FINALIZE, aggregate revision 889; Native `NOT_RUN` alone would not establish this |
| R1 daemon and R2 probe locked release builds | Successful Native FINALIZE before HOST, aggregate revision 889 |
| P1 Python source and P2 complete measurement unit suite | Successful Native FINALIZE, aggregate revision 889 |
| HOST mocked capture | CLI exit 0 and report success/complete; separate process and workload accounting in measurements |
| HOST user-unit verification | Initial exit 1 for absent executable retained; later exit 0 after authorized binary installation, with the unchanged unit and separate receipt [recorded here](MEASUREMENTS.md#unit-verification-and-remaining-acceptance) |
| Current evidence-documentation changes | C1/C2 remain for their dedicated FINALIZE; the earlier receipts do not validate these edits |

The exported persisted step-3 IDs C7/C8 denote R1/R2 and C9/C10 denote P1/P2. They do not
renumber the frozen global fourteen-row catalog or change conditional helper H1–H4 applicability
and selected trusted routes. No helper change or new helper-gate receipt is claimed here.
The report and receipt identities, release profile, separate daemon/probe feature profiles
and HOST retirement provenance are retained in the measurement record. The later static unit
verification passed after binary installation. Those capture/static-unit records establish no
service activation or live-client behavior. The separate
[dated baseline](MEASUREMENTS.md#dated-baseline-host-observations) records later bounded text-flow
success, absent idle wake and canonical-unit runtime failure; it supplies no extension acceptance.

## Behavioral acceptance

`app::tests::probe` exercises explicit loopback construction against the separate HTTP mock:
readiness followed by real MCP registration/send, caller cancellation and accepted-route
draining/reopen, plus credential/remote-peer/listener rejection before storage or Telegram
effects and failed cleanup without readiness. The example's pure argument tests reject DNS,
remote/unspecified/zero-port peers and extra/missing arguments. The
[recorded C6 outcome](#recorded-validation-evidence) covers the original probe cases. The
current MCP probe cases use the required project label; the
[interaction acceptance record](#interaction-rules-acceptance) records their C6 execution.
The HOST probe capture covers a separate synthetic workload.

Binary tests in `src/main.rs` launch isolated test subprocesses using the binary's shutdown
signal registration. They send actual SIGINT and SIGTERM after a readiness gate, hold synthetic
work until an explicit parent release, and check that a repeated signal permits completion
before process exit. These tests cover signal delivery and continued execution during draining;
the composition tests below cover actual send/route persistence with mocked Telegram.
No signal test installs handlers in the parent test process or uses live Telegram.

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
contains only SQLite with history disabled, with no outbound bodies, captions, quoted original text, file bytes or
local paths. The bounded mock accepts actual multipart uploads; response gates replace sleeps.

`app::tests::recovery` combines actual modern subscriptions/listen, Telegram long polling,
an ask and two competing inbox consumers with three reply slots and one-slot event queues.
It asserts three distinct durable reply IDs and fencing tokens, resource invalidations despite
the waiter, one winning resume reconciliation and unchanged first-start identity. A discarded
acknowledgment response, daemon restart, registration resume and Telegram redelivery recover
from the same processing record/receipt; old tokens remain fenced and completion stays explicit.
A separate scenario models the crash gap with an independent SQLite writer that cannot signal
the daemon's live interests, while its earlier poll remains unresolved. After restart, the
first offset and initial subscription invalidation recover the one pending reply with its
original ID; redelivery does not duplicate it. This models lost post-commit wakeups rather
than injecting a process kill between synchronous commit/publish instructions. Fixtures use
the production composition, private TMPDIR-aware storage and explicit HTTP response gates.

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
Subscription, ask and recovery acceptance interactions are defined in `app::tests::recovery`
above.

`routes::tests` exercises real loopback HTTP discovery, native/same-origin clients, bearer auth,
hostile/duplicate headers, paths, missing/mismatched `Mcp-Method`, required version/capability
metadata, safe protocol failures and inclusive body bounds with declared/chunked input.
Test-only SDK handlers check shared
handler state across requests, safe correlation extensions, calls beyond 600 seconds, SSE
admission, disconnect release and transport shutdown using explicit gates/controlled time.
Fixtures are private, randomized and TMPDIR-aware. These transport probes do not establish
named-client compatibility.

`routes::tests::legacy` defines synthetic 2025-06-18 HTTP initialization and fallback
negotiation, SSE results, initialized notification/session reuse, complete tool discovery,
explicit application registration shared with modern calls, read-only resource listings,
templates/reads and explicit transport termination. It checks capability separation,
unavailable legacy subscription APIs, bearer/Host/Origin checks on session operations,
unknown sessions, duplicate/conflicting version/session declarations, malformed input,
inclusive initialization body bounds and retained strict modern request metadata.
Additional regressions reject complete matching inline metadata with live, unknown and
terminated session IDs without registration effects, preserve partial legacy metadata,
and exercise session-owned cancellation/DELETE with controlled handler signals. The preflight
retains inclusive declared/chunked body bounds and shutdown cancellation during a partial body.
Supplied `Mcp-Method` initialization cases follow the SDK's legacy/modern header semantics.
Accepted complete C6 covers the baseline cases. The separate
[HOST record](MEASUREMENTS.md#agent-experience-host-acceptance) verifies fresh eleven-tool
metadata discovery and legacy initialization/catalog/guidance, not all model-driven tool effects.
The prior modern-only version-list assertion is
intentionally expanded to the two supported versions; independent unsupported-version
and security assertions remain. The historical benchmark at
`968802371f94f51eedab66998eff6031762db5ea` and its hashes
and figures remain unchanged and do not measure the new runtime.

`routes::tests::mixed` adds both protocols on one protected endpoint and shared handler.
Each protocol exercises the original tools, durable text/document/question routes, same-title
identity isolation and cross-protocol external-ID resume. Initialization creates no application
identity. Empty resource listings, stable templates and paginated reads agree across protocols
without claiming, exposing claim tokens or changing session activity. Semantic length and
file-root failures retain their pre-effect validation.

Concurrent legacy/modern inbox calls acquire distinct claims. Processing, reconciliation with
a saved prior token/request ID, retries after transport/database restart, stale/wrong-session
fencing and idempotent acknowledgments all use the same durable state. Concurrent questions
on one identity match replies to their own outbound IDs while a modern resource listener
remains active. Explicit cancellation preserves an admitted send; controlled abandonment
preserves a committed question. Both release waiter/session capacity and allow a new transport
to recover the reply.
Semaphore acquisition observes actual socket disconnect and session reclamation; in-process
HTTP body drop fixes the abandonment instant for the controlled-clock case, and
paused time advances the abandonment deadline without sleeps or retrying admission.

`app::tests::recovery` also defines production-composition shutdown with a legacy question,
modern send and connected modern resource listener. Mock Telegram responses hold both admitted
sends across admission closure; shutdown waits for durable route bookkeeping, and both routes
accept recoverable replies afterward without implicit acknowledgment. The accepted composition C6
covers the original mixed-protocol cases. Source inspection or no-run compilation does not
establish a passing test outcome, real Codex discovery, live Telegram operation or fresh
resource measurements. After Runner DONE and ownership retirement, the coordinator separately
[recorded](MEASUREMENTS.md#agent-experience-host-acceptance) fresh eleven-tool Codex metadata
discovery and bounded real HTTP/Telegram/file probes. That trial made no model turn, and the
resumed model still cached seven tools; live asks/subscription events, model concurrency and
independent visual rendering remain unverified.

`routes::tests::tools` exercises the real shared handler through separate modern MCP HTTP
requests and a separate synthetic Bot API. It covers typed input/output discovery, stable
tool listings, normalized registration, external-ID/saved-handle resume through a new handler,
same-title isolation, text at the final scalar boundary and document captions at the native
boundary (including a caption with just the title and project label). Successful sends return
committed composite text/document routes visible through a reopened database. Semantic validation
failures leave Telegram requests, routes and session activity unchanged. Malformed/unknown
arguments/tools remain JSON-RPC errors; application failures are structured tool errors with
redacted counts/guidance. Tool-call fixtures supply the required matching `Mcp-Name`; missing
and mismatched values retain the SDK header-error code/request ID without effects or input echo.
Rejection, uncertain delivery and post-send persistence failure remain distinct, without replay
or invented routes. Fixtures are private, randomized and TMPDIR-aware.

`routes::tests::inbox` adds actual modern MCP requests for stable template/empty resource
listings across registration, direct reads, tied-receipt pagination, session separation,
attachment-only payloads and delivery-state snapshots without tokens, claims or activity
changes. Concurrent inbox calls return distinct durable claims. Processing start/inactivity,
explicit reconciliation, a lost reconciliation response across a fresh handler/database,
stale/wrong-session fencing and idempotent per-reply completion use the shared transitions.
The processing-start case exercises both protocols without a guidance call. It checks that
redelivery after a confirmed start preserves the processing reply and that a repeated start
leaves the acknowledgment send count at one. It also distinguishes ambiguous Telegram delivery
from confirmed delivery with a failed route transaction. The
[interaction acceptance record](#interaction-rules-acceptance) records its execution. That case
now selects `processing_start_ack = "message"` and reopens under the reaction default to
prove schema-12 attempted and sent rows survive unchanged without redispatch.
`processing_reacts_once_by_default_on_both_protocols` is source-defined until FINALIZE. It
checks the exact `setMessageReaction` payload in a second allowlisted chat with history
enabled, no added route or journal row, rejected and uncertain reactions without text fallback,
a failed attempt confirmation, explicit threaded text in reaction mode, an in-flight competing
start, redelivery, repeated, inactive and reconciled calls, restart and a removed chat
allowlist entry. A controlled-time [outbound typing case](../src/services/outbound/typing.rs)
shows that the reaction leaves the typing lease refreshing. Client cases cover the single
non-retried request and its error mapping.
Invalid URIs, limits, listing cursors and tool arguments return safe errors without claiming.
Fixtures honor TMPDIR and use private randomized directories; no live Telegram calls are made.
These cases cover the read/claim/recovery portion of
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
These tests address subscription/reconnect and bounded slow-client criteria in §§6.1, 16 and 19.
Live named-client subscription/event interoperability remains unverified; capability
advertisement and bounded non-subscription HOST effects are recorded separately.

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
An undrained bounded two-inbox interest forces slow-listener overflow without relying on OS
socket buffering. A healthy real SSE listener and ask still receive the same durable reply;
retiring the slow interest releases admission for reconnect, and competing inbox calls claim
the other session's reply only once. The disconnect scenarios keep a real subscription active
through cancellation before/after send and after reply commit, preserving event delivery and
restart recovery independently of the canceled waiter.
These cases cover the ask/event/race, timeout and wait-cancellation portions of §§6.2–6.3,
16 and 19. Fixtures are private and TMPDIR-aware. Full-composition interactions are defined
above; live named-client ask/subscription interoperability is not established by those
fixtures or by the separate bounded HOST trial.

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
These cases address typing criteria in §§7, 16 and 19. The tests establish no live Telegram
behavior or runtime measurements.

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

Unit tests in `domain::outbound::tests` cover whitespace/control normalization, blank and
placeholder title rejection, legacy notice recognition, 50/51-grapheme titles (including combining marks, ZWJ emoji, flags and Hangul), unchanged
multiline bodies and shared native bracketed headers. They exercise final 1,000/1,001-scalar
boundaries for ASCII, Cyrillic, combining marks and supplementary symbols, native 1,024/1,025-UTF-16-unit
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
specification §§10, 16 and 19.
Production startup ordering is covered separately by `app::tests` above.
`services::outbound::tests::typing` also combines actual mocked Telegram HTTP, processing and
cleanup with a single inbox slot: rollback preserves typing refreshes, and committed retention
deletion frees the expired cap so a fresh reply can start typing. Controlled time separates
typing expiry from retention.

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
TMPDIR-aware. These cases do not claim real filesystem ENOSPC, power-loss simulation or
measured peak disk usage.
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
TMPDIR-aware. These cases address notice dispatch/loop criteria in §§8, 16 and 19, with no
live Telegram claim.

`tests/inbox.rs` covers read-only pagination, tied receipt ordering, attachment payloads,
session/composite-question isolation, simultaneous SQLite claim consumers, restart/lease
boundaries, fresh fencing tokens, retained processing ownership and independent reply expiry.
It also covers item/payload bounds, token metadata admission with send reservations, whole-batch
rollback and safe errors. Fixtures are private, randomized and TMPDIR-aware; timestamps are
injected and contention uses a barrier without sleeps.

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

`tests/ingress.rs` covers atomic reply/activity/cursor persistence, same-title cross-chat routes,
multiple replies, attachment-only documents, photo captions, receipt-based TTLs, long incoming
text, redelivery and restart after commit before signaling. Idle lower-ID resets and retained
redeliveries across restart prove exact deduplication without numeric high-water assumptions.
Unauthorized floods compact safely even with full receipt/variable metadata budgets, and a
subsequent authorized reply remains admissible. Tests also check safe unroutable dispositions,
authorization, route expiry, serialized metadata bounds, shared item/payload/
metadata admission including outstanding sends, late transaction failures and actual SQLite
page exhaustion without losing accepted replies or advancing unresolved input. Fixtures are
private, randomized and TMPDIR-aware.

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
directories. The recorded complete C6 outcome covers the baseline cases. The
[interaction acceptance record](#interaction-rules-acceptance) records C6 for the project-label
cases.

`services::outbound::tests::documents` adds actual multipart HTTP/SQLite scenarios: complete
caption scalar/native rejection without effects, exact 1,000-scalar/1,024-UTF-16-unit captions,
header-only captions, binary/empty and multichunk documents, growth and replacement after
validated opening, restart-visible document routes, shared storage/task pressure, definitive rate-limit/rejection,
lost/malformed/mismatched responses, post-send rollback, truncation, deadline and cancellation.
The tests check no replay, no invented routes, released permits/reservations and no outbound
bytes/captions in SQLite. Explicit response gates and clock advancement avoid timing sleeps.
`clients::telegram::upload::tests` consumes a sparse near-cloud-limit file incrementally,
checking bounded lazy chunks, changes after the first read, growth exclusion, early EOF,
read errors and empty snapshots without whole-file buffers. These are behavioral proofs;
resource observations come from the separate HOST record.

`services::files::tests` covers descriptor-confined regular/empty/nested files, multiple
roots, exact configured/cloud decimal-byte boundaries, traversal and prefix collisions,
missing/unreadable files, directories, FIFOs, Unix sockets and root/ancestor/leaf symlinks.
It checks root and leaf replacement after opening and concurrent atomic swaps between
regular paths and escaping symlinks: accepted handles always read the approved file.
Filename cases include controls, header punctuation, Unicode, non-UTF-8 and long basenames;
errors and Debug output stay path-free. Fixtures are exclusive, private, randomized and
TMPDIR-aware; size tests use sparse files, without allocating near-limit buffers.

Unit tests in `clients::telegram::tests` use a synthetic loopback HTTP peer with explicit
disconnects, malformed/oversized and chunked bodies, redirects, webhook conflicts, rate limits
and server errors. They cover request JSON, typed reply/attachment metadata, pre-I/O outbound
validation, redaction, bounded retries and no replay of ambiguous sends. A test-only retry
recorder replaces sleep and asserts the actual chosen delays; a paused Tokio clock covers a
response stalled after headers. Mock tasks are canceled on fixture drop. The historical
Runner FINALIZE for the recorded baseline ran the complete, unfiltered
`cargo test --locked --workspace` suite through the read-only trusted route. It exercised
the original seven HTTP tests and the pure retry-policy test without skips. Earlier Native
attempts failed before HTTP requests at loopback `bind` with
`Operation not permitted`. These fixtures establish no live Telegram result; see the separate
[baseline text trial](MEASUREMENTS.md#dated-baseline-host-observations) and
[architecture](ARCHITECTURE.md#telegram-http-client).

## Specification §§16/19 coverage map

This maps every mandatory section 16 criterion and section 19 requirement group to its
behavioral proof or evidence owner. The recorded validation section above supplies the prior
complete C6 outcome; the measurement record supplies the separate HOST result and the initial
failed and later successful static unit verification. Focused proofs remain in their owning suites; conversation/recovery scenarios
add full-daemon interactions without repeating boundary cases. This coverage map does not
mark the unchanged product specification accepted or establish named-client/live behavior.

| Acceptance / traceability contract | Owning proof |
| --- | --- |
| Three concurrent sessions; same-title identity; normalized rename; old text/document Replies; multiple distinct ordered Replies (§§4.1–4.3, 16, 19) | [Daemon conversation](../src/app/tests/conversations.rs), `three_sessions_route_interleaved_old_text_and_document_replies_after_rename`; [registry tests](../tests/registry.rs) retain cross-chat message-ID collision and identity/restart proofs |
| Meaningful titles, mandatory project labels and effect-free legacy recovery (§§4.1–4.2, 5, 16) | [MCP tools](../src/routes/tools.rs) define registration and recovery cases on both protocols without a guidance call. [Outbound domain tests](../src/domain/outbound/tests.rs) cover display-label normalization and valid framing |
| Enforced interaction rules without a guidance handshake (§§5.4, 14–16) | The [interaction rules acceptance record](#interaction-rules-acceptance) identifies the guidance, mixed-protocol recovery cases, current execution receipts and separate operator trials |
| Attachment-only document and photo-caption metadata; resource reads and inbox claims preserve original-session routing (§§6, 9, 16, 19) | The daemon conversation above; [ingress](../tests/ingress.rs) and [MCP inbox](../src/routes/inbox.rs) retain authorization, rollback, pagination and fencing boundaries |
| Title graphemes, final scalar/native caption limits, structured rejection without splitting/truncation or effects (§§4.2, 9, 16, 19) | [Outbound domain tests](../src/domain/outbound/tests.rs), [MCP tools](../src/routes/tools.rs) and [ask tests](../src/routes/ask.rs) |
| Streamed, confined, Reply-routable files without source copies/default outbound history (§§9, 12, 16, 19) | Daemon conversation checks actual multipart contents, routes and persisted files; [file tests](../src/services/files/tests.rs), [document tests](../src/services/outbound/documents.rs) and [upload tests](../src/clients/telegram/upload.rs) retain path-race, mutation and bounded-read proofs |
| Unknown/expired routes never reassigned; unauthorized input excluded; durable atomic cursor (§§4.3, 10–12, 16, 19) | [Ingress](../tests/ingress.rs), [retention](../tests/retention.rs) and [poller tests](../src/services/polling/tests.rs) |
| Always-active events, enabled/disabled bounded waits, one reply shared by ask/inbox/resource consumers (§§5–6, 16, 19) | [Daemon recovery](../src/app/tests/recovery.rs), `subscribed_ask_and_inbox_claims_reconcile_and_recover_missed_acknowledgments`; [ask tests](../src/routes/ask.rs) retain disabled waiting, initial recheck and question-targeting proofs |
| Wait timeout/cancellation never deletes routes/replies, acknowledges work, interrupts agents or emits timeout notices (§§6.2, 11, 16, 19) | [Ask tests](../src/routes/ask.rs), controlled timeout/late-reply recovery and subscribed disconnect cases; [daemon shutdown](../src/app/tests.rs) retains admitted send/ingress draining proofs |
| Idempotent concurrent claims/start/ack; rotating tokens, explicit reconciliation and missed acknowledgment recovery (§§6, 10–11, 16, 19) | Daemon recovery above; [inbox storage](../tests/inbox.rs) and [MCP inbox](../src/routes/inbox.rs) retain lease, wrong-session, transaction rollback and lost reconciliation-response boundaries |
| Offline replies and crash after commit before signal/next poll; restart offset and stable IDs without duplicates (§§6, 10–11, 16, 19) | Daemon recovery, `committed_reply_without_a_wakeup_or_next_poll_recovers_after_restart` models the lost-signal gap; [ingress](../tests/ingress.rs) retains atomicity and [poller tests](../src/services/polling/tests.rs) exercise actual restart offsets/redelivery |
| Subscription reconnection reads SQLite instead of replaying events; slow/disconnected clients remain bounded (§§6.1, 16, 19) | Daemon recovery; [subscription tests](../src/routes/subscriptions.rs) retain setup-gap/disconnect/overflow proofs; [ask tests](../src/routes/ask.rs), `slow_listener_overflow_preserves_a_healthy_stream_waiter_and_claims` isolates overflow from healthy events and waits |
| Confirmed capped typing, no idle refresh, factual once-only notices without loops (§§7–8, 16, 19) | [Typing tests](../src/services/typing/tests.rs), [notice storage tests](../tests/notices.rs) and [notice dispatch tests](../src/services/notices/tests.rs) |
| Independent TTLs, bounded cleanup and physical reclamation without losing valid replies (§§10–11, 16, 19) | [Retention](../tests/retention.rs), [maintenance](../tests/maintenance.rs) and [cleanup scheduler tests](../src/services/cleanup/tests.rs) |
| Protected secrets/roots, loopback defaults and secure MCP transport (§§12–13, 16, 19) | [Configuration tests](../src/config.rs), [secret tests](../src/config/secrets.rs), [storage tests](../tests/storage.rs), file tests above and [HTTP transport tests](../src/routes/tests.rs) cover authorization, Host/Origin, redaction, body/concurrency bounds |
| One native daemon, one poller, no vendor adapters/agent management, external database, broker, Node.js or Docker requirement; no public HTTPS/webhook needed (§§1–3, 13, 16, 19) | [Application](../src/app.rs), application/poller tests and [architecture](ARCHITECTURE.md) define and exercise the composition; [setup](SETUP.md) documents locked builds and optional user deployment. [HOST record](MEASUREMENTS.md#recorded-host-capture) preserves the resource result and initial failed/later successful static unit verification |
| Concise agent text/file policy; no periodic agent polling and no universal LLM wake promise (§§2, 6, 14, 16, 19) | Complete copyable section 14 [agent policy](AGENT_POLICY.md), token/reconciliation instructions and [client timeout guidance](SETUP.md#connect-mcp-clients); existing [contributor policy](../AGENTS.md) is preserved |
| Telegram at-most-24-hour unconsumed backlog, independent TTLs and reconnect limitations explained to users (§§10–11, 16, 19) | [Setup/recovery](SETUP.md#recovery-and-retention), [README](../README.md), [architecture](ARCHITECTURE.md) and [configuration](CONFIGURATION.md) explain the independent limits and webhook-safe setup |
| Protocol support and actual client capabilities (§§5–6, 16–19) | [Subscription](../src/routes/subscriptions.rs), transport and daemon recovery fixtures cover synthetic MCP 2026-07-28 clients. [Legacy HTTP fixtures](../src/routes/legacy.rs), [mixed clients](../src/routes/mixed.rs) and [extended composition](../src/app/tests/extended.rs) passed at the recorded composition baseline, including negotiation, full DTO catalog, observation/downloads and recovery/shutdown. [Client requirements](SETUP.md#connect-mcp-clients) separate later HOST protocol/discovery/bounded live effects from unverified model concurrency/catalog refresh and universal wake; legacy events remain unsupported |
| Near-zero idle CPU, best-effort idle RSS below approximately 30 MB, startup, near-limit upload peak RSS, stripped locked release size and dependency profile (§§3, 9, 16–19) | [Recorded HOST benchmark](MEASUREMENTS.md#workload-and-observed-resources) gives actual probe/mock/driver figures, startup and sampled/lifetime limits; [build identity](MEASUREMENTS.md#provenance-and-release-identity) records stripped daemon/probe sizes and distinct profiles. Unit tests retain bounded-read/idle-scheduler proofs without asserting resource targets |

## Section 17 deliverable coverage

The original ten deliverables remain in scope. Historical checks apply to their recorded
baselines. Current changes require their bound FINALIZE receipts. Operational acceptance has
separate evidence.

| Deliverable | Location / evidence and remaining limit |
| --- | --- |
| 1. Minimal working Rust source | [Architecture](ARCHITECTURE.md), `src/`, and mocked daemon interactions. The composition baseline records complete C4–C6 |
| 2. Lock, reproducible release instructions, sample configuration | [Cargo.lock](../Cargo.lock), [setup](SETUP.md), [sample TOML](../config/relay.example.toml), composition-baseline R1/R2 and bound offline HOST materialization |
| 3. SQLite initialization/migrations, cleanup, restart-safe cursor | [Repositories](../src/repositories.rs), storage/retention/maintenance/ingress/poller suites above |
| 4. MCP tools/resources/subscriptions with tested current protocol | Mixed-protocol composition-baseline C6 covered guidance/history/attachments and the independent DTO catalog. Legacy resource events remain unsupported |
| 5. Single poller, strict final text/caption limits, safe documents | Client/domain/file/outbound suites above; historical mocked upload and separately dated real native-framed text/file/history/download observations; visual rendering and general live cases remain outside that bounded trial |
| 6. Unit/integration tests using mock Telegram | Complete unfiltered C6 at the composition baseline and controlled P1/P2 at the recorded measurement baseline. Fixtures use synthetic messages and credentials |
| 7. README/setup covering bot, allowlists, URL, secrets, registration, wait caveat, notices | [README](../README.md), [setup/client guidance](SETUP.md), [configuration](CONFIGURATION.md); separate baseline and extension discovery/live records retain their model-catalog, concurrency and wake limits |
| 8. Contributor or copyable section 14 agent policy | Preserved [AGENTS.md](../AGENTS.md) and complete copyable [agent policy](AGENT_POLICY.md), including file reports, no periodic inbox polling and explicit completion |
| 9. Example user unit and protected environment-file approach | [Unit](../deploy/telegram-mcp-relay.service), [environment template](../deploy/relay.env.example), [setup](SETUP.md#optional-systemd-user-service); static verification failed then passed after binary installation; later installed canonical unit failed at runtime with `218/CAPABILITIES`, separate from transient one-process text operation |
| 10. Benchmark target, RSS/CPU, file-upload peak and binary size | [Recorded HOST capture](MEASUREMENTS.md#recorded-host-capture), with separate process overhead and sampled/lifetime limits; production bootstrap was not measured |

## Measurements and check ownership

Release acceptance records Linux target/toolchain, stripped locked release binary size,
dependency profile, startup time, idle RSS/CPU and peak RSS during a near-limit streamed upload.
Use an identified reproducible workload and state whether Telegram is mocked. The sub-30 MB
idle target is best effort, not an unmeasured guarantee or a brittle unit-test assertion.
The [measurement document](MEASUREMENTS.md) owns the release probe, bounded Python collectors,
mock/driver and receipt-gated HOST controller. `tools/tests/test_measure_resources.py` uses
controlled processes, pipes and clocks for prerequisite/profile rejection, phase coordination,
PID identity, artifact changes, retirement and exclusive failure reporting. It opens no network
sockets or Cargo build processes. These tests belong to the existing P1/P2 FINALIZE gates;
the [recorded P1/P2 results](#recorded-validation-evidence) apply to the measurement baseline.
Actual resource values come from the separate bound HOST capture. Source/build checks do not
attest runtime measurements; existing
complete C6 owns actual HTTP/routing proofs. The original unit failure is historical; later
verification passed after binary installation as [recorded](MEASUREMENTS.md#unit-verification-and-remaining-acceptance).
That benchmark/static trial exercised no service activation or live behavior. Later baseline
observations and the extension HOST record are separately scoped above.

Native Runner commands use the accepted staging-independent inventory and
[Runner finalization](../.agents/skills/finalization/agent-runner.md). Staged inspection belongs
only at COMMIT/HANDOFF. Assign unavailable HOST measurements explicitly in task gates; they
remain mandatory final acceptance evidence after the run and never become fabricated passes.
Observe actual network/cache/socket failures before classifying a capability blocker. Report
the command, cause and required owner; do not weaken a check or substitute an unrelated test.
