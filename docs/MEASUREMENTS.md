# Resource Measurements

The [recorded HOST capture](#recorded-host-capture) measured the real library composition with
synthetic loopback Telegram traffic: 10,067,968-byte idle RSS and a 10,428,416-byte sampled
near-limit upload peak for the probe. Separate user-unit verification initially failed because
its executable was absent, then passed after authorized binary installation as recorded below.
These results cover the disclosed mocked composition and static verification; they establish
neither production-bootstrap measurements, service activation nor live Telegram interoperability.
The later [baseline text-flow trial](#dated-baseline-host-observations) has its own release
identity and limits. The [extension HOST record](#agent-experience-host-acceptance) separately
identifies the later installed release and bounded observed behavior; it is not a new benchmark.

## Recorded HOST capture

### Provenance and release identity

The 2026-10-31 operator handoff records a successful completed capture (CLI exit 0, report
`status: "success"`, `phase: "complete"`) at clean commit
`968802371f94f51eedab66998eff6031762db5ea`, tree
`5b5df98c3a71ba94af1d61f6921aa90514a916ff`. This is the capture identity, not a claim that later
documentation or runtime commits were measured. All figures below remain bound to that source,
build and workload. A later performance claim would need its own observations; the
agent-experience continuation requires no new benchmark.

This historical summary records the operator's aggregate handoff, artifact digests and
canonical build/settings/workload binding. The controller
rechecked source, binaries, toolchain, dependency profiles, configuration and prerequisites
around capture before publishing success. Its owned temporary binaries/configuration were
removed after retirement. The tracked facts, method and limitations below stand on their own;
the ignored raw captures are not prerequisites for reading this record or setting up the project.
This continuation did not rebuild or remeasure the historical workload.

| Evidence | SHA-256 |
| --- | --- |
| Historical raw capture (116,509 bytes) | `a7da542f30b806b080b40c063887a7a62dba641f3083257a335ac44984af59c9` |
| Historical prerequisite export | `d92372456804101369ec81f56a3f207dac616270ef6e6da4e45faeaf04f5adf0` |
| Operator aggregate handoff | `d721dc381c1a63af3036eaa8b53e5c8c9993e3fc0fae68657087935a55633845` |
| Canonical report build/settings/workload binding | `79bca46850c3c2e777621eedcb0baa9bb2d38571ffb667b892327e68b15e2358` |
| `Cargo.toml` (2,061 bytes) | `e747e437083f51551f18fee16049cd53d98db79b526d444a53bd18c5d5227238` |
| `Cargo.lock` (54,415 bytes) | `b4603b3738116ecf3458b805263a406357e19d9d907558528da834a0501e3b1c` |

These digests preserve provenance, not portable file locations or cryptographic attestations.
The export records independent `plan-execution` run
`5852c1b2-e9ae-4736-a7fd-c1a541cfacf9`: aggregate finalization revision 889, independent
confirmation 898 and verified committed paused-idle boundary 948. At capture the coordinator
reported idle execution, no lease owner and no process record. R1/R2 and applicable tooling
gates were recorded successful before HOST. This is trusted operational provenance, not a
cryptographic attestation or separate per-command logs/timings; see
[recorded validation evidence](TESTING.md#recorded-validation-evidence).

The operator's actual invocation was
`env -u GIT_PAGER python3 tools/measure_resources.py --mock --output LOCAL_ARTIFACTS/mvp-measurements.json`.
The nonpersistent removal of `GIT_PAGER` satisfied the controller's environment-override policy;
the assigned Python arguments were unchanged. No exact wall-clock capture interval was
exported; 2026-10-31 is the handoff date, and phase timing uses monotonic observations.

| Build property | Recorded value |
| --- | --- |
| Native target | `x86_64-unknown-linux-gnu`, ELF64 machine 62 |
| Rust | `rustc 1.96.0 (ac68faa20 2026-05-25)`, commit `ac68faa20c58cbccd01ee7208bf3b6e93a7d7f96`, LLVM 22.1.2 |
| Cargo / Python | `cargo 1.96.0 (30a34c682 2026-05-25)` / Python 3.12.3 |
| Release | Thin LTO, one codegen unit, `strip = true`; locked offline private materialization after Runner gates |
| Stripped daemon | 11,173,112 bytes; SHA-256 `b793295bf968523ce48aa3be654a0412fa0d6078699c5f454453ed947e0863a0` |
| Stripped probe | 11,084,824 bytes; SHA-256 `39b01dc14236868e863f47b36b69011e01c3df5853f74f517c0bc3cc28f06308` |
| Daemon dependency-feature profile SHA-256 | `27ea8c94436ced67916d4a4a3bc809ea6676a6487a339a0330d1060d113ba542` |
| Probe dependency-feature profile SHA-256 | `70c7f6553a41a67ba4c552c64d9fc5d85541ad78245980b717dd9c1ca2b675cc` |

Profile hashes cover the exact normalized UTF-8 `cargo tree` output described below, with
local package locations replaced by `<local>`. Both profiles include rmcp 3.8.0 server/Streamable
HTTP, reqwest 0.13.5 JSON/multipart/rustls/stream, rusqlite 0.32.1 bundled SQLite and Tokio 1.54.1
current-thread `rt`, filesystem/I/O, network, signals, synchronization and time. The observed
profile difference is Tokio `test-util` enabled for the probe through dev-feature unification;
neither profile enables `rt-multi-thread`. The profile identities and normalization method
preserve the historical binding; direct features are listed in
[architecture](ARCHITECTURE.md#dependency-baseline).

### Workload and observed resources

One credential-free probe used separate numeric-loopback MCP and mock endpoints,
explicit synthetic chat/user allowlists, and private
database/export roots. Settings retained 45-second polling, 30-day TTLs, 64 MiB database cap,
50,000,000-byte outbound limit, 65,536-byte upload buffer and default admission/retention
bounds. Waiting, typing and both notice categories were enabled; no replies were injected to
activate typing/notices. No token source or MCP bearer
was configured. Generated TOML identity was 1,082 bytes, SHA-256
`f3a667194179bef2e4210cccb262ad0335117c77b3d5e6c8ddbce66c6ab98a59`; private locations are
represented by logical roles in the report.

| Other effective synthetic settings | Value |
| --- | --- |
| Wait / processing grace / claim lease | 600 / 60 / 60 seconds |
| Typing refresh / maximum | 4 / 600 seconds |
| Cleanup interval / auth-token file creation | 24 hours / disabled |
| Request / Telegram response bounds | 65,536 / 2,097,152 bytes |
| Concurrent requests / background tasks / waiters | 32 / 64 / 16 |
| Subscriptions / per-subscription queue | 64 / 16 |
| Inbox items / per-reply bytes / total inbox bytes | 10,000 / 65,536 / 16,777,216 |
| Metadata bytes / sessions / routes / receipts | 16,777,216 / 1,024 / 100,000 / 10,000 |

The driver confirmed MCP 2026-07-28 discovery, registered a synthetic session and title,
remained alive for the idle window, then called
`telegram_send_file` once with a 49,999,999-byte sparse zero file under the allowed root.
The normal streaming/committed-route path applied a short synthetic caption with the then-current
bracketed title header. This predates native entities. No file contents were loaded by the
driver or retained by the mock, no production limit was changed, and no upload pacing was
added. Final mock accounting records four requests, one document, exactly 49,999,999 drained
document bytes and `failed: false`.

| Timing / sampling | Observation |
| --- | --- |
| Probe launch to ready | 0.038482 seconds |
| Probe launch to confirmed MCP response | 0.098521 seconds |
| Requested idle window / cadence | 50 seconds / 0.5 seconds per process, covering a complete long poll |
| Actual idle observations | 100 per process; first-to-last span approximately 49.5807 seconds |
| Driver upload request interval | Monotonic 399952.843743039–399952.893711289; 0.049968 seconds |
| Upload observation window | Two samples per process, priming through trailing observation; approximately 0.501 seconds |

RSS values below are bytes. Idle mean/minimum/maximum were identical for each process. CPU is
user+system time over the observed idle span as a percentage of one logical CPU, excluding
children. The Python peers are separate measurement overhead, never added to the probe figure.

| Role | Idle RSS | Idle CPU seconds / percent | Upload sampled peak RSS | Lifetime VmHWM before → after upload |
| --- | --- | --- | --- | --- |
| Probe / daemon composition | 10,067,968 | 0.02 / 0.040338% | 10,428,416 | 10,424,320 → 10,690,560 |
| Telegram mock | 14,618,624 | 0.01 / 0.020169% | 14,749,696 | 14,999,552 → 15,183,872 |
| MCP driver | 20,242,432 | 0.01 / 0.020169% | 20,242,432 | 20,660,224 → 20,660,224 |

The probe's approximately 10.07 MB idle RSS is below the best-effort approximately 30 MB target
for this Linux x86_64 workload. Idle CPU was small but nonzero, including normal long-poll and
timer work. No target deviation was observed in this capture; it establishes no cross-platform
or production-bootstrap guarantee. The probe adds readiness/signal channels, omits production
diagnostics and has the distinct feature profile above. The controller, inspection/build
children and filesystem cache are excluded from peer accounting. Host hardware/kernel details
were not exported.

The upload completed between the two sampling ticks. Its sampled maximum can miss short
spikes; the kernel's **process-lifetime** VmHWM includes startup and idle as well as upload.
Neither the before/after readings nor their difference is a phase-only peak. The low observed
RSS is consistent with the existing bounded-streaming proofs, rather than an exhaustive
memory bound or a live-network throughput benchmark. Sampling verified each peer's PID/start-tick
identity; this summary publishes roles and aggregates rather than host process identifiers.

Both private build children (no sampled start ticks) and all three
peers exited 0; every owned group was recorded retired before workspace cleanup. The handoff's
subsequent metadata-only PID checks found all five absent. This describes that completed
capture, not the ownership state of a later documentation turn.

### Unit verification and remaining acceptance

The separate HOST command `systemd-analyze --user verify deploy/telegram-mcp-relay.service`
initially returned **1** because `<user-bin>/telegram-mcp-relay` was absent. That historical
failure remains part of the historical record. The unit at that trial had SHA-256
`3f573a275e6ec9ef1da581a92e3ba0394f42ce91cee1eb74adb108e3c9b0e72b`.

On **2026-10-31**, following explicit local-installation authorization, the operator installed
the existing measured daemon release binary at `<user-bin>/telegram-mcp-relay`, mode `0755`,
size **11,173,112 bytes**, SHA-256
`b793295bf968523ce48aa3be654a0412fa0d6078699c5f454453ed947e0863a0`.
It was copied without rebuilding or overwriting an existing file. The same static unit
verification command then returned **0**, with empty stdout and stderr, against the unchanged
unit. The later static-verification receipt has SHA-256
`f52bf49e536167b2103db202901aafc0d1885dab85fa437a407c7fd64ef0dde6`
and binds then-current HEAD `14b147de13fe17be7fbe999021dba6639c8a9a89`. It references the original
capture at `968802371f94f51eedab66998eff6031762db5ea`; no runtime/build input or capture identity
was changed or relabeled.

The executable-absence blocker was resolved. That static trial installed, enabled or started
no service unit and tested no live Telegram/client behavior. Preserve both its original failure
and later success. The later baseline below records actual service-start failure and bounded
text-flow success separately. Incoming downloads are now implemented and synthetically tested;
the later [extension trial](#agent-experience-host-acceptance) also records a bounded real
authorized file round trip. See the complete
[acceptance and deliverable coverage](TESTING.md#specification-1619-coverage-map).

## Dated baseline HOST observations

The coordinator supplied these **2026-10-31** observations after the three compatibility commits
reached DONE and retired at source `4389ce430a4bc9b113f8b3fb13abb51d1813d8f5`. A locked offline
daemon release build exited 0. The installed executable was **11,282,488 bytes**, SHA-256
`614f84e5c8027454e6094ed4a967acf3b28401b6d765df2ca0161118019ea007`.
This is a different runtime from the benchmark above and predates the agent-experience extension.
These are supplied historical operational observations, not checks rerun by an implementation
Worker or a claim about the host's present state. No private receipt is needed to interpret them.

| Observation | Result and scope |
| --- | --- |
| Separate fresh Codex 0.166.0 CLI public app-server probe | Discovered all seven original tools; no model turn or Telegram tool call occurred in this probe |
| Actual resumed conversation, 15:04–15:06 UTC | Still lacked Telegram tools; `list_mcp_resources` reported an unknown MCP server |
| Actual conversation after the user's next reload, 15:08:46 UTC | Exposed register/send/send_file/ask/inbox/processing/ack; the observation does not identify the causal reload mechanism or promise resumed-host refresh |
| Actual MCP startup and authorized outbound text, approximately 15:11 UTC | Registration with stable identity/accurate title succeeded, startup inbox was empty, and the explicitly requested send confirmed Telegram acceptance plus committed reply routing |
| Subsequent inbound text, same date | Inbox returned one claimed reply; processing start, status-response send, processing stop and acknowledgment with the current token succeeded |
| Discovery/events | Modern discovery returned HTTP 200; MCP 2025-06-18 and 2026-07-28 were supported; discovery advertised modern resource subscriptions, while legacy subscriptions were unavailable |
| Idle host | The Telegram message alone did not wake the agent; the user's conversation resume triggered the inbox read |
| Process and service | Exactly one ordinary-user foreground relay ran under a transient user service; the installed canonical hardened unit was inactive after runtime start failed with `218/CAPABILITIES` |

Private keys, real chat/user/message identifiers, usernames, content and personal paths are
omitted. This verifies one bounded pre-extension text claim/process/response/completion cycle.
It verifies neither file sends, asks, extended waits, native rendering, shared history,
guidance/downloads nor universal wake. The transient process did not prove canonical-unit
startup. Earlier static verification success did not test runtime capabilities; keep the
failure visible without weakening the unit or OS policy.

## Agent-experience HOST acceptance

After final-eight **DONE** and released/quiescent ownership, the coordinator performed the
following authorized **2026-11-01** installation and bounded probes. This is a sanitized factual
record, not a dependency on private receipts, a new benchmark, a universal host certification
or a claim that every live failure/recovery case was exercised.

### Installed source and preserved state

The accepted source HEAD is `70fedf8471b8981a802fa0085d527f6bd5d35645`; Rust runtime composition
is unchanged since `e7575f2636480b35fa1da75134e337fadb9a4a00`, whose full C1–C6 and Native
locked daemon/probe releases passed. The final documentation step passed its own Native C1/C2;
all fifteen planned steps reached DONE as [recorded in testing](TESTING.md#agent-experience-continuation-status).

The coordinator separately materialized the locked daemon and probe releases offline after
the accepted gates. The final identities below describe those separate artifacts; they do
not assert byte equality for a combined multi-target build.

| Artifact | Actual final identity |
| --- | --- |
| Installed stripped daemon | **11,912,632 bytes**, SHA-256 `aff87150e9f98129e5ed1cb0514b10cdf7ee471490684c5f1e050b7d492d7da1`; matched the accepted Native release artifact |
| Separate stripped probe | SHA-256 `1f7b56fecc9a0616e43f77fc522e2537952de52e87c5bd3dfa12f676935a55d8` |

Existing SQLite was privately backed up from schema **6**, then migrated to **10**; the
resulting database remained mode `0600`. Filled bot/bearer credentials and numeric user/chat
configuration were preserved. Only necessary opt-in history/download settings were enabled.
Private backup locations, filesystem paths, keys, IDs, usernames and message contents are omitted.
Exactly one ordinary-user relay process/listener ran under the transient user service.
The canonical hardened unit still failed at runtime with **218/CAPABILITIES**; no unit or OS
security was weakened. Old successful static unit verification remains a separate historical fact.

### Bounded observed protocol and interaction outcomes

| Observation | Actual result and limit |
| --- | --- |
| Legacy MCP 2025-06-18 | Initialization HTTP 200, eleven-tool catalog and guidance version 1; correct legacy result-tag omission, no resource subscription advertisement; deletion of the probe's own transport returned 202 |
| Modern MCP 2026-07-28 | Discovery listed both supported revisions and `resources.subscribe=true`; a request without bearer returned 401. This verifies capability advertisement, not a new live event-delivery trial |
| Fresh Codex public app-server inventory | Eleven tools, `toolsError: null`, no model turn. The resumed model still cached seven tools and required a client restart/refresh before natural calls to the new tools; the separate probe did not refresh its catalog |
| Guidance and registration | Guidance was fetched first; stable existing identity resumed with a short directory label; startup inbox returned zero. Retained title/directory display snapshots matched registration. Fresh public TUI title lookup was unavailable, so the last known public title was preserved privately |
| Authorized Telegram text and document | Each send succeeded with Telegram acceptance and committed reply routing. Shared history exposed confirmed outgoing metadata with the expected display context; no local content or identifiers are reproduced here |
| Authorized stored-handle download | Returned **44 bytes**, identical to the sent file; private mode `0600`, pinned root and device/inode identity were verified. This is one bounded same-host file round trip, not general remote-filesystem access |
| Active after-file wait | A single default **600-second** `telegram_wait` was armed after the successful file send, without resending. While it remained pending, tools/list, history and download requests succeeded concurrently through HTTP |
| Wait completion | Returned `wait_timeout` after **600.003 seconds**, without resending or granting permission; the owned probe session then retired with exit 0 |

The assigned bounded HOST probe completed successfully. A timeout is the observed outcome of
that wait, not permission to act or proof that no later reply can arrive within retention.

The observed concurrency belongs to the direct HTTP probe. It does not establish concurrent
Codex model/background execution or automatic waking of an idle/paused/unloaded host. The
native entity layout passed the accepted synthetic tests and Telegram accepted framed sends;
visual Telegram rendering was not independently inspected. The current cached model catalog,
fresh title-lookup limitation and canonical-unit runtime failure remain explicit boundaries.
The earlier pre-extension inbound-text handling cycle above is separate; this trial does not
claim every new inbound/error/restart scenario was exercised live.

These installed-release/file/protocol observations supply no fresh startup/RSS/CPU/upload-peak
measurement. The `968802` mocked resource figures retain their original source/build/workload
identity. Post-release contributor guidance/navigation and public evidence changes require their
own documentation/skill checks and selected-mode review/confirmation before authorized commit
and whole-task publication; they do not inherit checks from the completed product run.

## Library-linked release probe

The root Cargo example [resource_probe](../tools/resource_probe.rs) links the actual
[`app` composition](../src/app.rs) through its explicit [probe constructor](../src/app/probe.rs).
It uses protected SQLite, the same MCP endpoint, one poller, notices, retention, shared
typing and admitted-send draining. Production startup and TOML retain the fixed Telegram cloud
endpoint; there is no production endpoint override or new dependency.

Build prerequisites are the locked root toolchain/dependencies and ordinary private writable
Cargo output, as for the daemon. Both exact builds are mandatory Runner-managed FINALIZE checks:

```bash
cargo build --locked --release --bin telegram-mcp-relay
cargo build --locked --release --example resource_probe
```

The example uses the root stripped thin-LTO release profile. Root all-target Clippy compiles
its source; the complete root test gate includes its argument tests and library probe tests.
No release/probe mode is added to the standalone six-mode Runner helper. See
[testing](TESTING.md#release-build-and-probe-gates) for check identities and ownership.

The probe interface is:

```text
target/release/examples/resource_probe <synthetic-config.toml> <loopback-mock-ip:port>
```

Run it only as part of an authorized HOST workload after its build/tooling gates pass and
workspace ownership is quiescent. The mock is a **separate** process, supplied by the measurement
orchestrator, not a service started by the probe. Only numeric loopback addresses with nonzero
ports are accepted, including `[::1]:port`; no DNS names, proxies or redirects are used.
Requests use `/botSynthetic/<method>`, without reading a bot token. The mock must implement
the used Bot API methods, including webhook inspection, approximately 45-second long polls
and incrementally drained multipart uploads. The mock must not forward to Telegram.

Use validated synthetic TOML settings with explicit synthetic chat/user allowlists, a free
loopback MCP port and an absolute private temporary database/export directory. Keep default
bounds unless the workload records changes. **Omit** all bot-token/auth-token environment/file
sources and `create_auth_token_file`; the probe rejects them without resolving secrets.
It does not create workload files or own/delete the caller's configuration/database/export
directory. The orchestrator must create exclusive mode-0700 TMPDIR-aware resources and remove
only its own resources after process retirement.

The current-thread wrapper registers SIGINT/SIGTERM before service construction. Following
successful startup cleanup and service spawning, stdout emits exactly one JSON line and flushes:

```json
{"event":"ready","mcp_address":"127.0.0.1:8765"}
```

The driver should verify MCP responsiveness as part of startup measurement; this line does not
promise a successful future poll or a live Telegram connection. Startup/service failures exit
nonzero and report only a static safe error code on stderr. Failure to write readiness requests
a graceful drain before failure. SIGINT/SIGTERM requests shutdown and awaits accepted work;
repeated signals do not abort draining. Forced termination retains the production uncertainty
limitations. No work is acknowledged implicitly.

## Evidence boundaries

The probe wrapper adds signal listeners and bounded readiness/stop channels, and omits the
production diagnostic subscriber. It runs the real library services, not the production binary
bootstrap. Report that difference, build identity and separate mock/driver overhead; do not
attribute the mock's RSS to the daemon or present synthetic traffic as live-API evidence.
Cargo examples also enable existing dev-dependency features, including Tokio's `test-util`;
record the probe's feature profile separately from the production binary's profile.
The production locked release build remains independently required.

## Bounded loopback Telegram mock

[measure_mock.py](../tools/measure_mock.py) is the standard-library Python 3.9+ peer for the
probe's `/botSynthetic/` requests. It runs in one separate process, binds only numeric loopback
addresses and never forwards requests. Its subordinate interface for the controller is:

```text
python3 tools/measure_mock.py --listen 127.0.0.1 --port 0
```

Port zero selects a kernel-assigned port. Stdout emits a bounded JSON `ready` record with
`mock_address` (bracketed for IPv6). A final `stopped` record reports requests, documents,
drained document bytes and failure status; neither record includes content, filenames or private
paths. SIGINT/SIGTERM cancels long polls and reads; workers retire before the final record.
This interface does not establish probe readiness or MCP responsiveness.

The mock implements `getWebhookInfo`, `getUpdates`, `sendMessage`, `sendDocument` and
`sendChatAction`. Empty updates wait for the requested 1–50 seconds (45 by default), with one
outstanding poll. Text/document response dates use current Unix seconds so committed routes
retain their normal lifetime; request deadlines use a separate monotonic clock.
Document forms follow the probe's length-delimited, ordered `chat_id`,
`caption`, `caption_entities`, `document` parts. The native header-entity JSON field is bounded
to 256 bytes and at most two nonoverlapping UTF-16 spans. This current mock change adds no
new runtime measurement or reinterpretation of the historical capture above.
The mock incrementally discards document bytes and returns their
actual drained count as `document.file_size`. It applies the unchanged 50,000,000-byte limit and
1,000-scalar/1,024-UTF-16-unit caption bounds. There is no artificial upload pacing.

Work is capped at four workers, a listen backlog of four, 256 admitted requests and a 900-second
process lifetime. Each connection serves one request, then closes. Header payloads are limited
to 8 KiB, JSON to 8 KiB, part headers to 2 KiB, text fields to 4 KiB and multipart bodies to the
document limit plus 16 KiB. Reads request at most 64 KiB; no document is buffered in full.
Header/JSON reads have a ten-second absolute deadline, document processing 120 seconds and
response writes five seconds. Shutdown interrupts reads within their half-second socket wait;
retirement also awaits bounded response writes. Unsupported framing (including chunked bodies),
oversized/malformed input and exhausted worker/request budgets fail; failures remain reflected
in the exit status. The mock does not implement incoming replies or live Telegram semantics.

Controlled stream/event/server tests cover framing, bounded draining, cancellation and failure
paths without binding sockets. Their P1/P2 FINALIZE outcome is included in the
[recorded validation evidence](TESTING.md#recorded-validation-evidence); runtime observations
come from the separate HOST capture above.

## Synthetic MCP driver

[measure_driver.py](../tools/measure_driver.py) is a separate standard-library Python 3.9+
process. Its subordinate interface for the controller is:

```text
python3 tools/measure_driver.py --mcp-address 127.0.0.1:<port> --allowed-root <private-export-root>
```

The controller supplies the probe's bound numeric loopback MCP address and an existing absolute,
current-user-owned mode-0700 export root, also configured in the probe's allowed roots. Every
root component must be free of symlinks. The workload expects synthetic destination chat `-101`;
the orchestrator must explicitly allow that chat and a synthetic user in TOML. This interface
does not launch a probe or mock, grant file access, sample processes or run HOST capture.

The driver uses the implemented stateless MCP `2026-07-28` lifecycle: `server/discover`, then
`telegram_register` with external ID `resource-measurement-v1` and title `Resource measurement`,
then exactly one `telegram_send_file`. Every request carries `MCP-Protocol-Version`, `Mcp-Method`,
tool calls carry `Mcp-Name`, and params carry the protocol-version and empty client-capabilities
metadata. Discovery must confirm the version, private zero-TTL capability response and tools.
The driver preserves the returned session key internally and validates structured tool results
against their text representation. Neither the key nor an inbox URI is emitted.

After discovery, stdout emits `responsive`; its monotonic timestamp identifies confirmed MCP
responsiveness separately from the probe's readiness line. After registration and exclusive
file creation, `idle` reports `upload_bytes=49999999`. The fixed `measurement-upload.bin` is a
49,999,999-byte sparse zero file, one byte below the unchanged 50,000,000-byte production limit.
Only its absolute path is passed to MCP; the driver never reads its contents. The normal probe
file admission and Telegram multipart path read and stream all bytes. The short synthetic
caption receives the current native bold title header through `caption_entities`; it fits both
caption bounds. This differs from the historical benchmark's bracketed header and does not
remeasure it. Root/file identity and metadata are rechecked
before and after sending. Existing files are never overwritten or removed.

Control input consists of newline-delimited JSON. The controller first waits for `idle` and
primes its sampling window, then writes `{"command":"upload"}`. The driver emits
`upload_started`, invokes the normal MCP send tool, validates the expected chat and positive
message ID, then emits `uploaded` with those IDs and the actual verified file size. Tool success
confirms Telegram acceptance and committed reply routing under the implemented service contract;
existing C6 HTTP/routing tests own that proof. These events delimit the observed request interval,
including MCP and bookkeeping overhead. The driver stays alive for the controller's trailing
samples until `{"command":"stop"}`, then emits `stopped`. All events include
`at_monotonic_seconds`. The controller records PID/start-tick identity separately; the driver
does not claim a measurement from its events.

Only those two commands in that order succeed. EOF, timeout, cancellation, protocol/tool errors,
changed files and output failures exit nonzero with a static redacted diagnostic. There is no
send retry; a failed/disconnected attempt may have been accepted. The controller must retire
probe/mock/driver process groups before removing its private root, including a partial file
left by failed preparation. The driver starts no children and deletes no resources itself.

Control lines are capped at 256 bytes, each wait at 120 seconds, requests at 8 KiB, response
headers at 8 KiB, decoded bodies at 64 KiB and total response framing at 128 KiB. Fixed-length
and chunked JSON responses are supported; redirects, sessionful responses and encoded/SSE
bodies fail. Stdout records are at most 1 KiB each and 4 KiB total, with five-second writes.
Discovery and registration each have a ten-second absolute deadline; the upload request allows
620 seconds for the production 600-second attempt and bookkeeping. Total driver lifetime is
900 seconds. SIGINT/SIGTERM cancels waits/response reads within a quarter-second check interval;
connect and small request writes are bounded by five seconds. Mock upload draining remains
unpaced and independently bounded as documented above. Sample this driver's RSS/CPU separately
from the probe and mock; the controller's sampling cadence/windows remain orchestration-owned.

Controlled transports/streams/clocks and private-file tests cover protocol rejection, phase
ordering, bounded framing, cancellation, redaction and file ownership. Execution belongs to
FINALIZE; source and unit tests establish no real runtime/resource result.

## Bounded collectors

[measure_collectors.py](../tools/measure_collectors.py) provides standard-library Python 3.9+
building blocks. It does not launch the probe, run a workload, build binaries or produce a
runtime acceptance report. The controller below composes these collectors with the separate
mock and driver. The recorded capture above supplies runtime evidence. Its interfaces are:

| Collector | Result and limits |
| --- | --- |
| `startup_times` | Monotonic launch-to-readiness and launch-to-confirmed-MCP-response seconds; rejects reversed, nonfinite or over-60-second observations |
| `read_process`, `collect_window`, `summarize` | Linux `/proc/PID/stat` RSS in bytes and user+system CPU ticks; 2–1,201 samples, 0.01–5-second requested intervals, at most 600 seconds per window |
| `read_high_water` | Kernel `VmHWM` in bytes, with explicit process-lifetime scope; checks the original PID/start-tick identity before and after the read |
| `file_identity`, `collect_build` | Streaming SHA-256/size of existing binaries, stripped ELF64 section checks, clean Git HEAD/tree, manifest/lock hashes, native toolchain/target and separate daemon/probe dependency-feature profiles |
| `evidence_binding`, `verify_binding` | Canonical SHA-256 binding of the observed build identity and exact nonsecret synthetic settings/workload; rejects stale or changed identities |
| `private_workspace`, `run_command` | Exclusive mode-0700 TMPDIR-aware directories with owned cleanup; closed shell-free inspection commands with a 30-second default/60-second maximum deadline, 2 MiB combined output cap and process-group retirement |

RSS summary values are the arithmetic sample mean, minimum and **sampled** maximum. CPU percent
is process user+system CPU seconds divided by observed monotonic elapsed seconds, scaled to one
logical CPU; it excludes child processes. A short spike may fall between samples. Read the
kernel high water before and after upload as well: it captures intervening peaks but cannot
attribute a peak to upload when an earlier phase already set a higher lifetime maximum. Retain
both readings and the sampled upload window; do not relabel a lifetime high water as a measured
phase-only maximum. Sample daemon/probe, mock and driver PIDs separately and identify their roles.
Missing/malformed counters, PID reuse, regressing CPU/time and excessive windows fail explicitly.

Build inspection requires Git, `rustc`, Cargo, readable locked registry inputs for **offline**
`cargo tree`, and the two already-built release artifacts. It invokes `rustc -vV`,
`cargo --version` and, separately for the two profiles:

```text
cargo tree --locked --offline --target <native-target> --edges normal,build --prefix none --format "{p} features=[{f}]"
cargo tree --locked --offline --target <native-target> --edges normal,build,dev --prefix none --format "{p} features=[{f}]"
```

The second includes the example's dev-dependency feature unification. Local package source paths
are replaced with `<local>` before storing/binding profiles; package names, versions and features
are preserved so checkout location does not affect the dependency identity. Supported native targets
are Linux x86_64/aarch64 GNU/musl; little-endian ELF64 machine IDs must match the toolchain host.
Files must be regular, non-symlink, nonempty and at most 256 MiB; hashing uses at most 64 KiB
reads. Binary section inspection rejects static symbol/debug tables without altering artifacts.
Source cleanliness and artifact hashes are rechecked around collection; observed changes fail.
No command fetches dependencies, repairs prerequisites or runs a release build.
Git inspection disables optional locks/index refreshes with `GIT_OPTIONAL_LOCKS=0`.

These identities record what was observed together. A digest is an integrity/binding check,
not proof that arbitrary binaries were built from that source. The controller must
verify accepted R1/R2/tooling prerequisites, preserve the exact build/profile provenance and
recheck source/binaries around the workload. Bind all effective synthetic settings and workload
details, including sample cadence, upload size and separate mock behavior; omit secrets and
machine-local private paths. Runtime identity/prerequisite failures must remain failed evidence.
Collector tests use synthetic proc/ELF files and substituted commands/clocks; they establish
neither resource targets nor live Telegram behavior. Their execution belongs to FINALIZE.

## HOST controller and prerequisites

[measure_resources.py](../tools/measure_resources.py) retains the historical benchmark interface:

```bash
python3 tools/measure_resources.py --mock --output LOCAL_ARTIFACTS/mvp-measurements.json
systemd-analyze --user verify deploy/telegram-mcp-relay.service
```

The CLI disables bytecode caching before local imports so startup preserves the clean source tree.

The closed prerequisite schema below belongs to the earlier independent step-3 capture. It is
not the lazy final-eight schedule or a request to export new receipts into that old schema.
Keep its source/interface unchanged. The current extension needs the bounded coordinator
installation/probes above, not another benchmark. A separately authorized future measurement
must satisfy genuine compatible prerequisites rather than fabricate or relabel receipts.

The second command remains a separate operator action. Neither command runs in IMPLEMENT or
review. The recorded capture succeeded; the original unit failure and later successful static
verification are preserved above.
Linux `/proc`, Python 3.9+, Git, a native supported Rust toolchain and a complete
local locked dependency cache are prerequisites. Missing inputs fail without fetching/repair.

For that benchmark workflow, the coordinator must export actual successful public Runner receipts
to the ignored `LOCAL_ARTIFACTS/mvp-measurement-prerequisites.json`. This is a measurement input,
not a change to frozen Runner configuration/guidance or an export of Runner scratch/private
state. The controller does not operate Runner. The JSON envelope is:

```text
schema_version: 1
pipeline: "plan-execution"
mode: "independent"
step: 3
run_id: <canonical public run UUID>
source: <collectors.source_identity(root): head, tree, manifest and lock identities>
rustc: <exact stripped stdout from rustc -vV>
cargo: <exact stripped stdout from cargo --version>
public_status: {runId: <same UUID>, completedCommits: <public execution projection's array>}
checks: [{id, command, status: "PASS", route, public_revision}, ...]
```

The last public verified commit must equal the clean capture HEAD. Supply one check record for
each of C1–C10 in the persisted step-3 inventory: C7/C8 are R1/R2 and C9/C10 are P1/P2.
Preserve these actual receipt IDs, with the exact established command strings
and positive integer public receipt revisions. Routes are `runner-trusted` for C4–C6 and
`native` for the others. Obtain both the Native and selected trusted successful receipts;
Native `NOT_RUN` for a selected command is insufficient. The global bootstrap catalog uses
C11/C12 for R1/R2 and C13/C14 for P1/P2; those catalog IDs are distinct from persisted step-3
receipt IDs. The historical fourteen-row catalog and conditional helper gates
remain unchanged. This input records that run's commit 3 prerequisites and does not replace its catalog
or renumber exported receipts. Do not synthesize PASS records from build/unit/source
success, HOST execution or a clean tree. Coordinator exports are trusted operational input,
not cryptographic attestations; retain the original public observations for evidence review.

Only after these receipts match clean source and toolchain does the ordinary CLI materialize
unchanged release builds. It starts with an empty private target directory, using these exact
subordinate vectors (the mandatory Runner R1/R2 commands above remain separate):

```bash
cargo build --locked --offline --release --bin telegram-mcp-relay
cargo build --locked --offline --release --example resource_probe
```

Both run with private `CARGO_TARGET_DIR`/`TMPDIR`, `CARGO_BUILD_JOBS=1` and
`CARGO_NET_OFFLINE=true`. Inspection/build children disable Rustup automatic installation and
update checks. Each build has a 600-second deadline and a 2 MiB combined diagnostic cap;
diagnostic contents are discarded. Existing Cargo/Rust profile, target, flag and wrapper
environment overrides and Cargo configuration files at the Cargo home or repository/ancestor
levels and Git identity/index overrides are rejected. The manifest must retain thin LTO,
one codegen unit and stripping. Normal HOST cache/toolchain access supplies prerequisites;
no Runner scratch is exported and no HOST
build counts as R1/R2. Builds and inspection children are excluded from runtime peer samples.

The controller creates one exclusive mode-0700 TMPDIR-aware workspace, separate database/export
roots and mode-0600 credential-free TOML. All effective nonsecret settings are explicit and
recorded with logical private-path roles, synthetic chat `-101`, user `202`, default bounds and
45-second polling. The MCP port is selected on numeric loopback; the existing probe interface
requires releasing that reservation before launch. A competing bind fails without retry.

Startup records launch-to-probe-ready and launch-to-confirmed-MCP-discovery separately, bounded
by 60 seconds. The driver remains alive during a 50-second idle window, covering a complete
45-second long poll, then performs its one 49,999,999-byte upload. Requested sampling cadence
is 0.5 seconds for each of probe, mock and driver; actual timestamps, RSS/CPU counters and
PID/start-tick identities accompany summaries. Upload sampling is primed before dispatch and
retains a trailing observation, even when the request completes between cadence ticks. Its
590-second controller deadline bounds observation without changing the production upload
limit/deadline. Both driver event timestamps delimit the request interval; the sampled window
also includes controller/MCP overhead. No mock pacing is added. Final mock accounting must
confirm exactly one document and exactly 49,999,999 drained bytes.

Before/after upload `VmHWM` readings retain process-lifetime scope. Short spikes may be absent
from sampled maxima; an earlier startup/idle peak can dominate lifetime high water. Source,
manifest/lock, toolchain, stripped ELF hashes/sizes, separate dependency-feature profiles,
generated config identity, settings and workload are checked around capture. Private paths,
session keys, child diagnostics and content are absent from the report. Probe bootstrap and
dev-feature differences remain disclosed; controller/build/cache costs are excluded.

Each peer uses its own process group and bounded control/output pipes. The controller drains
and reaps the driver, probe and mock, checks exit statuses, and verifies group retirement.
Failure/cancellation retires every owned group, escalating after five seconds where necessary;
forced termination fails the capture and retains delivery uncertainty. Unverified retirement
preserves owned temporary resources instead of deleting beneath possible survivors. Verified
retirement permits removal only of the controller's original workspace. Completed phase data
and safe failure phase remain in a failed report, along with owned process exit/retirement
records (start ticks are unavailable for excluded build children or a failed initial sample).
A failed capture never becomes a pass.

The output must be beneath an existing nonsymlink repository `LOCAL_ARTIFACTS/` directory.
Publication is atomic, mode-0600 and exclusive: existing evidence is never overwritten. The
coordinator must preserve/archive a previous report before an authorized repeat capture. Output
failure returns nonzero with a static diagnostic. Controlled orchestration tests exercise
prerequisite rejection, framing, process ownership, phase coordination, identity changes and
failure publication; they do not establish measured resource use. Accept a capture only with
both zero CLI exit status and report `status: "success"`.

HOST never replaces root/helper checks or either locked release build. Capture only at a
verified paused-idle boundary with retired turns/children and quiescent ownership, or after
whole-run DONE and retirement. No live messages, service installation or service enablement
are part of this procedure. Preserve source/build/workload binding, failed results and deviations
when later integrating actual evidence; implementation completion alone is not MVP acceptance.
