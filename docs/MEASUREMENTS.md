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
addresses and never forwards requests. Its subordinate interface for the future orchestrator is:

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
paths without binding sockets. Their execution belongs to P1/P2 in FINALIZE. Orchestration,
real HOST capture and quantitative release acceptance remain pending.

## Synthetic MCP driver

[measure_driver.py](../tools/measure_driver.py) is a separate standard-library Python 3.9+
process. Its subordinate interface for the future orchestrator is:

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
runtime acceptance report. The separate mock and driver above are implemented; orchestration
and indexed release evidence remain subsequent planned work. Its interfaces are:

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
not proof that arbitrary binaries were built from that source. The later orchestrator must
verify accepted R1/R2/tooling prerequisites, preserve the exact build/profile provenance and
recheck source/binaries around the workload. Bind all effective synthetic settings and workload
details, including sample cadence, upload size and separate mock behavior; omit secrets and
machine-local private paths. Runtime identity/prerequisite failures must remain failed evidence.
Collector tests use synthetic proc/ELF files and substituted commands/clocks; they establish
neither resource targets nor live Telegram behavior. Their execution belongs to FINALIZE.

The assigned HOST commands remain pending until the orchestrator and its prerequisites exist:

```bash
python3 tools/measure_resources.py --mock --output LOCAL_ARTIFACTS/mvp-measurements.json
systemd-analyze --user verify deploy/telegram-mcp-relay.service
```

HOST never replaces root/helper checks or either locked release build. Capture only at a
verified paused-idle boundary with retired turns/children and quiescent ownership, or after
whole-run DONE and retirement. No live messages, service installation or service enablement
are part of this procedure. Preserve source/build/workload binding, failed results and deviations
when later integrating actual evidence; implementation completion alone is not MVP acceptance.
