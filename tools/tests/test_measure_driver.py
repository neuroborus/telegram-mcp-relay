"""Controlled MCP transports, streams, clocks and private files; no sockets or capture."""

from contextlib import redirect_stderr
import http.client
import io
import json
import os
from pathlib import Path
import socket
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

from tools import measure_driver as driver


KEY = "a" * 64
REGISTRATION = {"session_key": KEY, "title": driver.TITLE,
                "inbox_uri": f"telegram-bridge://sessions/{KEY}/inbox"}
SENT = {"telegram_chat_id": driver.CHAT_ID, "outbound_message_id": 7}
DISCOVERY = {"supportedVersions": [driver.VERSION], "cacheScope": "private", "ttlMs": 0,
             "capabilities": {"tools": {}, "resources": {"subscribe": True}}}


def tool(result):
    return {"isError": False, "structuredContent": result,
            "content": [{"type": "text", "text": json.dumps(result)}]}


class Transport:
    def __init__(self, results=None):
        self.results = list(results if results is not None else
                            [DISCOVERY, tool(REGISTRATION), tool(SENT)])
        self.calls = []

    def __call__(self, address, headers, data, seconds, budget):
        self.calls.append((address, headers, json.loads(data), seconds))
        result = self.results.pop(0)
        return json.dumps({"jsonrpc": "2.0", "id": self.calls[-1][2]["id"], "result": result}).encode()


def budget(clock=lambda: 0):
    return driver.Budget(threading.Event(), clock=clock)


def client(transport=None):
    return driver.Client("127.0.0.1:8765", budget(), transport=transport or Transport())


class Source(io.BytesIO):
    def __init__(self, data):
        super().__init__(data)
        self.sizes = []

    def read(self, size):
        self.sizes.append(size)
        return super().read(size)

    def readline(self, size):
        self.sizes.append(size)
        return super().readline(size)


class Peer:
    def __init__(self, data):
        self.source = Source(data)
        self.closed = False

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.closed = True

    def settimeout(self, timeout):
        pass

    def connect(self, address):
        self.address = address

    def sendall(self, data):
        self.sent = data

    def makefile(self, mode):
        return self.source


def http_response(body=b'{}', *, status=b"200 OK", fields=None):
    if fields is None:
        fields = b"Content-Type: application/json\r\nContent-Length: " + str(len(body)).encode() + b"\r\n"
    return b"HTTP/1.1 " + status + b"\r\n" + fields + b"\r\n" + body


