"""Bounded, credential-free Bot API peer for the HOST resource workload only."""

from __future__ import annotations

import argparse
import ipaddress
import json
import re
import signal
import socket
import socketserver
import sys
import threading
import time


CHUNK_BYTES = 64 * 1024
HEADER_BYTES = 8192
JSON_BYTES = 8192
PART_HEADER_BYTES = 2048
DOCUMENT_BYTES = 50_000_000
MULTIPART_BYTES = DOCUMENT_BYTES + 16 * 1024
WORKERS = 4
REQUESTS = 256
READ_SECONDS = 10
UPLOAD_SECONDS = 120
LIFETIME_SECONDS = 900
METHODS = {"getWebhookInfo", "getUpdates", "sendMessage", "sendDocument", "sendChatAction"}


class MockError(Exception):
    """Static diagnostics only; never retain request data in an error."""


def require(condition):
    if not condition:
        raise MockError("invalid synthetic request")


class Stream:
    """Small lookahead buffer with an absolute deadline and cancellable reads."""

    def __init__(self, receive, stopped, *, seconds=READ_SECONDS, clock=time.monotonic):
        self.receive, self.stopped, self.clock = receive, stopped, clock
        self.deadline = clock() + seconds
        self.buffer = bytearray()

    def check(self):
        if self.stopped.is_set() or self.clock() >= self.deadline:
            raise MockError("synthetic request stopped")

    def more(self, size):
        while True:
            self.check()
            try:
                data = self.receive(min(size, CHUNK_BYTES))
            except socket.timeout:
                continue
            self.check()
            if not data:
                raise MockError("incomplete synthetic request")
            return data

    def read(self, size):
        self.check()
        require(0 < size <= CHUNK_BYTES)
        if not self.buffer:
            return self.more(size)
        data = bytes(self.buffer[:size])
        del self.buffer[:size]
        return data

    def exact(self, size):
        require(0 <= size <= CHUNK_BYTES)
        result = bytearray()
        while len(result) < size:
            result.extend(self.read(size - len(result)))
        return bytes(result)

    def until(self, marker, limit):
        while True:
            self.check()
            index = self.buffer.find(marker)
            if index >= 0:
                require(index <= limit)
                result = bytes(self.buffer[:index])
                del self.buffer[:index + len(marker)]
                return result
            remaining = limit + len(marker) - len(self.buffer)
            require(remaining > 0)
            self.buffer.extend(self.more(remaining))


class Body:
    """Content-Length confinement, including header-reader lookahead."""

    def __init__(self, stream, size):
        self.stream, self.remaining = stream, size

    def read(self, size):
        require(self.remaining > 0)
        data = self.stream.read(min(size, self.remaining))
        self.remaining -= len(data)
        return data


def headers(raw):
    result = {}
    for line in raw.split(b"\r\n"):
        name, separator, value = line.partition(b":")
        require(separator and re.fullmatch(rb"[A-Za-z0-9-]+", name))
        key = name.decode("ascii").lower()
        require(key not in result and all(32 <= byte < 127 for byte in value))
        result[key] = value.strip().decode("ascii")
    return result


def request(stream):
    head = stream.until(b"\r\n\r\n", HEADER_BYTES)
    first, separator, rest = head.partition(b"\r\n")
    require(separator)
    match = re.fullmatch(rb"POST /botSynthetic/([A-Za-z]+) HTTP/1\.[01]", first)
    require(match is not None)
    method = match[1].decode("ascii")
    require(method in METHODS)
    fields = headers(rest)
    require("host" in fields and bool(fields["host"]))
    require(not {"transfer-encoding", "content-encoding", "expect", "authorization"} & fields.keys())
    length = fields.get("content-length", "")
    require(re.fullmatch(r"[0-9]{1,8}", length) is not None)
    size = int(length)
    require(0 < size <= (MULTIPART_BYTES if method == "sendDocument" else JSON_BYTES))
    return method, fields.get("content-type", ""), Body(stream, size)


def integer(value, low, high):
    require(type(value) is int and low <= value <= high)
    return value


