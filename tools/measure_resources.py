"""HOST-only, receipt-gated synthetic resource capture; no live Telegram traffic."""

from __future__ import annotations

import argparse
from dataclasses import asdict
import json
import math
import os
from pathlib import Path
import re
import selectors
import secrets
import shutil
import signal
import socket
import stat
import subprocess
import sys
import tempfile
import time

# Local import caches would invalidate the clean-source prerequisite of the assigned CLI.
sys.dont_write_bytecode = True

try:
    from . import measure_collectors as collectors, measure_driver as driver
except ImportError:  # Direct documented CLI invocation.
    import measure_collectors as collectors
    import measure_driver as driver


ROOT = Path(__file__).resolve().parent.parent
RECEIPTS = "LOCAL_ARTIFACTS/mvp-measurement-prerequisites.json"
CADENCE = 0.5
IDLE_SECONDS = 50
UPLOAD_SECONDS = 590
PIPE_BYTES = 8192
BUILD_SECONDS = 600
BUILD_COMMANDS = (
    ("cargo", "build", "--locked", "--offline", "--release", "--bin", "telegram-mcp-relay"),
    ("cargo", "build", "--locked", "--offline", "--release", "--example", "resource_probe"),
)
# Persisted step-3 receipt IDs, distinct from the global bootstrap catalog IDs.
# These prerequisite commands are never executed by this controller.
GATES = {
    "C1": "python3 tools/check_repository.py",
    "C2": "git diff --check HEAD",
    "C3": "cargo fmt --all -- --check",
    "C4": "cargo clippy --locked --workspace --all-targets -- -D warnings",
    "C5": 'RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items',
    "C6": "cargo test --locked --workspace",
    "C7": "cargo build --locked --release --bin telegram-mcp-relay",
    "C8": "cargo build --locked --release --example resource_probe",
    "C9": 'python3 -B -c "import ast,pathlib; [ast.parse(p.read_text(encoding=\'utf-8\'), filename=str(p)) for p in pathlib.Path(\'tools\').rglob(\'*.py\')]"',
    "C10": "python3 -B -m unittest discover -s tools/tests -p 'test_measure*.py'",
}
TRUSTED = {"C4", "C5", "C6"}


def require(condition):
    if not condition:
        raise collectors.MeasurementError("invalid measurement prerequisite or observation")


def read_json(path):
    require(collectors.file_identity(path)["size_bytes"] <= collectors.MAX_OUTPUT)
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as source:
        require(stat.S_ISREG(os.fstat(source.fileno()).st_mode))
        raw = source.read(collectors.MAX_OUTPUT + 1)
    require(len(raw) <= collectors.MAX_OUTPUT)
    return driver.parse_json(raw)


def prerequisites(root, source):
    """Consume coordinator-exported public receipts, never Runner private state/scratch."""
    path = root / RECEIPTS
    identity = collectors.file_identity(path)
    receipt = read_json(path)
    require(type(receipt) is dict and type(receipt.get("schema_version")) is int
            and receipt["schema_version"] == 1)
    require(json.dumps(receipt.get("source"), sort_keys=True) == json.dumps(source, sort_keys=True)
            and receipt.get("step") == 3)
    require(receipt.get("mode") == "independent" and receipt.get("pipeline") == "plan-execution")
    require(receipt.get("public_status", {}).get("completedCommits", [None])[-1:] == [source["head"]])
    run_id = receipt.get("run_id")
    require(type(run_id) is str and re.fullmatch(
        r"[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}", run_id) is not None)
    require(receipt["public_status"].get("runId") == run_id)
    checks = receipt.get("checks")
    require(type(checks) is list and len(checks) == len(GATES))
    require(all(type(item) is dict for item in checks))
    require({item.get("id") for item in checks} == set(GATES))
    for item in checks:
        require(set(item) == {"id", "command", "status", "route", "public_revision"})
        require(item.get("command") == GATES[item["id"]] and item.get("status") == "PASS")
        require(item.get("route") == ("runner-trusted" if item["id"] in TRUSTED else "native"))
        # A stable public observation reference, without paths or provider output.
        require(type(item.get("public_revision")) is int and item["public_revision"] > 0)
    require(collectors.file_identity(path) == identity)
    return receipt, identity


