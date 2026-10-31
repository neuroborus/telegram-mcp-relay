# Resource Measurements

Quantitative acceptance remains pending. No startup, RSS, CPU, upload-peak or binary-size
result is established by probe source or a successful build. The best-effort idle RSS target
below approximately 30 MB needs an identified Linux x86_64 release workload and actual evidence.

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
`caption`, `document` parts. The mock incrementally discards document bytes and returns their
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
paths without binding sockets. Their execution belongs to P1/P2 in FINALIZE. Real HOST capture
and quantitative release acceptance remain pending.

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
file admission and Telegram multipart path read and stream all bytes. Caption text is
`Synthetic near-limit upload.`, prefixed by the normal `[Resource measurement]` header; these
ASCII strings fit both existing caption bounds. Root/file identity and metadata are rechecked
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
mock and driver. Indexed release evidence remains pending. Its interfaces are:

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

[measure_resources.py](../tools/measure_resources.py) implements the assigned interface:

```bash
python3 tools/measure_resources.py --mock --output LOCAL_ARTIFACTS/mvp-measurements.json
systemd-analyze --user verify deploy/telegram-mcp-relay.service
```

The CLI disables bytecode caching before local imports so startup preserves the clean source tree.

The second command remains a separate operator action. Neither command runs in IMPLEMENT or
review. Capture remains **Pending HOST**; no measured value or accepted unit verification is
recorded here. Linux `/proc`, Python 3.9+, Git, a native supported Rust toolchain and a complete
local locked dependency cache are prerequisites. Missing inputs fail without fetching/repair.

Before capture, the coordinator must export actual successful public Runner receipts to the
ignored `LOCAL_ARTIFACTS/mvp-measurement-prerequisites.json`. This is a new measurement input,
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
receipt IDs. The frozen fourteen-row catalog, conditional helper gates and complete schedule
remain unchanged. This file records commit 3 prerequisites and does not replace that catalog
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
