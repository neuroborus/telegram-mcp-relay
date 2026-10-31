"""Bounded Linux resource/build collectors; runtime orchestration belongs to HOST."""

from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
import hashlib
import json
import math
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import struct
import subprocess
import tempfile
import time


MAX_OUTPUT = 2 * 1024 * 1024
MAX_FILE = 256 * 1024 * 1024
MAX_SAMPLES = 1201
MAX_WINDOW_SECONDS = 600
MAX_COUNTER = (1 << 64) - 1
NATIVE_TARGETS = {"x86_64-unknown-linux-gnu": 62, "x86_64-unknown-linux-musl": 62,
                  "aarch64-unknown-linux-gnu": 183, "aarch64-unknown-linux-musl": 183}
EVIDENCE_COMMANDS = {
    ("git", "rev-parse", "--show-toplevel"),
    ("git", "status", "--porcelain", "--untracked-files=normal"),
    ("git", "rev-parse", "--verify", "HEAD"),
    ("git", "rev-parse", "HEAD^{tree}"),
    ("rustc", "-vV"), ("cargo", "--version"),
} | {
    ("cargo", "tree", "--locked", "--offline", "--target", target,
     "--edges", edges, "--prefix", "none", "--format", "{p} features=[{f}]")
    for target in NATIVE_TARGETS for edges in ("normal,build", "normal,build,dev")
}


class MeasurementError(ValueError):
    """A prerequisite or observation is invalid; no acceptance result is implied."""


@contextmanager
def private_workspace():
    """Own only an exclusive mode-0700 directory under the configured TMPDIR."""
    with tempfile.TemporaryDirectory(prefix="relay-measure-", dir=os.environ.get("TMPDIR")) as directory:
        yield Path(directory)


def run_command(arguments, *, cwd, timeout=30, environment=None):
    """Run only closed inspection vectors, retiring children on any failure."""
    arguments = tuple(arguments)
    if arguments not in EVIDENCE_COMMANDS:
        raise MeasurementError("unsupported evidence command")
    if not 0 < timeout <= 60:
        raise MeasurementError("invalid command deadline")
    try:
        process = subprocess.Popen(
            arguments, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL, start_new_session=True,
            env={**(os.environ if environment is None else environment), "GIT_OPTIONAL_LOCKS": "0"},
        )
    except OSError as error:
        raise MeasurementError("missing or unusable command prerequisite") from error
    try:
        deadline = time.monotonic() + timeout
        output = bytearray()
        with process.stdout, selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise MeasurementError("command deadline exceeded")
                for key, _ in selector.select(remaining):
                    chunk = os.read(key.fd, min(65536, MAX_OUTPUT + 1 - len(output)))
                    if not chunk:
                        selector.unregister(key.fileobj)
                    output.extend(chunk)
                    if len(output) > MAX_OUTPUT:
                        raise MeasurementError("command output exceeded bound")
        process.wait(timeout=max(0, deadline - time.monotonic()))
        if process.returncode:
            raise MeasurementError("evidence command failed")
        return output.decode("utf-8", errors="strict")
    except (OSError, UnicodeError, subprocess.TimeoutExpired) as error:
        raise MeasurementError("unusable evidence command result") from error
    finally:
        # A descendant may retain a pipe or outlive its parent. Never leave it running.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()


def _bounded_read(path, limit):
    with path.open("rb") as source:
        data = source.read(limit + 1)
    if len(data) > limit:
        raise MeasurementError("input exceeded bound")
    return data


def _stamp(metadata):
    return (metadata.st_dev, metadata.st_ino, metadata.st_size,
            metadata.st_mtime_ns, metadata.st_ctime_ns)


