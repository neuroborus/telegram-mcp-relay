"""Synthetic streams/events/server substitutes; no sockets or runtime measurements."""

from contextlib import redirect_stderr, redirect_stdout
import io
import json
import socket
import threading
import unittest
from unittest.mock import Mock, patch

from tools import measure_mock as mock


class Segments:
    """Generate large zero runs on demand, never a whole near-limit document."""

    def __init__(self, *parts):
        self.parts = list(parts)
        self.largest_read = self.read_bytes = 0

    def read(self, size):
        self.largest_read = max(self.largest_read, size)
        if not self.parts:
            return b""
        part = self.parts[0]
        count = min(size, part if type(part) is int else len(part))
        result = b"\0" * count if type(part) is int else part[:count]
        rest = part - count if type(part) is int else part[count:]
        if rest:
            self.parts[0] = rest
        else:
            self.parts.pop(0)
        self.read_bytes += count
        return result


def wire(body, method="getWebhookInfo", *, content_type="application/json", extra=b""):
    head = (f"POST /botSynthetic/{method} HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n"
            f"Content-Type: {content_type}\r\nContent-Length: {len(body)}\r\n").encode()
    return head + extra + b"\r\n" + body


def form_parts():
    prefix = (b'--synthetic\r\nContent-Disposition: form-data; name="chat_id"\r\n\r\n-123\r\n'
              b'--synthetic\r\nContent-Disposition: form-data; name="caption"\r\n\r\nMeasure\r\n'
              b'--synthetic\r\nContent-Disposition: form-data; name="caption_entities"\r\n\r\n'
              b'[{"type":"bold","offset":0,"length":7}]\r\n'
              b'--synthetic\r\nContent-Disposition: form-data; name="document"; filename="data.bin"\r\n'
              b'Content-Type: application/octet-stream\r\n\r\n')
    return prefix, b"\r\n--synthetic--\r\n"


def handle(source, *, state=None, stopped=None):
    stopped = stopped or threading.Event()
    state = state or mock.State()
    method, content_type, body = mock.request(mock.Stream(source.read, stopped, clock=lambda: 0))
    return mock.dispatch(method, content_type, body, state, stopped, clock=lambda: 0)


class Stop:
    def __init__(self, cancelled=False):
        self.cancelled, self.waits = cancelled, []

    def is_set(self):
        return False

    def wait(self, seconds):
        self.waits.append(seconds)
        return self.cancelled