def release_settings(root):
    require(collectors.file_identity(root / "Cargo.toml")["size_bytes"] <= 65_536)
    text = (root / "Cargo.toml").read_text(encoding="utf-8")
    sections = re.findall(r"^\[profile[^\n]*\]", text, re.MULTILINE)
    require(sections == ["[profile.release]"])
    body = text.split("[profile.release]", 1)[1].split("[", 1)[0]
    require([line.strip() for line in body.splitlines() if line.strip()] ==
            ['lto = "thin"', 'codegen-units = 1', 'strip = true'])
    return {"lto": "thin", "codegen_units": 1, "strip": True}


def build_environment(root):
    """Refuse implicit profile/target/wrapper overrides instead of repairing prerequisites."""
    forbidden = {"RUSTFLAGS", "RUSTDOCFLAGS", "RUSTC", "RUSTDOC", "RUSTC_WRAPPER",
                 "RUSTC_WORKSPACE_WRAPPER", "CARGO_ENCODED_RUSTFLAGS"}
    require(not forbidden.intersection(os.environ))
    require(not any(key.startswith("CARGO_") and key != "CARGO_HOME" for key in os.environ))
    require(not any(key.startswith("GIT_") and key != "GIT_OPTIONAL_LOCKS" for key in os.environ))
    home = Path(os.environ.get("HOME", ""))
    cargo_home = Path(os.environ.get("CARGO_HOME", str(home / ".cargo")))
    for directory in (cargo_home, *(parent / ".cargo" for parent in (root, *root.parents))):
        require(not any((directory / name).exists() or (directory / name).is_symlink()
                        for name in ("config", "config.toml")))
    require(cargo_home.is_absolute() and cargo_home.is_dir())
    require(all(shutil.which(name) for name in ("cargo", "rustc", "git")))
    return {**{key: value for key, value in os.environ.items() if key in
               {"PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "TMPDIR"}},
            "RUSTUP_AUTO_INSTALL": "0", "RUSTUP_NO_UPDATE_CHECK": "1"}


def synthetic_settings(address):
    """All effective defaults are explicit; private locations use logical roles in evidence."""
    return {
        "server": {"listen": address, "create_auth_token_file": False},
        "telegram": {"chat_id": driver.CHAT_ID, "allowed_chat_ids": [driver.CHAT_ID],
                     "allowed_user_ids": [202], "long_poll_timeout_seconds": 45},
        "delivery": {"wait_fallback_enabled": True, "wait_timeout_seconds": 600,
                     "processing_start_grace_seconds": 60, "claim_lease_seconds": 60},
        "activity": {"typing_enabled": True, "typing_refresh_seconds": 4,
                     "typing_max_duration_seconds": 600},
        "storage": {"path": "<private-database>/relay.sqlite", "session_ttl_days": 30,
                    "route_ttl_days": 30, "reply_ttl_days": 30, "cleanup_interval_hours": 24,
                    "max_database_bytes": 67_108_864},
        "notifications": {"notify_unconfirmed_processing": True, "notify_unroutable": True},
        "files": {"allowed_roots": ["<private-export>"], "max_outbound_mb": 50},
        "limits": {"request_bytes": 65_536, "telegram_response_bytes": 2_097_152,
                   "concurrent_requests": 32, "background_tasks": 64, "waiters": 16,
                   "subscriptions": 64, "subscription_queue": 16, "inbox_items": 10_000,
                   "reply_bytes": 65_536, "inbox_bytes": 16_777_216,
                   "metadata_bytes": 16_777_216, "sessions": 1024, "routes": 100_000,
                   "receipts": 10_000, "upload_buffer_bytes": 65_536},
    }


def write_config(workspace, settings):
    for name in ("database", "export"):
        (workspace / name).mkdir(mode=0o700)
    lines = []
    for section, fields in settings.items():
        lines.append(f"[{section}]")
        for key, value in fields.items():
            if (section, key) == ("storage", "path"):
                value = str(workspace / "database" / "relay.sqlite")
            elif (section, key) == ("files", "allowed_roots"):
                value = [str(workspace / "export")]
            lines.append(f"{key} = {json.dumps(value, ensure_ascii=False)}")
    path = workspace / "synthetic.toml"
    with path.open("x", encoding="utf-8") as target:
        target.write("\n".join(lines) + "\n")
    path.chmod(0o600)
    return path


def free_address():
    # The probe interface cannot inherit an FD. A reservation race fails startup, never retries.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return f"127.0.0.1:{listener.getsockname()[1]}"


class Child:
    def __init__(self, role, process, launched, identity, limit):
        self.role, self.process, self.launched, self.identity = role, process, launched, identity
        self.limit, self.total = limit, 0
        self.buffer, self.events = bytearray(), []
        self.eof = set()
        self.retired = False

    def feed(self, channel, data):
        self.total += len(data)
        require(self.total <= self.limit)
        if not data:
            self.eof.add(channel)
            require(channel != "stdout" or not self.buffer)
        if channel != "stdout" or self.role == "build":
            return  # Never retain stderr or build diagnostics (may contain private paths).
        self.buffer.extend(data)
        require(len(self.buffer) <= PIPE_BYTES)
        while b"\n" in self.buffer:
            line, _, rest = self.buffer.partition(b"\n")
            self.buffer = bytearray(rest)
            event = driver.parse_json(line)
            require(type(event) is dict and len(self.events) < 8)
            self.events.append(event)


class Supervisor:
    """At most three simultaneous peers, bounded pipes, deadlines and owned process groups."""
    def __init__(self, root, environment, *, clock=time.monotonic):
        self.root, self.environment, self.clock = root, environment, clock
        self.children = []
        self.selector = selectors.DefaultSelector()
        self.cancelled = False
        self.forced = False

    def inspect(self, arguments, **kwargs):
        return collectors.run_command(arguments, environment=self.environment, **kwargs)

    def launch(self, role, arguments, *, env=None):
        require(not self.cancelled)
        require(sum(child.process.poll() is None for child in self.children) < 3)
        require(len(self.children) < 5)
        launched = self.clock()
        process = subprocess.Popen(arguments, cwd=self.root, env=env or self.environment,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   start_new_session=True)
        # Record ownership before any fallible pipe setup or /proc inspection.
        child = Child(role, process, launched, None,
                      collectors.MAX_OUTPUT if role == "build" else PIPE_BYTES)
        self.children.append(child)
        for channel in ("stdout", "stderr"):
            stream = getattr(process, channel)
            os.set_blocking(stream.fileno(), False)
            self.selector.register(stream, selectors.EVENT_READ, (child, channel))
        os.set_blocking(process.stdin.fileno(), False)
        if role != "build":
            child.identity = collectors.read_process(process.pid)
        return child

    def pump(self, seconds):
        require(not self.cancelled and 0 <= seconds <= 1)
        for key, _ in self.selector.select(seconds):
            child, channel = key.data
            data = os.read(key.fd, 4096)
            child.feed(channel, data)
            if not data:
                self.selector.unregister(key.fileobj)

    def event(self, child, name, deadline):
        require(not self.cancelled)
        while not child.events:
            # A short-lived peer may have exited after writing its final event.
            require(self.clock() < deadline and "stdout" not in child.eof)
            self.pump(min(0.1, deadline - self.clock()))
        result = child.events.pop(0)
        require(result.get("event") == name)
        return result

    def command(self, child, command):
        require(not self.cancelled)
        data = (json.dumps({"command": command}) + "\n").encode("ascii")
        require(os.write(child.process.stdin.fileno(), data) == len(data))

    def wait(self, child, seconds):
        deadline = self.clock() + seconds
        while child.process.poll() is None or len(child.eof) != 2:
            require(self.clock() < deadline)
            self.pump(min(0.1, deadline - self.clock()))
        require(child.process.wait(timeout=0) == 0 and not child.events)
        try:
            os.killpg(child.process.pid, 0)
        except ProcessLookupError:
            child.retired = True
            return
        raise collectors.MeasurementError("peer left an unretired process group")

    def retire(self):
        """Attempt every group even after a failure. Unverified retirement forbids cleanup."""
        clean = True
        for child in reversed(self.children):
            process = child.process
            try:
                if child.retired:
                    continue  # Never signal an old, verified-absent group ID after PID reuse.
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.forced = True
                # Even an exited leader can have surviving group members.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
                try:
                    os.killpg(process.pid, 0)
                    clean = False
                except ProcessLookupError:
                    child.retired = True
            except (OSError, subprocess.TimeoutExpired):
                clean = False
            finally:
                for stream in (process.stdin, process.stdout, process.stderr):
                    try:
                        stream.close()
                    except OSError:
                        clean = False
        try:
            self.selector.close()
        except OSError:
            clean = False
        return clean


def materialize(supervisor, workspace, source, receipt):
    require(receipt.get("rustc") == supervisor.inspect(["rustc", "-vV"], cwd=supervisor.root).strip())
    require(receipt.get("cargo") == supervisor.inspect(["cargo", "--version"], cwd=supervisor.root).strip())
    environment = {**supervisor.environment, "CARGO_TARGET_DIR": str(workspace / "target"),
                   "TMPDIR": str(workspace), "CARGO_BUILD_JOBS": "1", "CARGO_NET_OFFLINE": "true"}
    for command in BUILD_COMMANDS:
        require(collectors.source_identity(supervisor.root, command=supervisor.inspect) == source)
        child = supervisor.launch("build", command, env=environment)
        supervisor.wait(child, BUILD_SECONDS)
    return workspace / "target/release/telegram-mcp-relay", workspace / "target/release/examples/resource_probe"


def snapshot(children):
    result = {}
    for child in children:
        require(child.process.poll() is None)
        sample = collectors.read_process(child.process.pid)
        require((sample.pid, sample.start_ticks) == (child.identity.pid, child.identity.start_ticks))
        result[child.role] = sample
    return result


def high_water(children):
    return {child.role: collectors.read_high_water(child.identity.pid, child.identity.start_ticks)
            for child in children}


def event_time(event, expected, start, end):
    require(set(event) == {"event", "at_monotonic_seconds", *expected})
    value = event["at_monotonic_seconds"]
    require(type(value) in (int, float) and start <= value <= end and math.isfinite(value))
    for key, wanted in expected.items():
        require(type(event[key]) is type(wanted) and event[key] == wanted)
    return value


def observe(supervisor, children, *, seconds, workload=None):
    """Prime before dispatch; include a trailing sample even for sub-cadence uploads."""
    samples = {child.role: [] for child in children}
    start = supervisor.clock()
    deadline, next_sample = start + seconds, start
    uploaded = None
    if workload is not None:
        primed = snapshot(children)
        for role, sample in primed.items():
            samples[role].append(sample)
        next_sample += CADENCE
        supervisor.command(workload, "upload")
        started = supervisor.event(workload, "upload_started", deadline)
        began = event_time(started, {"upload_bytes": driver.UPLOAD_BYTES}, start, supervisor.clock())
    while True:
        now = supervisor.clock()
        require(now <= deadline + CADENCE)
        if now >= next_sample:
            for role, sample in snapshot(children).items():
                require(len(samples[role]) < collectors.MAX_SAMPLES)
                samples[role].append(sample)
            next_sample = now + CADENCE
        if workload is not None and workload.events and uploaded is None:
            uploaded = supervisor.event(workload, "uploaded", deadline)
            message_id = uploaded.get("outbound_message_id")
            require(type(message_id) is int and 0 < message_id < (1 << 63))
            ended = event_time(uploaded, {"upload_bytes": driver.UPLOAD_BYTES,
                               "telegram_chat_id": driver.CHAT_ID, "outbound_message_id": message_id},
                               began, supervisor.clock())
        if (workload is None and now >= deadline) or (uploaded is not None and
                min(len(items) for items in samples.values()) >= 2 and
                min(items[-1].monotonic_seconds for items in samples.values()) >= ended):
            break
        require(now < deadline)
        supervisor.pump(min(0.1, next_sample - now, deadline - now))
    result = {role: {"summary": collectors.summarize(items),
                     "observations": [asdict(item) for item in items]} for role, items in samples.items()}
    if workload is not None:
        result["request_interval"] = {"started_monotonic_seconds": began,
                                      "completed_monotonic_seconds": ended}
    return result


def capture(supervisor, workspace, probe, settings, result):
    mock = supervisor.launch("mock", [sys.executable, "-B", "tools/measure_mock.py", "--port", "0"])
    ready = supervisor.event(mock, "ready", supervisor.clock() + 10)
    require(set(ready) == {"event", "mock_address"})
    driver.endpoint(ready["mock_address"])
    mock_address = ready["mock_address"]
    config = write_config(workspace, settings)
    config_identity = collectors.file_identity(config)
    composition = supervisor.launch("probe", [str(probe), str(config), ready["mock_address"]])
    ready = supervisor.event(composition, "ready", composition.launched + 60)
    ready_at = supervisor.clock()
    require(ready == {"event": "ready", "mcp_address": settings["server"]["listen"]})
    workload = supervisor.launch("driver", [sys.executable, "-B", "tools/measure_driver.py",
        "--mcp-address", settings["server"]["listen"], "--allowed-root", str(workspace / "export")])
    responsive = supervisor.event(workload, "responsive", composition.launched + 60)
    responsive_at = event_time(responsive, {"protocol_version": driver.VERSION}, ready_at, supervisor.clock())
    idle = supervisor.event(workload, "idle", composition.launched + 60)
    event_time(idle, {"upload_bytes": driver.UPLOAD_BYTES}, responsive_at, supervisor.clock())
    peers = [composition, mock, workload]
    result.update(mock_address=mock_address,
                  startup=collectors.startup_times(composition.launched, ready_at, responsive_at),
                  processes={child.role: {"pid": child.identity.pid, "start_ticks": child.identity.start_ticks}
                             for child in peers})
    result["idle"] = observe(supervisor, peers, seconds=IDLE_SECONDS)
    result["before_upload_lifetime_high_water"] = high_water(peers)
    result["upload"] = observe(supervisor, peers, seconds=UPLOAD_SECONDS, workload=workload)
    result["after_upload_lifetime_high_water"] = high_water(peers)
    supervisor.command(workload, "stop")
    stopped = supervisor.event(workload, "stopped", supervisor.clock() + 10)
    event_time(stopped, {}, responsive_at, supervisor.clock())
    supervisor.wait(workload, 10)
    os.killpg(composition.process.pid, signal.SIGTERM)
    supervisor.wait(composition, 15)
    os.killpg(mock.process.pid, signal.SIGTERM)
    stopped = supervisor.event(mock, "stopped", supervisor.clock() + 10)
    require(set(stopped) == {"event", "requests", "documents", "document_bytes", "failed"})
    require(type(stopped["documents"]) is int and type(stopped["document_bytes"]) is int
            and stopped["documents"] == 1 and stopped["document_bytes"] == driver.UPLOAD_BYTES
            and stopped["failed"] is False and type(stopped["requests"]) is int
            and 3 <= stopped["requests"] <= 256)
    supervisor.wait(mock, 10)
    result["mock_accounting"] = stopped
    require(collectors.file_identity(config) == config_identity)
    result["synthetic_config_identity"] = config_identity
    return result


def output_path(root, value):
    path = Path(os.path.abspath(value))
    allowed = root / "LOCAL_ARTIFACTS"
    require(path.is_relative_to(allowed) and path != allowed)
    # Never follow an output ancestor link or overwrite existing evidence.
    for parent in (path.parent, *path.parent.parents):
        require(not parent.is_symlink() and parent.is_dir())
    require(not path.exists() and not path.is_symlink())
    return path


def publish(path, record):
    data = (json.dumps(record, sort_keys=True, indent=2, allow_nan=False) + "\n").encode("utf-8")
    require(len(data) <= 8 * collectors.MAX_OUTPUT)
    directory = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    temporary = None
    created = False
    try:
        for part in path.parent.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                            dir_fd=directory)
            os.close(directory)
            directory = child
        temporary = ".relay-report-" + secrets.token_hex(16)
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o600, dir_fd=directory)
        created = True
        with os.fdopen(descriptor, "wb") as target:
            target.write(data)
            target.flush()
            os.fsync(target.fileno())
        os.link(temporary, path.name, src_dir_fd=directory, dst_dir_fd=directory,
                follow_symlinks=False)  # Atomic, exclusive, anchored to the opened directory.
    finally:
        try:
            if created:
                os.unlink(temporary, dir_fd=directory)
        finally:
            os.close(directory)