def _elf_machine(source, size):
    """Require a stripped ELF64 executable, inspecting bounded section metadata only."""
    source.seek(0)
    header = source.read(64)
    if len(header) != 64 or header[:6] != b"\x7fELF\x02\x01":
        raise MeasurementError("expected a little-endian Linux ELF64 binary")
    kind, machine = struct.unpack_from("<HH", header, 16)
    offset = struct.unpack_from("<Q", header, 40)[0]
    entry_size, count, names_index = struct.unpack_from("<HHH", header, 58)
    if (kind not in (2, 3) or entry_size != 64 or not 0 < count <= 4096
            or not 0 < names_index < count or offset < 64
            or offset + count * entry_size > size):
        raise MeasurementError("unsupported ELF section table")
    source.seek(offset)
    sections = source.read(count * entry_size)
    names_offset, names_size = struct.unpack_from("<QQ", sections, names_index * 64 + 24)
    if not 0 < names_size <= 1024 * 1024 or names_offset + names_size > size:
        raise MeasurementError("invalid ELF section names")
    source.seek(names_offset)
    names = source.read(names_size)
    for index in range(count):
        name_index, section_type = struct.unpack_from("<II", sections, index * 64)
        end = names.find(b"\0", name_index)
        if name_index >= len(names) or end < 0:
            raise MeasurementError("invalid ELF section name")
        name = names[name_index:end]
        if section_type == 2 or name.startswith((b".debug", b".zdebug")):
            raise MeasurementError("release binary is not stripped")
    return machine


def file_identity(path, *, binary=False):
    """Stream hashes from regular non-symlink files; reject observed replacement/mutation."""
    path = Path(path)
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            before = os.fstat(source.fileno())
            if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= MAX_FILE:
                raise MeasurementError("missing or excessive identity file")
            if binary and not before.st_mode & 0o111:
                raise MeasurementError("release binary is not executable")
            digest = hashlib.sha256()
            total = 0
            while True:
                chunk = source.read(min(65536, MAX_FILE + 1 - total))
                if not chunk:
                    break
                digest.update(chunk)
                total += len(chunk)
                if total > MAX_FILE:
                    raise MeasurementError("identity file exceeded bound")
            identity = {"sha256": digest.hexdigest(), "size_bytes": total}
            if binary:
                identity.update(elf_machine=_elf_machine(source, total), stripped=True)
            if (total != before.st_size or _stamp(before) != _stamp(os.fstat(source.fileno()))
                    or _stamp(before) != _stamp(path.lstat())):
                raise MeasurementError("identity file changed during collection")
            return identity
    except (OSError, struct.error) as error:
        raise MeasurementError("missing or unreadable identity file") from error


def source_identity(root, *, command=run_command):
    """Bind only a clean accepted Git tree, including nonignored untracked files."""
    root = Path(root).resolve()
    def git(*args):
        return command(["git", *args], cwd=root)

    if Path(git("rev-parse", "--show-toplevel").strip()).resolve() != root:
        raise MeasurementError("expected the source repository root")
    if git("status", "--porcelain", "--untracked-files=normal"):
        raise MeasurementError("measurement source must be clean and committed")
    head = git("rev-parse", "--verify", "HEAD").strip()
    tree = git("rev-parse", "HEAD^{tree}").strip()
    if any(len(value) not in (40, 64) or any(c not in "0123456789abcdef" for c in value)
           for value in (head, tree)):
        raise MeasurementError("invalid Git identity")
    return {"head": head, "tree": tree,
            "manifest": file_identity(root / "Cargo.toml"),
            "lock": file_identity(root / "Cargo.lock")}


def _normalize_profile_locations(profile):
    """Keep Cargo package/version/features while removing absolute local source locations."""
    return re.sub(r" \(/.*\)(?= features=\[)", " (<local>)", profile)