def chat(value):
    return integer(value, -(1 << 63), (1 << 63) - 1)


def document(content_type, body, stopped, *, clock=time.monotonic):
    # The probe's reqwest form has a known length and these three ordered parts.
    # Unsupported chunked/extra-part forms fail rather than entering a generic MIME stack.
    match = re.fullmatch(
        r'multipart/form-data; boundary=(?:"([A-Za-z0-9_-]{1,70})"|([A-Za-z0-9_-]{1,70}))',
        content_type,
    )
    require(match is not None)
    boundary = (match[1] or match[2]).encode("ascii")
    stream = Stream(body.read, stopped, seconds=UPLOAD_SECONDS, clock=clock)
    require(stream.exact(len(boundary) + 4) == b"--" + boundary + b"\r\n")
    values = {}
    for name in ("chat_id", "caption", "document"):
        part = headers(stream.until(b"\r\n\r\n", PART_HEADER_BYTES))
        disposition = part.get("content-disposition", "")
        expected = 'form-data; name="' + name + '"'
        if name != "document":
            require(disposition == expected and set(part) == {"content-disposition"})
            values[name] = stream.until(b"\r\n--" + boundary + b"\r\n", 4096)
        else:
            require(re.fullmatch(re.escape(expected) + r'; filename="(?:[^"\\\r\n]|\\.){1,255}"',
                                 disposition) is not None)
            require(set(part) <= {"content-disposition", "content-type"})
    try:
        chat_text = values["chat_id"].decode("ascii")
        require(re.fullmatch(r"-?[0-9]{1,19}", chat_text) is not None)
        chat_id = chat(int(chat_text))
        caption = values["caption"].decode("utf-8")
        require(len(caption) <= 1000 and len(caption.encode("utf-16-le")) // 2 <= 1024)
    except (ValueError, UnicodeError):
        raise MockError("invalid synthetic document metadata") from None
    tail = b"\r\n--" + boundary + b"--\r\n"
    declared_size = body.remaining + len(stream.buffer) - len(tail)
    require(0 <= declared_size <= DOCUMENT_BYTES)
    delimiter = re.compile(rb"\r\n--" + re.escape(boundary) + rb"(--\r\n|\r\n)")
    size = 0
    while True:
        stream.check()
        match = delimiter.search(stream.buffer)
        if match:
            size += match.start()
            require(match[1] == b"--\r\n" and size == declared_size)
            del stream.buffer[:match.end()]
            require(body.remaining == 0 and not stream.buffer)
            return chat_id, size
        # Retain only enough lookbehind for a delimiter split across reads.
        consumed = max(0, len(stream.buffer) - len(tail) + 1)
        size += consumed
        require(size <= DOCUMENT_BYTES)
        del stream.buffer[:consumed]
        stream.buffer.extend(stream.more(CHUNK_BYTES))


class State:
    def __init__(self, *, wall_clock=time.time):
        self.wall_clock = wall_clock
        self.lock = threading.Lock()
        self.poll = threading.Lock()
        self.messages = self.documents = self.document_bytes = 0

    def message(self, chat_id, size=None):
        with self.lock:
            self.messages += 1
            if size is not None:
                self.documents += 1
                self.document_bytes += size
            result = {"message_id": self.messages, "date": int(self.wall_clock()),
                      "chat": {"id": chat_id}}
        if size is not None:
            result["document"] = {"file_id": "synthetic", "file_unique_id": "synthetic",
                                  "file_size": size}
        return result


def dispatch(method, content_type, body, state, stopped, *, clock=time.monotonic):
    if method == "sendDocument":
        return state.message(*document(content_type, body, stopped, clock=clock))
    require(content_type == "application/json")
    data = bytearray()
    while body.remaining:
        data.extend(body.read(min(CHUNK_BYTES, body.remaining)))
    try:
        payload = json.loads(data)
    except (ValueError, UnicodeError, RecursionError):
        raise MockError("invalid synthetic JSON") from None
    require(type(payload) is dict)
    if method == "getWebhookInfo":
        require(not payload)
        return {"url": "", "pending_update_count": 0}
    if method == "getUpdates":
        seconds = integer(payload.get("timeout", 45), 1, 50)
        integer(payload.get("limit", 100), 1, 100)
        integer(payload.get("offset", 0), 0, (1 << 63) - 1)
        require(payload.get("allowed_updates", ["message"]) == ["message"])
        require(state.poll.acquire(blocking=False))
        try:
            if stopped.wait(seconds):
                raise MockError("synthetic long poll stopped")
            return []
        finally:
            state.poll.release()
    chat_id = chat(payload.get("chat_id"))
    if method == "sendChatAction":
        require(payload.get("action") == "typing")
        return True
    text = payload.get("text")
    require(type(text) is str and 0 < len(text) <= 1000)
    return state.message(chat_id)


def respond(connection, status, payload):
    data = json.dumps(payload, separators=(",", ":")).encode("ascii")
    head = (f"HTTP/1.1 {status}\r\nContent-Type: application/json\r\n"
            f"Content-Length: {len(data)}\r\nConnection: close\r\n\r\n").encode("ascii")
    connection.settimeout(5)
    connection.sendall(head + data)


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(0.5)
        try:
            stream = Stream(self.request.recv, self.server.stopped)
            method, content_type, body = request(stream)
            if method == "sendDocument":
                stream.deadline = stream.clock() + UPLOAD_SECONDS
            result = dispatch(method, content_type, body, self.server.state, self.server.stopped)
            respond(self.request, "200 OK", {"ok": True, "result": result})
        except (MockError, OSError):
            if not self.server.stopped.is_set():
                self.server.failed = True
                try:
                    respond(self.request, "400 Bad Request", {"ok": False, "error_code": 400})
                except OSError:
                    pass


class Server(socketserver.ThreadingMixIn, socketserver.TCPServer):
    request_queue_size = WORKERS
    daemon_threads = False
    block_on_close = True

    def __init__(self, address, stopped):
        address_ip = loopback(address[0])
        integer(address[1], 0, 65535)
        self.address_family = socket.AF_INET6 if address_ip.version == 6 else socket.AF_INET
        self.stopped, self.state = stopped, State()
        self.slots = threading.BoundedSemaphore(WORKERS)
        self.requests = 0
        self.failed = False
        super().__init__(address, Handler)
        self.timeout = 0.5

    def process_request(self, request, client_address):
        if self.requests >= REQUESTS or not self.slots.acquire(blocking=False):
            self.failed = True
            self.stopped.set()
            self.shutdown_request(request)
            return
        self.requests += 1
        try:
            super().process_request(request, client_address)
        except Exception:
            self.slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.slots.release()

    def handle_error(self, request, client_address):
        self.failed = True
        self.stopped.set()
        print("Synthetic mock worker failed.", file=sys.stderr)


def loopback(value):
    try:
        address = ipaddress.ip_address(value)
        require(address.is_loopback and "%" not in value)
        return address
    except ValueError:
        raise MockError("numeric loopback address required") from None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--listen", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    options = parser.parse_args()
    stopped = threading.Event()
    for kind in (signal.SIGINT, signal.SIGTERM):
        signal.signal(kind, lambda *_: stopped.set())
    try:
        with Server((options.listen, options.port), stopped) as server:
            try:
                host, port = server.server_address[:2]
                address = f"[{host}]:{port}" if ":" in host else f"{host}:{port}"
                print(json.dumps({"event": "ready", "mock_address": address}), flush=True)
                deadline = time.monotonic() + LIFETIME_SECONDS
                while not stopped.is_set() and time.monotonic() < deadline:
                    server.handle_request()
                if not stopped.is_set():
                    server.failed = True
            finally:
                stopped.set()
        print(json.dumps({"event": "stopped", "requests": server.requests,
                          "documents": server.state.documents,
                          "document_bytes": server.state.document_bytes,
                          "failed": server.failed}), flush=True)
        return int(server.failed)
    except (MockError, OSError, ValueError, RuntimeError):
        print("Synthetic mock failed.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