def run(root, output):
    record = {"schema_version": 1, "status": "failed", "phase": "prerequisites"}
    workspace = supervisor = workspace_id = None
    previous_signals = {}
    retired = True
    try:
        environment = build_environment(root)
        supervisor = Supervisor(root, environment)
        for kind in (signal.SIGINT, signal.SIGTERM):
            previous_signals[kind] = signal.signal(kind, lambda *_: setattr(supervisor, "cancelled", True))
        source = collectors.source_identity(root, command=supervisor.inspect)
        record["source"] = source
        receipt, receipt_id = prerequisites(root, source)
        require(not supervisor.cancelled)
        record["prerequisites"] = {"receipt_identity": receipt_id, "run_id": receipt["run_id"],
                                   "source": source, "checks": receipt["checks"]}
        workspace = Path(tempfile.mkdtemp(prefix="relay-measure-", dir=os.environ.get("TMPDIR"))).absolute()
        workspace.chmod(0o700)
        workspace_id = workspace.stat()
        record["phase"] = "materialization"
        profile = release_settings(root)
        daemon, probe = materialize(supervisor, workspace, source, receipt)
        build = collectors.collect_build(root, daemon, probe, command=supervisor.inspect)
        require(build["source"] == source and build["rustc"] == receipt["rustc"]
                and build["cargo"] == receipt["cargo"])
        build["release_settings"] = profile
        build["materialization_commands"] = [list(command) for command in BUILD_COMMANDS]
        build["python"] = sys.version.split()[0]
        settings = synthetic_settings(free_address())
        workload = {"upload_bytes": driver.UPLOAD_BYTES, "file": "sparse-zero-file",
                    "title": driver.TITLE, "caption": driver.CAPTION,
                    "external_session_id": driver.EXTERNAL_ID, "protocol": driver.VERSION,
                    "cadence_seconds": CADENCE, "idle_window_seconds": IDLE_SECONDS,
                    "upload_deadline_seconds": UPLOAD_SECONDS, "mock_upload_pacing": "none",
                    "mock_address": "numeric-loopback/kernel-assigned-port", "credentials": "none"}
        record["binding"] = collectors.evidence_binding(build, settings, workload)
        record["phase"] = "capture"
        record["measurements"] = {}
        capture(supervisor, workspace, probe, settings, record["measurements"])
        record["phase"] = "identity_recheck"
        again = collectors.collect_build(root, daemon, probe, command=supervisor.inspect)
        again.update({key: build[key] for key in ("release_settings", "materialization_commands", "python")})
        collectors.verify_binding(record["binding"], again, settings, workload)
        require(prerequisites(root, source)[1] == receipt_id)
        require(not supervisor.cancelled)
        record.update(status="success", phase="complete")
    except (collectors.MeasurementError, driver.DriverError, OSError, ValueError,
            TypeError, KeyError, AttributeError, subprocess.SubprocessError):
        # Keep only the static phase, never exception text, paths or child diagnostics.
        record["status"] = "failed"
    finally:
        if supervisor is not None:
            retired = supervisor.retire()
            record["owned_processes"] = [
                {"role": child.role, "pid": child.process.pid,
                 "start_ticks": child.identity.start_ticks if child.identity is not None else None,
                 "return_code": child.process.returncode, "group_retired": child.retired}
                for child in supervisor.children
            ]
        if not retired or (supervisor is not None and supervisor.forced is True):
            record.update(status="failed", phase="retirement")
        if workspace is not None and retired:
            try:
                current = workspace.lstat()
                require(workspace_id is not None and stat.S_ISDIR(current.st_mode)
                        and (current.st_dev, current.st_ino) == (workspace_id.st_dev, workspace_id.st_ino))
                shutil.rmtree(workspace)
            except (OSError, collectors.MeasurementError):
                record.update(status="failed", phase="cleanup")
        for kind, handler in previous_signals.items():
            signal.signal(kind, handler)
    record["owned_groups_retired"] = retired
    record["limitations"] = [
        "Probe links real services but adds readiness/signal channels and omits production diagnostics.",
        "Probe dev-feature unification differs from daemon; profiles and sizes are recorded separately.",
        "Controller, inspection/build children and filesystem cache are excluded from peer samples.",
        "Upload samples include priming/trailing observations; short spikes may fall between samples.",
        "VmHWM covers process lifetime, including startup and idle, not only upload.",
        "Synthetic results do not establish live Telegram, named-client interoperability or idle-host wake.",
        "Prerequisites are coordinator-exported public observations, not cryptographic Runner attestations.",
        "Failed/disconnected sends may have been accepted; no retry is performed.",
    ]
    publish(output, record)
    return 0 if record["status"] == "success" else 1


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise collectors.MeasurementError("invalid measurement arguments")


def main():
    parser = Parser(description=__doc__)
    parser.add_argument("--mock", required=True, action="store_true")
    parser.add_argument("--output", required=True)
    try:
        options = parser.parse_args()
        result = run(ROOT, output_path(ROOT, options.output))
        if result:
            print("Resource measurement failed; inspect the failure report.", file=sys.stderr)
        return result
    except (OSError, ValueError, driver.DriverError):
        print("Resource measurement failed; no acceptance is established.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
