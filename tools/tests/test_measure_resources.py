"""Controlled orchestration decisions only; no sockets, Cargo execution or measurements."""

from contextlib import ExitStack
import copy
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from tools import measure_resources as measure


SOURCE = {"head": "a" * 40, "tree": "b" * 40, "manifest": {"sha256": "c" * 64},
          "lock": {"sha256": "d" * 64}}

# Independently specified from the persisted step-3 inventory, not the validator's constants.
STEP_THREE_CHECKS = (
    ("C1", "python3 tools/check_repository.py", "native"),
    ("C2", "git diff --check HEAD", "native"),
    ("C3", "cargo fmt --all -- --check", "native"),
    ("C4", "cargo clippy --locked --workspace --all-targets -- -D warnings", "runner-trusted"),
    ("C5", 'RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items', "runner-trusted"),
    ("C6", "cargo test --locked --workspace", "runner-trusted"),
    ("C7", "cargo build --locked --release --bin telegram-mcp-relay", "native"),
    ("C8", "cargo build --locked --release --example resource_probe", "native"),
    ("C9", 'python3 -B -c "import ast,pathlib; [ast.parse(p.read_text(encoding=\'utf-8\'), filename=str(p)) for p in pathlib.Path(\'tools\').rglob(\'*.py\')]"', "native"),
    ("C10", "python3 -B -m unittest discover -s tools/tests -p 'test_measure*.py'", "native"),
)


def receipt():
    return {"schema_version": 1, "source": SOURCE, "step": 3, "mode": "independent",
            "pipeline": "plan-execution", "run_id": "11111111-2222-3333-4444-555555555555",
            "rustc": "rustc synthetic", "cargo": "cargo synthetic",
            "public_status": {"runId": "11111111-2222-3333-4444-555555555555",
                              "completedCommits": [SOURCE["head"]]},
            "checks": [{"id": key, "command": command, "status": "PASS", "public_revision": 1,
                        "route": route} for key, command, route in STEP_THREE_CHECKS]}


def child(role, pid=101):
    process = Mock(pid=pid, returncode=0)
    process.poll.return_value = None
    process.wait.return_value = 0
    return measure.Child(role, process, 0,
                         measure.collectors.ProcessSample(0, pid, 7, 0, 1024), measure.PIPE_BYTES)


