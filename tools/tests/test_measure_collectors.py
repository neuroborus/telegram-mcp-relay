"""Deterministic collector contracts, without sockets, builds or resource measurements."""

from contextlib import contextmanager
from dataclasses import replace
import hashlib
import io
import os
import struct
import unittest
from unittest.mock import patch

from tools import measure_collectors as measure


def write_elf(path, *, debug=False, machine=62):
    """Small synthetic ELF64 section metadata, never executed."""
    names = b"\0.shstrtab\0.debug_info\0"
    count = 3 if debug else 2
    header = bytearray(64)
    header[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<HH", header, 16, 3, machine)
    struct.pack_into("<Q", header, 40, 64)
    struct.pack_into("<HHH", header, 58, 64, count, 1)
    sections = bytearray(64 * count)
    struct.pack_into("<II", sections, 64, 1, 3)
    struct.pack_into("<QQ", sections, 64 + 24, 64 + len(sections), len(names))
    if debug:
        struct.pack_into("<II", sections, 128, 11, 1)
    path.write_bytes(header + sections + names)
    path.chmod(0o700)


class CollectorTests(unittest.TestCase):
    def test_private_workspace_honors_tmpdir_and_cleans_only_owned_files_on_failure(self):
        with measure.private_workspace() as parent:
            sentinel = parent / "keep"
            sentinel.write_text("synthetic", encoding="utf-8")
            with patch.dict(os.environ, {"TMPDIR": str(parent)}):
                with self.assertRaisesRegex(RuntimeError, "controlled failure"):
                    with measure.private_workspace() as owned:
                        self.assertEqual(owned.parent, parent)
                        self.assertEqual(owned.stat().st_mode & 0o777, 0o700)
                        (owned / "workload").write_bytes(b"synthetic")
                        raise RuntimeError("controlled failure")
            self.assertFalse(owned.exists())
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "synthetic")

    def test_proc_fields_handle_comm_punctuation_and_use_linux_units(self):
        with measure.private_workspace() as root:
            directory = root / "73"
            directory.mkdir()
            fields = ["S"] + ["0"] * 21
            for index, value in ((11, 120), (12, 30), (19, 456), (21, 17)):
                fields[index] = str(value)
            path = directory / "stat"
            path.write_text("73 (synthetic ) worker) " + " ".join(fields), encoding="utf-8")
            self.assertEqual(
                measure.read_process(73, proc=root, clock=lambda: 2.5, page_size=4096),
                measure.ProcessSample(2.5, 73, 456, 150, 17 * 4096),
            )
            status = directory / "status"
            status.write_text("Name:\tsynthetic\nVmHWM:\t100 kB\n", encoding="utf-8")
            self.assertEqual(measure.read_high_water(73, 456, proc=root),
                             {"rss_high_water_bytes": 102400, "scope": "process_lifetime"})
            with self.assertRaises(measure.MeasurementError):
                measure.read_high_water(73, 999, proc=root)
            for malformed in ("VmHWM: 1 MB", "VmHWM: -1 kB", "", "VmHWM: NaN kB"):
                status.write_text(malformed, encoding="utf-8")
                with self.assertRaises(measure.MeasurementError):
                    measure.read_high_water(73, 456, proc=root)
            for data in ("invalid", "74 (wrong) " + " ".join(fields), "x" * 16385,
                         "73 (zombie) Z " + " ".join(fields[1:])):
                path.write_text(data, encoding="utf-8")
                with self.subTest(data=data[:30]), self.assertRaises(measure.MeasurementError):
                    measure.read_process(73, proc=root)
            path.unlink()
            with self.assertRaises(measure.MeasurementError):
                measure.read_process(73, proc=root)

    def test_idle_and_upload_aggregation_excludes_child_cpu_and_retains_sampled_peak(self):
        samples = [measure.ProcessSample(10, 73, 456, 100, 1000),
                   measure.ProcessSample(12, 73, 456, 101, 3000),
                   measure.ProcessSample(14, 73, 456, 104, 2000)]
        result = measure.summarize(samples, clock_ticks=100)
        self.assertEqual(result, {
            "samples": 3, "elapsed_seconds": 4, "rss_min_bytes": 1000,
            "rss_mean_bytes": 2000, "rss_peak_sampled_bytes": 3000,
            "cpu_seconds": 0.04, "cpu_percent_one_core": 1,
        })
        idle = measure.summarize([samples[0], replace(samples[1], cpu_ticks=100)], clock_ticks=100)
        self.assertEqual(idle["cpu_percent_one_core"], 0)

    def test_malformed_reused_process_and_unbounded_samples_are_rejected(self):
        first = measure.ProcessSample(1, 73, 456, 10, 1000)
        second = replace(first, monotonic_seconds=2, cpu_ticks=11)
        invalid = [replace(second, pid=74), replace(second, start_ticks=999),
                   replace(second, cpu_ticks=9), replace(second, rss_bytes=-1),
                   replace(second, rss_bytes=1 << 64),
                   replace(second, monotonic_seconds=1),
                   replace(second, monotonic_seconds=float("nan")),
                   replace(second, monotonic_seconds=602)]
        for sample in invalid:
            with self.subTest(sample=sample), self.assertRaises(measure.MeasurementError):
                measure.summarize([first, sample], clock_ticks=100)
        for samples in ([], [first], [first, object()]):
            with self.assertRaises(measure.MeasurementError):
                measure.summarize(samples, clock_ticks=100)
        with patch.object(measure, "MAX_SAMPLES", 2), self.assertRaises(measure.MeasurementError):
            measure.summarize([first, second, replace(second, monotonic_seconds=3)])

    def test_window_sampling_uses_injected_events_and_rejects_before_reading(self):
        elapsed = [0]
        reads = []

        def advance(seconds):
            elapsed[0] += seconds

        def read():
            reads.append(elapsed[0])
            return measure.ProcessSample(elapsed[0], 73, 456, 10, 1000)

        result = measure.collect_window(read, count=3, interval_seconds=0.5,
                                        sleep=advance, clock_ticks=100)
        self.assertEqual(reads, [0, 0.5, 1])
        self.assertEqual(result["elapsed_seconds"], 1)
        for count, interval in ((1, 1), (1202, 1), (3, 0), (1201, 5), (3, float("inf"))):
            with self.assertRaises(measure.MeasurementError):
                measure.collect_window(read, count=count, interval_seconds=interval)
        self.assertEqual(len(reads), 3)

    def test_startup_requires_ordered_readiness_and_mcp_response(self):
        self.assertEqual(measure.startup_times(10, 10.25, 10.5),
                         {"ready_seconds": 0.25, "mcp_responsive_seconds": 0.5})
        for values in ((2, 1, 3), (1, 3, 2), (1, 2, 62), (1, 2, float("nan")), (1, 2, 1 << 1024)):
            with self.assertRaises(measure.MeasurementError):
                measure.startup_times(*values)

    def test_file_hash_size_stripping_and_missing_prerequisites(self):
        with measure.private_workspace() as root:
            binary = root / "synthetic-binary"
            write_elf(binary)
            result = measure.file_identity(binary, binary=True)
            self.assertEqual(result["size_bytes"], binary.stat().st_size)
            self.assertEqual(result["sha256"], hashlib.sha256(binary.read_bytes()).hexdigest())
            self.assertTrue(result["stripped"])
            link = root / "link"
            link.symlink_to(binary)
            for path in (link, root / "missing", root):
                with self.assertRaises(measure.MeasurementError):
                    measure.file_identity(path)
            with patch.object(measure, "MAX_FILE", 16), self.assertRaises(measure.MeasurementError):
                measure.file_identity(binary)
            write_elf(binary, debug=True)
            with self.assertRaises(measure.MeasurementError):
                measure.file_identity(binary, binary=True)
            binary.write_bytes(b"not an ELF")
            with self.assertRaises(measure.MeasurementError):
                measure.file_identity(binary, binary=True)

    def test_mutation_during_hashing_cannot_produce_an_identity(self):
        with measure.private_workspace() as root:
            path = root / "file"
            path.write_bytes(b"original")
            original_hash = hashlib.sha256

            def changed_hash():
                path.write_bytes(b"changed length")
                return original_hash()

            with patch.object(measure.hashlib, "sha256", changed_hash):
                with self.assertRaises(measure.MeasurementError):
                    measure.file_identity(path)

    def test_build_evidence_binds_clean_source_binaries_toolchain_and_distinct_features(self):
        with measure.private_workspace() as root:
            (root / "Cargo.toml").write_text("synthetic manifest", encoding="utf-8")
            (root / "Cargo.lock").write_text("synthetic lock", encoding="utf-8")
            daemon, probe = root / "daemon", root / "probe"
            write_elf(daemon)
            write_elf(probe)
            commands = []
            dirty = [False]

            def command(args, *, cwd):
                self.assertEqual(cwd, root)
                commands.append(args)
                if args[:2] == ["git", "status"]:
                    return "?? unreviewed.py\n" if dirty[0] else ""
                if args[:3] == ["git", "rev-parse", "--show-toplevel"]:
                    return str(root)
                if args[0] == "git":
                    return "a" * 40
                if args[0] == "rustc":
                    return "rustc synthetic\nhost: x86_64-unknown-linux-gnu\n"
                if args == ["cargo", "--version"]:
                    return "cargo synthetic"
                self.assertEqual(args[:4], ["cargo", "tree", "--locked", "--offline"])
                features = "test-util" if "normal,build,dev" in args else ""
                return f"synthetic v0.1.0 ({root}) features=[{features}]\n"

            build = measure.collect_build(root, daemon, probe, command=command)
            self.assertEqual(build["source"]["head"], "a" * 40)
            self.assertNotEqual(build["dependency_features"]["daemon"], build["dependency_features"]["probe"])
            self.assertEqual(build["dependency_features"]["daemon"],
                             "synthetic v0.1.0 (<local>) features=[]\n")
            self.assertEqual(build["dependency_features"]["probe"],
                             "synthetic v0.1.0 (<local>) features=[test-util]\n")
            self.assertFalse(any("build" in args for args in commands))
            dirty[0] = True
            with self.assertRaises(measure.MeasurementError):
                measure.collect_build(root, daemon, probe, command=command)
            dirty[0] = False
            write_elf(probe, machine=183)
            with self.assertRaises(measure.MeasurementError):
                measure.collect_build(root, daemon, probe, command=command)

    def test_dependency_profile_binding_is_independent_of_local_checkout_locations(self):
        records = []
        for checkout in ("/home/synthetic/private checkout (copy)", "/tmp/synthetic/other"):
            profile = (
                f"relay v0.1.0 ({checkout}) features=[]\n"
                f"local-helper v1.0.0 ({checkout}/helper) features=[std]\n"
                "tokio v1.54.1 features=[rt,test-util] (*)\n"
                "git-crate v2.0.0 (https://example.invalid/crate#abc) features=[default]\n"
            )
            normalized = measure._normalize_profile_locations(profile)
            self.assertEqual(normalized, (
                "relay v0.1.0 (<local>) features=[]\n"
                "local-helper v1.0.0 (<local>) features=[std]\n"
                "tokio v1.54.1 features=[rt,test-util] (*)\n"
                "git-crate v2.0.0 (https://example.invalid/crate#abc) features=[default]\n"
            ))
            records.append(measure.evidence_binding({"dependency_features": normalized}, {}, {}))
        self.assertEqual(records[0], records[1])

    def test_evidence_rejects_tampering_stale_identity_settings_and_workload(self):
        build = {"source": {"head": "a" * 40}, "artifacts": {"probe": "synthetic hash"}}
        settings = {"max_outbound_mb": 50}
        workload = {"telegram": "separate-mock", "upload_bytes": 49_000_000}
        record = measure.evidence_binding(build, settings, workload)
        measure.verify_binding(record, build, settings, workload)
        for changed in ({**build, "target": "changed"}, {**build, "artifacts": {}},
                        {**build, "source": {"head": "b" * 40}}):
            with self.assertRaises(measure.MeasurementError):
                measure.verify_binding(record, changed, settings, workload)
        for changed_settings, changed_workload in (({}, workload), (settings, {})):
            with self.assertRaises(measure.MeasurementError):
                measure.verify_binding(record, build, changed_settings, changed_workload)
        settings["max_outbound_mb"] = 1
        self.assertEqual(record["settings"]["max_outbound_mb"], 50)
        record["identity_sha256"] = "forged"
        with self.assertRaises(measure.MeasurementError):
            measure.verify_binding(record, build, record["settings"], workload)
        with self.assertRaises(measure.MeasurementError):
            measure.evidence_binding(build, {}, {"invalid": float("nan")})
        record = measure.evidence_binding(build, {"count": 1}, workload)
        record["settings"]["count"] = True
        with self.assertRaises(measure.MeasurementError):
            measure.verify_binding(record, build, {"count": 1}, workload)

    def test_command_failures_output_bounds_and_cleanup_use_process_substitutes(self):
        class Process:
            pid = 12345
            returncode = 0

            def __init__(self):
                self.stdout = io.BytesIO()
                self.waited = 0

            def wait(self, **kwargs):
                self.waited += 1

        @contextmanager
        def substitute(process, chunks):
            with patch.object(measure.subprocess, "Popen", return_value=process) as popen, \
                    patch.object(measure.os, "killpg") as kill, \
                    patch.object(measure.os, "read", side_effect=chunks), \
                    patch.object(measure.selectors, "DefaultSelector") as selector:
                selected = selector.return_value.__enter__.return_value
                selected.get_map.side_effect = [True] * len(chunks) + [False]
                key = type("Key", (), {"fd": 1, "fileobj": process.stdout})()
                selected.select.return_value = [(key, 1)]
                yield kill
                self.assertEqual(popen.call_args.kwargs["env"]["GIT_OPTIONAL_LOCKS"], "0")

        process = Process()
        with substitute(process, [b"synthetic", b""]) as kill:
            self.assertEqual(measure.run_command(["rustc", "-vV"], cwd="."), "synthetic")
        kill.assert_called_once()
        self.assertGreater(process.waited, 0)
        for chunks in ([b"x" * 9], [b"\xff", b""], [OSError("controlled")]):
            process = Process()
            with substitute(process, chunks) as kill, patch.object(measure, "MAX_OUTPUT", 8):
                with self.assertRaises(measure.MeasurementError):
                    measure.run_command(["rustc", "-vV"], cwd=".")
            kill.assert_called_once()
            self.assertGreater(process.waited, 0)
        with patch.object(measure.subprocess, "Popen", side_effect=FileNotFoundError):
            with self.assertRaises(measure.MeasurementError):
                measure.run_command(["rustc", "-vV"], cwd=".")
        process = Process()
        process.returncode = 1
        with substitute(process, [b""]), self.assertRaises(measure.MeasurementError):
            measure.run_command(["rustc", "-vV"], cwd=".")
        process = Process()
        with substitute(process, [b""]), \
                patch.object(measure.time, "monotonic", side_effect=[0, 31]), \
                self.assertRaises(measure.MeasurementError):
            measure.run_command(["rustc", "-vV"], cwd=".")
        with patch.object(measure.subprocess, "Popen") as popen:
            with self.assertRaises(measure.MeasurementError):
                measure.run_command(["cargo", "build", "--release"], cwd=".")
            popen.assert_not_called()


if __name__ == "__main__":
    unittest.main()