def collect_build(root, daemon, probe, *, command=run_command):
    """Inspect already-built artifacts, never compile or fetch missing prerequisites."""
    root = Path(root)
    source = source_identity(root, command=command)
    rustc = command(["rustc", "-vV"], cwd=root).strip()
    cargo = command(["cargo", "--version"], cwd=root).strip()
    hosts = [line[6:] for line in rustc.splitlines() if line.startswith("host: ")]
    if not cargo or len(hosts) != 1 or hosts[0] not in NATIVE_TARGETS:
        raise MeasurementError("unsupported native Linux measurement target")
    target = hosts[0]
    artifacts = {"daemon": file_identity(daemon, binary=True),
                 "probe": file_identity(probe, binary=True)}
    if any(item["elf_machine"] != NATIVE_TARGETS[target] for item in artifacts.values()):
        raise MeasurementError("binary and native toolchain target differ")
    profiles = {}
    for name, edges in (("daemon", "normal,build"), ("probe", "normal,build,dev")):
        profiles[name] = _normalize_profile_locations(command([
            "cargo", "tree", "--locked", "--offline", "--target", target,
            "--edges", edges, "--prefix", "none", "--format", "{p} features=[{f}]",
        ], cwd=root))
        if not profiles[name].strip():
            raise MeasurementError("missing dependency feature profile")
    if source_identity(root, command=command) != source:
        raise MeasurementError("source changed during build evidence collection")
    if artifacts != {"daemon": file_identity(daemon, binary=True),
                     "probe": file_identity(probe, binary=True)}:
        raise MeasurementError("binaries changed during build evidence collection")
    return {"source": source, "rustc": rustc, "cargo": cargo, "target": target,
            "profile": "release", "artifacts": artifacts, "dependency_features": profiles}


def evidence_binding(build, settings, workload):
    """Bind exact nonsecret synthetic settings/workload and observed build identities."""
    record = {"schema_version": 1, "build": build, "settings": settings, "workload": workload}
    try:
        data = json.dumps(record, sort_keys=True, separators=(",", ":"), allow_nan=False)
        if len(data.encode("utf-8")) > 4 * MAX_OUTPUT:
            raise MeasurementError("evidence identity exceeded bound")
        # Copy through JSON so later mutation of caller dictionaries cannot alter the binding.
        record = json.loads(data)
    except (TypeError, ValueError) as error:
        raise MeasurementError("invalid evidence identity") from error
    record["identity_sha256"] = hashlib.sha256(data.encode("utf-8")).hexdigest()
    return record


def verify_binding(record, build, settings, workload):
    """Reject stale source, artifact, settings or workload evidence before integration."""
    expected = evidence_binding(build, settings, workload)
    try:
        actual_json = json.dumps(record, sort_keys=True, separators=(",", ":"), allow_nan=False)
        expected_json = json.dumps(expected, sort_keys=True, separators=(",", ":"), allow_nan=False)
    except (TypeError, ValueError) as error:
        raise MeasurementError("invalid evidence identity") from error
    # Canonical comparison distinguishes JSON booleans, integers and floating-point values.
    if actual_json != expected_json:
        raise MeasurementError("evidence identity mismatch")


@dataclass(frozen=True)
class ProcessSample:
    monotonic_seconds: float
    pid: int
    start_ticks: int
    cpu_ticks: int
    rss_bytes: int


def _finite(value):
    try:
        return type(value) in (int, float) and math.isfinite(value) and value >= 0
    except OverflowError:
        return False


def read_process(pid, *, proc=Path("/proc"), clock=time.monotonic, page_size=None):
    """Read one bounded /proc/PID/stat snapshot; comm may contain spaces and parentheses."""
    if type(pid) is not int or pid <= 0:
        raise MeasurementError("invalid process ID")
    try:
        data = _bounded_read(Path(proc) / str(pid) / "stat", 16384).decode("utf-8")
        prefix, fields = data.rsplit(") ", 1)
        if int(prefix.split(" (", 1)[0]) != pid:
            raise ValueError()
        fields = fields.split()
        if fields[0] not in ("R", "S", "D", "T", "t", "W", "I", "P"):
            raise ValueError()
        user, system, start, resident = (int(fields[index]) for index in (11, 12, 19, 21))
        page_size = os.sysconf("SC_PAGE_SIZE") if page_size is None else page_size
        if min(user, system, start, resident) < 0 or type(page_size) is not int or page_size <= 0:
            raise ValueError()
        if max(user + system, start, resident * page_size) > MAX_COUNTER:
            raise ValueError()
        timestamp = clock()
        if not _finite(timestamp):
            raise ValueError()
        return ProcessSample(timestamp, pid, start, user + system, resident * page_size)
    except (OSError, UnicodeError, ValueError, IndexError) as error:
        raise MeasurementError("missing or malformed Linux process sample") from error


