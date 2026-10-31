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

## Bounded collectors

[measure_collectors.py](../tools/measure_collectors.py) provides standard-library Python 3.9+
building blocks. It does not launch the probe, run a workload, build binaries or produce a
runtime acceptance report. The separate mock/orchestrator and indexed release evidence remain
subsequent planned work. Its interfaces are:

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