class ResourceTests(unittest.TestCase):
    def test_default_bytecode_cli_startup_leaves_source_tree_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tools = root / "tools"
            tools.mkdir()
            names = ("measure_resources.py", "measure_collectors.py", "measure_driver.py")
            for name in names:
                shutil.copyfile(Path(measure.__file__).parent / name, tools / name)
            expected = {Path("tools"), *(Path("tools") / name for name in names)}
            # A fresh interpreter has no -B; -E ignores inherited Python cache settings.
            # --help exercises the real imports without starting builds or a HOST workload.
            result = subprocess.run(
                [sys.executable, "-E", "-S", "tools/measure_resources.py", "--help"], cwd=root,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                timeout=10, check=False,
            )
            self.assertEqual(result.returncode, 0)
            self.assertEqual({path.relative_to(root) for path in root.rglob("*")}, expected)

    def test_step_three_receipts_bind_source_exact_commands_routes_and_public_commit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / measure.RECEIPTS
            path.parent.mkdir()
            path.write_text(json.dumps(receipt()), encoding="utf-8")
            accepted, identity = measure.prerequisites(root, SOURCE)
            self.assertEqual(accepted, receipt())
            self.assertEqual(identity, measure.collectors.file_identity(path))
            cases = [{"source": {**SOURCE, "tree": "e" * 40}}, {"step": 2}, {"mode": "lazy"},
                     {"public_status": {"completedCommits": []}}, {"checks": []}, {"schema_version": 2}]
            for index, field, value in ((0, "status", "NOT_RUN"), (0, "command", "cargo test --lib"),
                                       (0, "route", "host"), (0, "public_revision", True),
                                       (0, "private_path", "/private/path"),
                                       (6, "command", "cargo build --locked --offline --release --bin telegram-mcp-relay"),
                                       (6, "route", "runner-trusted"), (9, "status", "NOT_RUN")):
                changed = copy.deepcopy(receipt()["checks"])
                changed[index][field] = value
                cases.append({"checks": changed})
            bootstrap_ids = {"C7": "C11", "C8": "C12", "C9": "C13", "C10": "C14"}
            bootstrap = copy.deepcopy(receipt()["checks"])
            for item in bootstrap:
                item["id"] = bootstrap_ids.get(item["id"], item["id"])
            cases.append({"checks": bootstrap})
            duplicate = copy.deepcopy(receipt()["checks"])
            duplicate[-1] = duplicate[0]
            cases.append({"checks": duplicate})
            for change in cases:
                path.write_text(json.dumps({**receipt(), **change}), encoding="utf-8")
                with self.subTest(change=change), self.assertRaises(measure.collectors.MeasurementError):
                    measure.prerequisites(root, SOURCE)
            path.write_text('{"schema_version":1,"schema_version":1}', encoding="utf-8")
            with self.assertRaises(measure.driver.DriverError):
                measure.prerequisites(root, SOURCE)
            path.unlink()
            with self.assertRaises(measure.collectors.MeasurementError):
                measure.prerequisites(root, SOURCE)

    def test_profile_environment_and_implicit_config_rejection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            home = root / "cargo-home"
            home.mkdir()
            environment = {"HOME": str(root), "CARGO_HOME": str(home), "PATH": "/synthetic/bin"}
            with patch.dict(os.environ, environment, clear=True), patch.object(measure.shutil, "which", return_value="tool"):
                self.assertEqual(measure.build_environment(root), {**environment, "RUSTUP_AUTO_INSTALL": "0",
                                                                  "RUSTUP_NO_UPDATE_CHECK": "1"})
                for key in ("RUSTFLAGS", "RUSTC_WRAPPER", "CARGO_PROFILE_RELEASE_LTO", "CARGO_BUILD_TARGET", "GIT_INDEX_FILE"):
                    with patch.dict(os.environ, {key: ""}), self.assertRaises(measure.collectors.MeasurementError):
                        measure.build_environment(root)
                config = home / "config.toml"
                config.write_text("# synthetic override", encoding="utf-8")
                with self.assertRaises(measure.collectors.MeasurementError):
                    measure.build_environment(root)
            manifest = root / "Cargo.toml"
            manifest.write_text('[profile.release]\nlto = "thin"\ncodegen-units = 1\nstrip = true\n', encoding="utf-8")
            self.assertTrue(measure.release_settings(root)["strip"])
            manifest.write_text('[profile.release]\nstrip = false\n', encoding="utf-8")
            with self.assertRaises(measure.collectors.MeasurementError):
                measure.release_settings(root)

    def test_config_preserves_limits_and_confines_only_synthetic_roles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            settings = measure.synthetic_settings("127.0.0.1:8765")
            config = measure.write_config(root, settings)
            text = config.read_text(encoding="utf-8")
            self.assertIn('allowed_chat_ids = [-101]', text)
            self.assertIn('allowed_user_ids = [202]', text)
            self.assertIn('long_poll_timeout_seconds = 45', text)
            self.assertIn('max_outbound_mb = 50', text)
            self.assertIn('upload_buffer_bytes = 65536', text)
            for key in ("auth_token_env", "auth_token_file", "bot_token_env", "bot_token_file"):
                self.assertNotRegex(text, rf"(?m)^{key}\s*=")
            self.assertIn("create_auth_token_file = false\n", text)
            self.assertEqual(config.stat().st_mode & 0o777, 0o600)
            for name in ("database", "export"):
                self.assertEqual((root / name).stat().st_mode & 0o777, 0o700)
            self.assertNotIn(str(root), json.dumps(settings))
            self.assertIn(str(root / "export"), text)
            self.assertEqual(settings["storage"]["path"], "<private-database>/relay.sqlite")

    def test_bounded_pipe_framing_discards_diagnostics_and_rejects_partial_eof(self):
        peer = child("mock")
        peer.feed("stdout", b'{"event":')
        peer.feed("stdout", b'"ready"}\n')
        self.assertEqual(peer.events, [{"event": "ready"}])
        peer.feed("stderr", b"PrivateToken /private/path")
        self.assertNotIn(b"Private", peer.buffer)
        for role, channel, data in (("mock", "stdout", b"x" * (measure.PIPE_BYTES + 1)),
                                    ("build", "stderr", b"x" * (measure.PIPE_BYTES + 1))):
            with self.assertRaises(measure.collectors.MeasurementError):
                child(role).feed(channel, data)
        peer.feed("stdout", b"partial")
        with self.assertRaises(measure.collectors.MeasurementError):
            peer.feed("stdout", b"")

    def test_final_event_is_drained_even_after_process_exit_and_eof_is_failure(self):
        supervisor = measure.Supervisor(Path("."), {}, clock=lambda: 0)
        try:
            peer = child("mock")
            peer.process.poll.return_value = 0
            supervisor.pump = lambda _: peer.feed("stdout", b'{"event":"stopped"}\n')
            self.assertEqual(supervisor.event(peer, "stopped", 1), {"event": "stopped"})
            supervisor.pump = lambda _: peer.feed("stdout", b"")
            with self.assertRaises(measure.collectors.MeasurementError):
                supervisor.event(peer, "ready", 1)
        finally:
            supervisor.selector.close()

    def test_launch_records_ownership_before_proc_failure_and_always_retires(self):
        with patch.object(measure.selectors, "DefaultSelector"), \
                patch.object(measure.subprocess, "Popen", return_value=child("probe").process), \
                patch.object(measure.os, "set_blocking"), \
                patch.object(measure.collectors, "read_process", side_effect=ValueError("synthetic")), \
                patch.object(measure.os, "killpg", side_effect=ProcessLookupError):
            supervisor = measure.Supervisor(Path("."), {})
            with self.assertRaises(ValueError):
                supervisor.launch("probe", ["synthetic-probe"])
            self.assertEqual(len(supervisor.children), 1)
            self.assertTrue(supervisor.retire())
            supervisor.children[0].process.wait.assert_called()

    def test_retirement_attempts_all_groups_and_never_certifies_survivors(self):
        for survivor in (False, True):
            supervisor = measure.Supervisor(Path("."), {})
            supervisor.children = [child("mock", 101), child("probe", 102), child("driver", 103)]
            calls = []

            def kill(pid, kind):
                calls.append((pid, kind))
                if kind == 0 and not survivor:
                    raise ProcessLookupError()

            with patch.object(measure.os, "killpg", side_effect=kill):
                self.assertEqual(supervisor.retire(), not survivor)
            self.assertEqual([pid for pid, kind in calls if kind == signal.SIGTERM], [103, 102, 101])
            for peer in supervisor.children:
                peer.process.wait.assert_called()
                peer.process.stdout.close.assert_called_once()

    def test_cancellation_prevents_launch_and_dispatch_and_retired_groups_are_not_signalled(self):
        supervisor = measure.Supervisor(Path("."), {})
        peer = child("driver")
        supervisor.children = [peer]
        supervisor.cancelled = True
        peer.retired = True
        with patch.object(measure.subprocess, "Popen") as launch, patch.object(measure.os, "write") as write, \
                patch.object(measure.os, "killpg") as kill:
            with self.assertRaises(measure.collectors.MeasurementError):
                supervisor.launch("mock", ["synthetic"])
            with self.assertRaises(measure.collectors.MeasurementError):
                supervisor.command(peer, "upload")
            self.assertTrue(supervisor.retire())
            launch.assert_not_called()
            write.assert_not_called()
            kill.assert_not_called()

    def test_materialization_requires_receipts_before_exact_offline_private_builds(self):
        supervisor = Mock(root=Path("/synthetic/source"), environment={"PATH": "/synthetic/bin"})
        supervisor.launch.return_value = child("build")
        command = lambda args, **_: "rustc synthetic" if args[0] == "rustc" else "cargo synthetic"
        supervisor.inspect.side_effect = command
        with patch.object(measure.collectors, "source_identity", return_value=SOURCE):
            daemon, probe = measure.materialize(supervisor, Path("/synthetic/private"), SOURCE, receipt())
            self.assertEqual([call.args[1] for call in supervisor.launch.call_args_list], list(measure.BUILD_COMMANDS))
            self.assertTrue(str(daemon).startswith("/synthetic/private/target/"))
            self.assertTrue(str(probe).endswith("/examples/resource_probe"))
            for call in supervisor.launch.call_args_list:
                self.assertEqual(call.kwargs["env"]["CARGO_TARGET_DIR"], "/synthetic/private/target")
                self.assertEqual(call.kwargs["env"]["CARGO_NET_OFFLINE"], "true")
            supervisor.reset_mock()
            with self.assertRaises(measure.collectors.MeasurementError):
                measure.materialize(supervisor, Path("/synthetic/private"), SOURCE, {**receipt(), "rustc": "stale"})
            supervisor.launch.assert_not_called()

    def test_short_upload_has_priming_trailing_samples_and_distinct_event_interval(self):
        now = [0.0]
        peers = [child("probe", 101), child("mock", 102), child("driver", 103)]
        supervisor = Mock(clock=lambda: now[0])
        workload = peers[-1]
        sent = [False]

        def pump(seconds):
            now[0] += seconds
            if now[0] >= 0.2 and not sent[0]:
                workload.events.append({"event": "uploaded", "at_monotonic_seconds": 0.2,
                    "upload_bytes": measure.driver.UPLOAD_BYTES, "telegram_chat_id": -101,
                    "outbound_message_id": 7})
                sent[0] = True

        supervisor.pump.side_effect = pump
        supervisor.event.side_effect = lambda peer, name, _: (peer.events.pop(0) if name == "uploaded" else
            {"event": "upload_started", "at_monotonic_seconds": 0, "upload_bytes": measure.driver.UPLOAD_BYTES})
        supervisor.command.side_effect = None
        read = lambda pid: measure.collectors.ProcessSample(now[0], pid, 7, 0, pid * 1024)
        with patch.object(measure.collectors, "read_process", side_effect=read):
            result = measure.observe(supervisor, peers, seconds=2, workload=workload)
        self.assertEqual(now[0], 0.5)
        self.assertEqual(result["request_interval"]["completed_monotonic_seconds"], 0.2)
        for role in ("probe", "mock", "driver"):
            self.assertEqual(result[role]["summary"]["samples"], 2)
            self.assertEqual(result[role]["summary"]["elapsed_seconds"], 0.5)
        with patch.object(measure.collectors, "read_process", return_value=read(999)), \
                self.assertRaises(measure.collectors.MeasurementError):
            measure.snapshot(peers)

    def test_capture_coordinates_real_peer_interfaces_and_checks_drained_bytes(self):
        for drained in (measure.driver.UPLOAD_BYTES, 12):
            supervisor = Mock(clock=lambda: 10)
            peers = {role: child(role, pid) for role, pid in (("probe", 101), ("mock", 102), ("driver", 103))}
            supervisor.launch.side_effect = lambda role, *args: peers[role]
            results = [
                {"event": "ready", "mock_address": "127.0.0.1:9000"},
                {"event": "ready", "mcp_address": "127.0.0.1:8765"},
                {"event": "responsive", "at_monotonic_seconds": 10, "protocol_version": measure.driver.VERSION},
                {"event": "idle", "at_monotonic_seconds": 10, "upload_bytes": measure.driver.UPLOAD_BYTES},
                {"event": "stopped", "at_monotonic_seconds": 10},
                {"event": "stopped", "requests": 4, "documents": 1, "document_bytes": drained, "failed": False},
            ]
            supervisor.event.side_effect = results
            record = {}
            with patch.object(measure, "write_config", return_value=Path("/synthetic/config")), \
                    patch.object(measure.collectors, "file_identity", return_value={"sha256": "a" * 64}), \
                    patch.object(measure, "observe", return_value={"synthetic": True}) as observe, \
                    patch.object(measure, "high_water", return_value={"scope": "process_lifetime"}), \
                    patch.object(measure.os, "killpg"):
                if drained == 12:
                    with self.assertRaises(measure.collectors.MeasurementError):
                        measure.capture(supervisor, Path("/synthetic"), Path("probe"),
                                        measure.synthetic_settings("127.0.0.1:8765"), record)
                    self.assertIn("idle", record)  # Retain completed phases after workload failure.
                else:
                    measure.capture(supervisor, Path("/synthetic"), Path("probe"),
                                    measure.synthetic_settings("127.0.0.1:8765"), record)
                    self.assertEqual(record["mock_accounting"]["document_bytes"], measure.driver.UPLOAD_BYTES)
                    self.assertEqual(set(record["processes"]), {"probe", "mock", "driver"})
                    self.assertEqual(observe.call_args_list[0].kwargs["seconds"], 50)
                    self.assertIs(observe.call_args_list[1].kwargs["workload"], peers["driver"])

    def test_runtime_failure_identity_change_and_unverified_retirement_preserve_failed_report(self):
        for failure in ("none", "capture", "identity", "retirement", "prerequisite"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                root = Path(directory)
                supervisor = Mock(cancelled=False, children=[])
                supervisor.retire.return_value = failure != "retirement"
                build = {"source": SOURCE, "rustc": "rustc synthetic", "cargo": "cargo synthetic"}
                after = {**build, "target": "changed"} if failure == "identity" else build
                replacements = {
                    "build_environment": Mock(return_value={}), "Supervisor": Mock(return_value=supervisor),
                    "prerequisites": Mock(return_value=(receipt(), {"sha256": "a" * 64})),
                    "release_settings": Mock(return_value={"strip": True}),
                    "materialize": Mock(return_value=(root / "daemon", root / "probe")),
                    "free_address": Mock(return_value="127.0.0.1:8765"), "capture": Mock(), "publish": Mock(),
                }
                if failure in ("capture", "prerequisite"):
                    replacements["capture" if failure == "capture" else "prerequisites"].side_effect = OSError("PrivateToken /private/path")
                for name, substitute in replacements.items():
                    stack.enter_context(patch.object(measure, name, substitute))
                stack.enter_context(patch.object(measure.signal, "signal"))
                stack.enter_context(patch.dict(os.environ, {"TMPDIR": directory}))
                stack.enter_context(patch.object(measure.collectors, "source_identity", return_value=SOURCE))
                stack.enter_context(patch.object(measure.collectors, "collect_build", side_effect=[copy.deepcopy(build), after]))
                self.assertEqual(measure.run(root, root / "report.json"), 0 if failure == "none" else 1)
                report = replacements["publish"].call_args.args[1]
                self.assertEqual(report["status"], "success" if failure == "none" else "failed")
                self.assertNotIn("PrivateToken", json.dumps(report))
                self.assertNotIn("/private/path", json.dumps(report))
                self.assertEqual(bool(list(root.glob("relay-measure-*"))), failure == "retirement")
                supervisor.retire.assert_called_once()
                if failure == "prerequisite":
                    replacements["materialize"].assert_not_called()

    def test_output_is_confined_exclusive_atomic_and_failure_leaves_no_partial_success(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "LOCAL_ARTIFACTS"
            artifacts.mkdir()
            output = artifacts / "report.json"
            self.assertEqual(measure.output_path(root, str(output)), output)
            measure.publish(output, {"status": "failed"})
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                measure.publish(output, {"status": "success"})
            self.assertEqual(json.loads(output.read_text())["status"], "failed")
            output.unlink()
            with patch.object(measure.os, "fsync", side_effect=OSError("synthetic")), self.assertRaises(OSError):
                measure.publish(output, {"status": "success"})
            self.assertEqual(list(artifacts.iterdir()), [])
            sentinel = artifacts / ".relay-report-collision"
            sentinel.write_text("keep", encoding="utf-8")
            with patch.object(measure.secrets, "token_hex", return_value="collision"), self.assertRaises(FileExistsError):
                measure.publish(output, {"status": "success"})
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "keep")
            (artifacts / "link").symlink_to(root, target_is_directory=True)
            for path in (root / "outside.json", artifacts / "link" / "report.json"):
                with self.assertRaises(measure.collectors.MeasurementError):
                    measure.output_path(root, str(path))


if __name__ == "__main__":
    unittest.main()