def read_high_water(pid, start_ticks, *, proc=Path("/proc")):
    """Kernel RSS high water catches spikes between samples, but covers the whole lifetime."""
    before = read_process(pid, proc=proc)
    try:
        status = _bounded_read(Path(proc) / str(pid) / "status", 65536).decode("utf-8")
        values = [line.split()[1:] for line in status.splitlines() if line.startswith("VmHWM:")]
        if len(values) != 1 or len(values[0]) != 2 or values[0][1] != "kB":
            raise ValueError()
        high_water = int(values[0][0]) * 1024
        if not 0 <= high_water <= MAX_COUNTER:
            raise ValueError()
    except (OSError, UnicodeError, ValueError) as error:
        raise MeasurementError("missing or malformed Linux RSS high water") from error
    after = read_process(pid, proc=proc)
    if before.start_ticks != start_ticks or after.start_ticks != start_ticks:
        raise MeasurementError("process identity changed during high-water reading")
    return {"rss_high_water_bytes": high_water, "scope": "process_lifetime"}


def summarize(samples, *, clock_ticks=None):
    """RSS uses bytes; CPU percent is one logical CPU, excluding children/mock/driver."""
    clock_ticks = os.sysconf("SC_CLK_TCK") if clock_ticks is None else clock_ticks
    if type(clock_ticks) is not int or clock_ticks <= 0:
        raise MeasurementError("invalid Linux clock tick frequency")
    # Do not materialize an unbounded iterator supplied by a caller.
    bounded = []
    for sample in samples:
        if len(bounded) == MAX_SAMPLES:
            raise MeasurementError("sample count exceeded bound")
        if (not isinstance(sample, ProcessSample) or not _finite(sample.monotonic_seconds)
                or any(type(value) is not int or not 0 <= value <= MAX_COUNTER for value in
                       (sample.pid, sample.start_ticks, sample.cpu_ticks, sample.rss_bytes))
                or sample.pid == 0):
            raise MeasurementError("invalid process sample")
        if bounded:
            previous = bounded[-1]
            if ((sample.pid, sample.start_ticks) != (previous.pid, previous.start_ticks)
                    or sample.monotonic_seconds <= previous.monotonic_seconds
                    or sample.cpu_ticks < previous.cpu_ticks):
                raise MeasurementError("process identity or counters changed")
        bounded.append(sample)
    if len(bounded) < 2:
        raise MeasurementError("at least two process samples are required")
    first, last = bounded[0], bounded[-1]
    elapsed = last.monotonic_seconds - first.monotonic_seconds
    if elapsed > MAX_WINDOW_SECONDS:
        raise MeasurementError("sample window exceeded bound")
    rss = [sample.rss_bytes for sample in bounded]
    cpu_seconds = (last.cpu_ticks - first.cpu_ticks) / clock_ticks
    return {"samples": len(bounded), "elapsed_seconds": elapsed,
            "rss_min_bytes": min(rss), "rss_mean_bytes": sum(rss) / len(rss),
            "rss_peak_sampled_bytes": max(rss), "cpu_seconds": cpu_seconds,
            "cpu_percent_one_core": 100 * cpu_seconds / elapsed}


def collect_window(read_sample, *, count, interval_seconds, sleep=time.sleep, clock_ticks=None):
    """Bound observation count/duration; tests inject readers and a clock-advancing sleep."""
    if (type(count) is not int or not 2 <= count <= MAX_SAMPLES
            or not _finite(interval_seconds) or not 0.01 <= interval_seconds <= 5
            or (count - 1) * interval_seconds > MAX_WINDOW_SECONDS):
        raise MeasurementError("invalid observation window")

    def observations():
        for index in range(count):
            if index:
                sleep(interval_seconds)
            yield read_sample()

    return summarize(observations(), clock_ticks=clock_ticks)


def startup_times(launched_at, ready_at, responsive_at):
    """MCP responsiveness must be confirmed externally, beyond the probe readiness line."""
    if (not all(_finite(value) for value in (launched_at, ready_at, responsive_at))
            or not launched_at <= ready_at <= responsive_at
            or responsive_at - launched_at > 60):
        raise MeasurementError("invalid or excessive startup interval")
    return {"ready_seconds": ready_at - launched_at,
            "mcp_responsive_seconds": responsive_at - launched_at}