class DriverTests(unittest.TestCase):
    def test_protocol_headers_metadata_and_committed_send_contract(self):
        transport = Transport()
        mcp = client(transport)
        mcp.discover()
        key = mcp.register()
        self.assertEqual(mcp.upload(key, Path("/synthetic/export/measurement-upload.bin")), 7)
        for index, (_, headers, body, seconds) in enumerate(transport.calls, 1):
            self.assertEqual(body["id"], index)
            self.assertEqual(headers["MCP-Protocol-Version"], "2026-07-28")
            self.assertEqual(headers["Mcp-Method"], body["method"])
            self.assertEqual(headers["Accept"], "application/json, text/event-stream")
            self.assertEqual(body["params"]["_meta"], {
                "io.modelcontextprotocol/protocolVersion": driver.VERSION,
                "io.modelcontextprotocol/clientCapabilities": {}})
            self.assertNotIn("Authorization", headers)
            self.assertEqual(seconds, driver.UPLOAD_SECONDS if index == 3 else 10)
        self.assertEqual(transport.calls[1][2]["params"]["arguments"],
                         {"external_session_id": driver.EXTERNAL_ID, "title": driver.TITLE})
        _, headers, body, _ = transport.calls[2]
        self.assertEqual(headers["Mcp-Name"], "telegram_send_file")
        self.assertEqual(body["params"]["arguments"], {"session_key": KEY,
                         "path": "/synthetic/export/measurement-upload.bin", "caption": driver.CAPTION})
        with self.assertRaises(driver.DriverError):
            mcp.upload(key, Path("/synthetic/export/measurement-upload.bin"))
        self.assertEqual(len(transport.calls), 3)  # No fourth attempt or uncertain-send retry.

    def test_protocol_and_discovery_rejection_never_quotes_server_input(self):
        cases = [b'{', b'[]', b'{"jsonrpc":"2.0","id":true,"result":{}}',
                 b'{"jsonrpc":"2.0","id":2,"result":{}}',
                 b'{"jsonrpc":"2.0","id":1,"id":1,"result":{}}',
                 b'{"jsonrpc":"2.0","id":1,"result":NaN}',
                 b'{"jsonrpc":"2.0","id":1,"error":{"message":"PrivateToken"}}',
                 b'x' * (driver.RESPONSE_BYTES + 1)]
        for raw in cases:
            with self.subTest(raw=raw[:15]), self.assertRaises(driver.DriverError) as error:
                client(lambda *_: raw).discover()
            self.assertNotIn("PrivateToken", str(error.exception))
        for change in ({"supportedVersions": ["2025-03-26"]}, {"cacheScope": "public"},
                       {"ttlMs": True}, {"capabilities": {}}):
            with self.subTest(change=change), self.assertRaises(driver.DriverError):
                client(Transport([{**DISCOVERY, **change}])).discover()

    def test_registration_and_tool_result_validation(self):
        invalid = [tool({**REGISTRATION, "title": "wrong"}),
                   tool({**REGISTRATION, "session_key": "PrivateKey"}),
                   tool({**REGISTRATION, "inbox_uri": "telegram-bridge://wrong/inbox"}),
                   {**tool(REGISTRATION), "isError": True},
                   {**tool(REGISTRATION), "isError": 0},
                   {**tool(REGISTRATION), "structuredContent": None},
                   {**tool(REGISTRATION), "content": []},
                   {**tool(REGISTRATION), "content": [{"type": "text", "text": "{}"}]}]
        for result in invalid:
            with self.subTest(result=result), self.assertRaises(driver.DriverError):
                client(Transport([result])).register()
        for change in ({"telegram_chat_id": 7}, {"telegram_chat_id": True},
                       {"outbound_message_id": 0}, {"outbound_message_id": True},
                       {"outbound_message_id": 1 << 63}):
            with self.subTest(change=change), self.assertRaises(driver.DriverError):
                client(Transport([tool({**SENT, **change})])).upload(KEY, Path("/synthetic/file"))

    def test_http_fixed_and_chunked_responses_close_and_use_direct_numeric_peer(self):
        for raw in (http_response(), http_response(b'1\r\n{\r\n1\r\n}\r\n0\r\n\r\n',
                    fields=b"Content-Type: application/json\r\nTransfer-Encoding: chunked\r\n")):
            peer = Peer(raw)
            result = driver.post("[::1]:8765", {"Content-Type": "application/json"}, b"{}", 10,
                                 budget(), peer_factory=lambda *_: peer)
            self.assertEqual(result, b"{}")
            self.assertEqual(peer.address, ("::1", 8765))
            self.assertIn(b"Host: [::1]:8765\r\n", peer.sent)
            self.assertTrue(peer.closed and peer.source.closed)
            self.assertLessEqual(max(peer.source.sizes), driver.RESPONSE_BYTES + 1)

    def test_http_framing_limits_and_failure_always_retire_connection(self):
        fields = b"Content-Type: application/json\r\n"
        cases = [http_response(status=b"302 Found"), http_response(status=b"500 PrivateToken"),
                 http_response(fields=fields), http_response(body=b"{", fields=fields + b"Content-Length: 2\r\n"),
                 http_response(fields=fields + b"Content-Length: 65537\r\n"),
                 http_response(fields=fields + b"Content-Length: 2\r\nContent-Length: 2\r\n"),
                 http_response(fields=fields + b"Content-Length: 2\r\nTransfer-Encoding: chunked\r\n"),
                 http_response(fields=fields + b"Content-Length: 2\r\nContent-Encoding: gzip\r\n"),
                 http_response(fields=fields + b"Content-Length: 2\r\nMcp-Session-Id: PrivateKey\r\n"),
                 http_response(fields=b"Content-Type: text/event-stream\r\nContent-Length: 2\r\n"),
                 http_response(fields=fields + b"X-Large: " + b"x" * driver.HEADER_BYTES + b"\r\n"),
                 http_response(body=b"invalid\r\n{}", fields=fields + b"Transfer-Encoding: chunked\r\n"),
                 http_response(body=(b"1;" + b"x" * 8000 + b"\r\nx\r\n") * 17 + b"0\r\n\r\n",
                               fields=fields + b"Transfer-Encoding: chunked\r\n"),
                 http_response(body=b"10001\r\n" + b"x" * (driver.RESPONSE_BYTES + 1) + b"\r\n0\r\n\r\n",
                               fields=fields + b"Transfer-Encoding: chunked\r\n")]
        for raw in cases:
            peer = Peer(raw)
            with self.subTest(size=len(raw)), self.assertRaises((driver.DriverError, http.client.HTTPException)):
                driver.post("127.0.0.1:8765", {}, b"{}", 10, budget(), peer_factory=lambda *_: peer)
            self.assertTrue(peer.closed and peer.source.closed)
        peer = Peer(http_response())
        peer.sendall = Mock(side_effect=OSError("PrivateToken"))
        with self.assertRaises(OSError):
            driver.post("127.0.0.1:8765", {}, b"{}", 10, budget(), peer_factory=lambda *_: peer)
        self.assertTrue(peer.closed)

    def test_addresses_and_request_size_rejected_before_connect(self):
        peer = Mock()
        for address in ("localhost:8765", "192.0.2.1:8765", "0.0.0.0:8765", "127.0.0.1:0",
                        "127.0.0.1:65536", "[::1%lo]:8765", "127.0.0.1:8765/private",
                        "user@127.0.0.1:8765", "127.0.0.1:08765"):
            with self.subTest(address=address), self.assertRaises(driver.DriverError):
                driver.post(address, {}, b"{}", 10, budget(), peer_factory=peer)
        with self.assertRaises(driver.DriverError):
            driver.post("127.0.0.1:8765", {}, b"x" * (driver.REQUEST_BYTES + 1), 10,
                        budget(), peer_factory=peer)
        peer.assert_not_called()

    def test_raw_reads_use_absolute_deadlines_and_cancellation_between_timeouts(self):
        now = [0]
        limits = budget(lambda: now[0])
        timeout = Mock()
        receive = Mock(side_effect=[socket.timeout(), 2])
        self.assertEqual(limits.receive(receive, timeout, 1, bytearray(8)), 2)
        self.assertEqual([call.args[0] for call in timeout.call_args_list], [0.25, 0.25])

        def expire(*_):
            now[0] = 1
            raise socket.timeout()

        with self.assertRaises(driver.DriverError):
            limits.receive(expire, timeout, 1, bytearray(8))
        now[0] = 0

        def cancel(*_):
            limits.stopped.set()
            return 2

        with self.assertRaises(driver.DriverError):
            limits.receive(cancel, timeout, 1, bytearray(8))
        receive.reset_mock()
        with self.assertRaises(driver.DriverError):
            limits.receive(receive, timeout, 1, bytearray(8))
        receive.assert_not_called()

    def test_bounded_control_streams_eof_commands_and_deadlines(self):
        source = io.BytesIO(b'{"command":"upload"}\n{"command":"stop"}\n')
        reads = []

        def read(_, size):
            reads.append(size)
            return source.read(min(size, 3))

        controls = driver.Controls(budget(), read=read, wait=lambda *_: True)
        self.assertEqual([controls.command(), controls.command()], ["upload", "stop"])
        self.assertLessEqual(max(reads), driver.CONTROL_BYTES + 1)
        for raw in (b"", b"{}\n", b'{"command":"retry"}\n', b'{"command":"upload", "extra":1}\n',
                    b'{"command":"stop","command":"upload"}\n', b"x" * 257, b"x" * 257 + b"\n"):
            source = io.BytesIO(raw)
            with self.subTest(raw=raw[:20]), self.assertRaises(driver.DriverError):
                driver.Controls(budget(), read=lambda _, size: source.read(size), wait=lambda *_: True).command()
        now = [0]
        limits = budget(lambda: now[0])

        def wait(_, seconds):
            now[0] += driver.CONTROL_SECONDS
            return False

        with self.assertRaises(driver.DriverError):
            driver.Controls(limits, wait=wait).command()
        limits.stopped.set()
        with self.assertRaises(driver.DriverError):
            limits.remaining(limits.deadline)

    def test_phase_coordination_waits_for_sampling_and_keeps_driver_alive_after_send(self):
        events, upload = Mock(), Mock(path=Path("/synthetic/export/measurement-upload.bin"))
        mcp = client()
        phases = []
        events.emit.side_effect = lambda event, **_: phases.append(event)

        def command():
            if phases[-1] == "idle":
                self.assertEqual(mcp.request_id, 2)
                return "upload"
            self.assertEqual(phases[-1], "uploaded")
            self.assertEqual(mcp.request_id, 3)
            return "stop"

        driver.drive(mcp, Path("/synthetic/export"), budget(), Mock(command=command), events,
                     upload_factory=lambda *_: upload)
        self.assertEqual(phases, ["responsive", "idle", "upload_started", "uploaded", "stopped"])
        self.assertEqual(upload.verify.call_count, 2)
        self.assertNotIn(KEY, str(events.mock_calls))
        self.assertNotIn(str(upload.path), str(events.mock_calls))

    def test_cancelled_failed_or_repeated_phases_never_retry_upload(self):
        for commands, fail_send, verifies in ((["stop"], False, None), (["upload"], True, None),
                                               (["upload", "upload"], False, None),
                                               (["upload"], False, [driver.DriverError("changed")]),
                                               (["upload"], False, [None, driver.DriverError("changed")])):
            transport, events = Transport(), Mock()
            if fail_send:
                transport.results[2] = {**tool(SENT), "isError": True}
            upload = Mock(path=Path("/synthetic/file"))
            upload.verify.side_effect = verifies
            with self.subTest(commands=commands, fail=fail_send), self.assertRaises(driver.DriverError):
                driver.drive(client(transport), Path("/synthetic"), budget(),
                             Mock(command=Mock(side_effect=commands)), events, upload_factory=lambda *_: upload)
            self.assertLessEqual(len(transport.calls), 3)
            self.assertNotIn("stopped", [call.args[0] for call in events.emit.call_args_list])

    def test_sparse_exact_workload_is_exclusive_and_mutation_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            upload = driver.Upload(root, budget())
            self.assertEqual(upload.path.stat().st_size, 49_999_999)
            self.assertEqual(upload.path.stat().st_mode & 0o777, 0o600)
            with upload.path.open("rb") as source:
                self.assertEqual(source.read(3), b"\0" * 3)
                source.seek(-3, os.SEEK_END)
                self.assertEqual(source.read(3), b"\0" * 3)
            with self.assertRaises(FileExistsError):
                driver.Upload(root, budget())
            with upload.path.open("wb") as target:
                target.truncate(12)
            with self.assertRaises(driver.DriverError):
                upload.verify()
            # It remains controller-owned until all peers retire, even after failure.
            self.assertEqual(upload.path.stat().st_size, 12)

    def test_private_roots_links_and_failed_creation_preserve_unowned_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside, linked = root / "outside", root / "linked"
            outside.mkdir(mode=0o700)
            linked.symlink_to(outside, target_is_directory=True)
            for path in (linked, linked / ".." / "outside", Path("relative")):
                with self.assertRaises((driver.DriverError, OSError)):
                    driver.Upload(path, budget())
            outside.chmod(0o755)
            with self.assertRaises(driver.DriverError):
                driver.Upload(outside, budget())
            self.assertFalse(list(outside.iterdir()))
            sentinel = outside / "keep"
            sentinel.write_text("synthetic sentinel", encoding="utf-8")
            (root / driver.FILENAME).symlink_to(sentinel)
            with self.assertRaises(OSError):
                driver.Upload(root, budget())
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "synthetic sentinel")
            (root / driver.FILENAME).unlink()
            descriptors = []

            def fail(descriptor, size):
                descriptors.append(descriptor)
                raise OSError("synthetic allocation failure")

            with patch.object(driver.os, "ftruncate", side_effect=fail), self.assertRaises(OSError):
                driver.Upload(root, budget())
            with self.assertRaises(OSError):
                os.fstat(descriptors[0])
            self.assertTrue((root / driver.FILENAME).exists())

    def test_output_is_bounded_partial_writes_and_stalls_fail(self):
        chunks = []

        def write(_, data):
            chunks.append(data[:3])
            return len(chunks[-1])

        driver.Events(budget(), write=write, wait=lambda *_: True).emit("idle", upload_bytes=49_999_999)
        self.assertEqual(json.loads(b"".join(chunks))["upload_bytes"], 49_999_999)
        with self.assertRaises(driver.DriverError):
            driver.Events(budget(), write=Mock(), wait=Mock()).emit("x" * 1024)
        now = [0]
        limits = budget(lambda: now[0])

        def stalled(_, seconds):
            now[0] += 5
            return False

        with self.assertRaises(driver.DriverError):
            driver.Events(limits, wait=stalled).emit("idle")

    def test_cli_failure_diagnostics_redact_paths_tokens_and_response_details(self):
        for error in (driver.DriverError("PrivateKey /private/path"), OSError("PrivateToken"),
                      http.client.BadStatusLine("PrivateResponse")):
            output = io.StringIO()
            with patch.object(driver.signal, "signal"), patch.object(driver.os, "set_blocking"), \
                    patch.object(driver.sys, "argv", ["measure_driver.py", "--mcp-address",
                         "127.0.0.1:8765", "--allowed-root", "/private/path"]), \
                    patch.object(driver, "drive", side_effect=error), redirect_stderr(output):
                self.assertEqual(driver.main(), 1)
            self.assertNotIn("Private", output.getvalue())
            self.assertNotIn("/private", output.getvalue())


if __name__ == "__main__":
    unittest.main()