class MockTests(unittest.TestCase):
    def test_native_header_entities_use_utf16_and_leave_literal_markup_untouched(self):
        text = "🚀T\n📁 *dir*\n<b> & *literal*"
        spans = [{"type": "bold", "offset": 0, "length": 3},
                 {"type": "italic", "offset": 4, "length": 8}]
        payload = {"chat_id": 123, "text": text, "entities": spans}
        handle(io.BytesIO(wire(json.dumps(payload).encode(), "sendMessage")))
        prefix, tail = form_parts()
        metadata = json.dumps(spans).encode()
        form = prefix.replace(b"Measure", text.encode()).replace(
            b'[{"type":"bold","offset":0,"length":7}]', metadata)
        result = handle(io.BytesIO(wire(form + b"data" + tail, "sendDocument",
                                       content_type="multipart/form-data; boundary=synthetic")))
        self.assertEqual(result["document"]["file_size"], 4)
        for bad in [None, {}, [{"type": "bold", "offset": 1, "length": 1}],
                    [{"type": "bold", "offset": 0, "length": 100}],
                    [{"type": "html", "offset": 0, "length": 3}], spans * 2]:
            with self.subTest(spans=bad), self.assertRaises(mock.MockError):
                handle(io.BytesIO(wire(json.dumps({**payload, "entities": bad}).encode(),
                                       "sendMessage")))
        form = prefix.replace(b'[{"type":"bold","offset":0,"length":7}]', b"x" * 257)
        with self.assertRaises(mock.MockError):
            handle(io.BytesIO(wire(form + tail, "sendDocument",
                                   content_type="multipart/form-data; boundary=synthetic")))

    def test_split_http_and_multipart_framing_preserves_binary_byte_count(self):
        prefix, tail = form_parts()
        payload = b"\0\xff\r\n--synthe\0\r\n--synthetic-not-a-delimiter"
        request = wire(prefix + payload + tail, "sendDocument",
                       content_type='multipart/form-data; boundary="synthetic"')
        for index in range(1, len(request)):
            with self.subTest(split=index):
                result = handle(Segments(request[:index], request[index:]))
                self.assertEqual(result["document"]["file_size"], len(payload))
                self.assertEqual(result["chat"]["id"], -123)

    def test_near_limit_document_is_generated_and_drained_in_bounded_reads(self):
        prefix, tail = form_parts()
        size = 49_999_999
        source = Segments(prefix, size, tail)
        stopped = threading.Event()
        body = mock.Body(mock.Stream(source.read, stopped, clock=lambda: 0),
                         len(prefix) + size + len(tail))
        state = mock.State()
        result = mock.dispatch("sendDocument", "multipart/form-data; boundary=synthetic",
                               body, state, stopped, clock=lambda: 0)
        self.assertEqual(result["document"]["file_size"], size)
        self.assertEqual((state.documents, state.document_bytes), (1, size))
        self.assertEqual(source.read_bytes, len(prefix) + size + len(tail))
        self.assertLessEqual(source.largest_read, 64 * 1024)
        self.assertFalse(source.parts)

    def test_invalid_or_oversized_framing_fails_without_echoing_input(self):
        good = wire(b"{}")
        cases = [good.replace(b"POST ", b"GET "),
                 good.replace(b"/botSynthetic/", b"/botPrivateToken/"),
                 good.replace(b"getWebhookInfo", b"deleteWebhook"),
                 good.replace(b"Content-Length: 2", b"Content-Length: -2"),
                 good.replace(b"Content-Length: 2", b"Content-Length: 8193"),
                 good.replace(b"Content-Length: 2\r\n", b""),
                 wire(b"{}", extra=b"Content-Length: 2\r\n"),
                 wire(b"{}", extra=b"Transfer-Encoding: chunked\r\n"),
                 wire(b"{}", extra=b"Authorization: PrivateToken\r\n"),
                 wire(b"{}", extra=b"X-Long: " + b"x" * mock.HEADER_BYTES + b"\r\n"),
                 good[:-1], good.replace(b"Host:", b"Host\x00:")]
        for data in cases:
            with self.subTest(data=data[:30]), self.assertRaises(mock.MockError) as error:
                handle(io.BytesIO(data))
            self.assertNotIn("PrivateToken", str(error.exception))

    def test_multipart_rejects_wrong_order_headers_tail_metadata_and_size(self):
        prefix, tail = form_parts()
        valid = prefix + b"synthetic" + tail
        cases = [(valid, "multipart/form-data; boundary=wrong"),
                 (valid[:-1], "multipart/form-data; boundary=synthetic"),
                 (prefix + b"data\r\n--synthetic\r\nextra part" + tail,
                  "multipart/form-data; boundary=synthetic"),
                 (prefix + tail + b"trailing data", "multipart/form-data; boundary=synthetic"),
                 (valid.replace(b'name="caption"', b'name="document"', 1),
                  "multipart/form-data; boundary=synthetic"),
                 (valid.replace(b"-123", b"true", 1), "multipart/form-data; boundary=synthetic"),
                 (valid.replace(b"Measure", b"x" * 1001), "multipart/form-data; boundary=synthetic"),
                 (valid.replace(b"Measure", b"\xff"), "multipart/form-data; boundary=synthetic"),
                 (valid.replace(b"Measure", ("\U0001f680" * 513).encode()),
                  "multipart/form-data; boundary=synthetic"),
                 (valid.replace(b"Content-Type: application/octet-stream",
                                b"X-Long: " + b"x" * mock.PART_HEADER_BYTES),
                  "multipart/form-data; boundary=synthetic")]
        for data, content_type in cases:
            with self.subTest(content_type=content_type), self.assertRaises(mock.MockError):
                handle(io.BytesIO(wire(data, "sendDocument", content_type=content_type)))
        source = Segments(prefix, mock.DOCUMENT_BYTES + 1, tail)
        stopped = threading.Event()
        body = mock.Body(mock.Stream(source.read, stopped, clock=lambda: 0),
                         len(prefix) + mock.DOCUMENT_BYTES + 1 + len(tail))
        with self.assertRaises(mock.MockError):
            mock.document("multipart/form-data; boundary=synthetic", body, stopped, clock=lambda: 0)
        self.assertLess(source.read_bytes, 16 * 1024)

    def test_upload_cancellation_never_produces_a_document_receipt(self):
        prefix, tail = form_parts()
        stopped, state = threading.Event(), mock.State()
        source = Segments(prefix, 49_999_999, tail)

        def receive(size):
            data = source.read(size)
            if source.read_bytes > 64 * 1024:
                stopped.set()
            return data

        body = mock.Body(mock.Stream(receive, stopped, clock=lambda: 0),
                         len(prefix) + 49_999_999 + len(tail))
        with self.assertRaises(mock.MockError):
            mock.dispatch("sendDocument", "multipart/form-data; boundary=synthetic",
                          body, state, stopped, clock=lambda: 0)
        self.assertEqual((state.documents, state.document_bytes), (0, 0))
        self.assertLess(source.read_bytes, 128 * 1024)

    def test_webhook_text_and_action_use_bot_api_envelopes_and_unique_routes(self):
        self.assertEqual(handle(io.BytesIO(wire(b"{}"))), {"url": "", "pending_update_count": 0})
        state = mock.State()
        messages = [handle(io.BytesIO(wire(b'{"chat_id":123,"text":"Measure\\nsynthetic","entities":[{"type":"bold","offset":0,"length":7}]}',
                                           "sendMessage")), state=state) for _ in range(2)]
        self.assertEqual([item["message_id"] for item in messages], [1, 2])
        self.assertEqual(messages[0]["chat"]["id"], 123)
        self.assertTrue(handle(io.BytesIO(wire(b'{"chat_id":123,"action":"typing"}',
                                              "sendChatAction"))))
        for payload in (b"[]", b"{", b'{"chat_id":true,"text":"x"}',
                        b'{"chat_id":1,"text":5}', b'{"chat_id":1,"text":""}'):
            with self.assertRaises(mock.MockError):
                handle(io.BytesIO(wire(payload, "sendMessage")))

    def test_text_and_document_dates_use_current_unix_seconds_not_monotonic_time(self):
        now = [1_800_000_000.75]
        state = mock.State(wall_clock=lambda: now[0])
        text = handle(io.BytesIO(wire(b'{"chat_id":123,"text":"Measure synthetic","entities":[{"type":"bold","offset":0,"length":7}]}',
                                     "sendMessage")), state=state)
        now[0] = 1_800_000_061.25
        prefix, tail = form_parts()
        document = handle(io.BytesIO(wire(prefix + b"synthetic" + tail, "sendDocument",
                                          content_type="multipart/form-data; boundary=synthetic")),
                          state=state)
        # handle() supplies a fixed zero monotonic clock for request deadlines.
        self.assertEqual([text["date"], document["date"]], [1_800_000_000, 1_800_000_061])
        self.assertIs(type(text["date"]), int)
        self.assertIs(type(document["date"]), int)

    def test_handler_emits_a_length_delimited_envelope_and_static_failure(self):
        for body, failed in ((b"{}", False), (b"private synthetic content", True)):
            connection = Mock()
            connection.recv.side_effect = io.BytesIO(wire(body)).read
            server = Mock(stopped=threading.Event(), state=mock.State(), failed=False)
            mock.Handler(connection, ("127.0.0.1", 1234), server)
            head, payload = connection.sendall.call_args.args[0].split(b"\r\n\r\n", 1)
            self.assertIn(f"Content-Length: {len(payload)}".encode(), head)
            self.assertIn(b"Connection: close", head)
            result = json.loads(payload)
            self.assertEqual(result["ok"], not failed)
            self.assertEqual(server.failed, failed)
            self.assertNotIn(b"private synthetic content", payload)

    def test_long_poll_waits_45_seconds_and_releases_its_slot_on_cancellation(self):
        for cancelled in (False, True):
            stopped, state = Stop(cancelled), mock.State()
            if cancelled:
                with self.assertRaises(mock.MockError):
                    handle(io.BytesIO(wire(b"{}", "getUpdates")), state=state, stopped=stopped)
            else:
                self.assertEqual(handle(io.BytesIO(wire(b"{}", "getUpdates")),
                                        state=state, stopped=stopped), [])
            self.assertEqual(stopped.waits, [45])
            self.assertTrue(state.poll.acquire(blocking=False))
            state.poll.release()
        state.poll.acquire()
        with self.assertRaises(mock.MockError):
            handle(io.BytesIO(wire(b"{}", "getUpdates")), state=state, stopped=Stop())
        state.poll.release()
        for timeout in (0, 51, True):
            with self.assertRaises(mock.MockError):
                handle(io.BytesIO(wire(json.dumps({"timeout": timeout}).encode(), "getUpdates")),
                       stopped=Stop())

    def test_deadlines_and_stop_are_checked_even_with_buffered_input(self):
        now, reads = [0], []

        def receive(size):
            reads.append(size)
            now[0] += 2
            raise socket.timeout()

        stopped = threading.Event()
        stream = mock.Stream(receive, stopped, seconds=5, clock=lambda: now[0])
        with self.assertRaises(mock.MockError):
            stream.read(1)
        self.assertEqual(len(reads), 3)
        stream = mock.Stream(io.BytesIO(b"a\r\nb").read, stopped)
        self.assertEqual(stream.until(b"\r\n", 1), b"a")
        stopped.set()
        with self.assertRaises(mock.MockError):
            stream.read(1)

    def server(self):
        with patch.object(mock.socketserver.TCPServer, "__init__", return_value=None):
            server = mock.Server(("127.0.0.1", 0), threading.Event())
        server.shutdown_request = Mock()
        return server

    def test_worker_and_request_caps_close_excess_connections_without_spawning(self):
        for count, limit in ((mock.WORKERS, mock.REQUESTS), (2, 2)):
            server = self.server()
            with patch.object(mock, "REQUESTS", limit), patch.object(
                    mock.socketserver.ThreadingMixIn, "process_request") as start:
                for _ in range(count + 1):
                    server.process_request(object(), ("127.0.0.1", 1234))
            self.assertEqual(start.call_count, count)
            self.assertEqual(server.shutdown_request.call_count, 1)
            self.assertTrue(server.failed and server.stopped.is_set())
        server = self.server()
        server.slots.acquire()
        with patch.object(mock.socketserver.ThreadingMixIn, "process_request_thread",
                          side_effect=RuntimeError("synthetic")), self.assertRaises(RuntimeError):
            server.process_request_thread(object(), ("127.0.0.1", 1234))
        for _ in range(mock.WORKERS):
            self.assertTrue(server.slots.acquire(blocking=False))
        self.assertFalse(server.slots.acquire(blocking=False))

    def test_binding_rejects_dns_remote_unspecified_and_scoped_addresses(self):
        for address in ("localhost", "192.0.2.1", "0.0.0.0", "::", "::1%lo"):
            with self.assertRaises(mock.MockError):
                mock.loopback(address)
        for address in ("127.0.0.1", "::1"):
            self.assertTrue(mock.loopback(address).is_loopback)

    def test_main_stops_workers_before_close_on_normal_exit_and_failure(self):
        for failure in (False, True):
            stopped, order = threading.Event(), []
            server = Mock(server_address=("127.0.0.1", 1234), failed=False, requests=0,
                          state=mock.State())

            def tick():
                if failure:
                    raise OSError("private synthetic path")
                stopped.set()

            def close(*_):
                self.assertTrue(stopped.is_set())
                order.append("closed")

            server.handle_request.side_effect = tick
            server.__enter__ = Mock(return_value=server)
            server.__exit__ = Mock(side_effect=close)
            output, errors = io.StringIO(), io.StringIO()
            with patch.object(mock, "Server", return_value=server), patch.object(
                    mock.threading, "Event", return_value=stopped), patch.object(
                    mock.signal, "signal"), patch("sys.argv", ["measure_mock.py"]), \
                    redirect_stdout(output), redirect_stderr(errors):
                self.assertEqual(mock.main(), int(failure))
            self.assertEqual(order, ["closed"])
            self.assertNotIn("private synthetic path", errors.getvalue())
            records = [json.loads(line) for line in output.getvalue().splitlines()]
            self.assertEqual(records[0], {"event": "ready", "mock_address": "127.0.0.1:1234"})
            if not failure:
                self.assertEqual(records[1]["document_bytes"], 0)


if __name__ == "__main__":
    unittest.main()
