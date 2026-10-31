"""Single-use synthetic MCP workload peer; runtime capture belongs to HOST."""

from contextlib import contextmanager
import argparse
import http.client
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import select
import signal
import socket
import stat
import sys
import threading
import time


VERSION = "2026-07-28"
TITLE = "Resource measurement"
EXTERNAL_ID = "resource-measurement-v1"
CAPTION = "Synthetic near-limit upload."
CHAT_ID = -101
UPLOAD_BYTES = 49_999_999
FILENAME = "measurement-upload.bin"
REQUEST_BYTES = 8192
RESPONSE_BYTES = 65_536
HEADER_BYTES = 8192
WIRE_BYTES = 131_072
CONTROL_BYTES = 256
LIFETIME_SECONDS = 900
CONTROL_SECONDS = 120
UPLOAD_SECONDS = 620  # Includes the unchanged production 600-second attempt and bookkeeping.


class DriverError(Exception):
    """Only static diagnostics; never retain a path, key or server response."""


def require(condition):
    if not condition:
        raise DriverError("invalid synthetic driver input or result")


def parse_json(data):
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result)
            result[key] = value
        return result

    def constant(_):
        raise DriverError("invalid synthetic JSON")

    try:
        return json.loads(data, object_pairs_hook=pairs, parse_constant=constant)
    except (ValueError, UnicodeError, RecursionError):
        raise DriverError("invalid synthetic JSON") from None


def endpoint(value):
    match = re.fullmatch(r"(\[[0-9a-fA-F:]+\]|[0-9.]+):([0-9]{1,5})", value)
    require(match is not None)
    try:
        address, port = ipaddress.ip_address(match[1].strip("[]")), int(match[2])
        require(address.is_loopback and 0 < port <= 65535)
        authority = f"[{address}]:{port}" if address.version == 6 else f"{address}:{port}"
        require(value == authority)
        return str(address), port, authority
    except ValueError:
        raise DriverError("numeric loopback MCP address required") from None


class Budget:
    def __init__(self, stopped, *, clock=time.monotonic):
        self.stopped, self.clock = stopped, clock
        self.deadline = clock() + LIFETIME_SECONDS

    def remaining(self, deadline):
        remaining = min(deadline, self.deadline) - self.clock()
        if self.stopped.is_set() or not math.isfinite(remaining) or remaining <= 0:
            raise DriverError("synthetic driver cancelled or deadline exceeded")
        return remaining

    def until(self, seconds):
        return min(self.deadline, self.clock() + seconds)

    def receive(self, read, settimeout, deadline, *arguments):
        while True:
            settimeout(min(0.25, self.remaining(deadline)))
            try:
                count = read(*arguments)
            except socket.timeout:
                continue
            self.remaining(deadline)
            return count


class PeerSocket(socket.socket):
    """SocketIO uses this recv_into for every raw read, including slow headers."""

    def __init__(self, family, budget, deadline):
        super().__init__(family, socket.SOCK_STREAM)
        self.budget, self.deadline = budget, deadline

    def recv_into(self, buffer, nbytes=0, flags=0):
        return self.budget.receive(super().recv_into, self.settimeout, self.deadline, buffer, nbytes, flags)


class WireReader:
    """Bound total HTTP framing as well as the response body and header block."""

    def __init__(self, source):
        self.source = source
        self.remaining, self.header_remaining = WIRE_BYTES, HEADER_BYTES

    def take(self, method, size):
        limit = self.remaining
        if self.header_remaining is not None:
            limit = min(limit, self.header_remaining)
        require(size >= 0)
        data = method(min(size, limit + 1))
        require(len(data) <= limit)
        self.remaining -= len(data)
        if self.header_remaining is not None:
            self.header_remaining -= len(data)
        return data

    def readline(self, size):
        return self.take(self.source.readline, size)

    def read(self, size):
        return self.take(self.source.read, size)

    def flush(self):
        self.source.flush()

    def close(self):
        self.source.close()


class Response(http.client.HTTPResponse):
    def __init__(self, peer):
        super().__init__(peer)
        self.fp = WireReader(self.fp)

    def begin(self):
        super().begin()
        self.fp.header_remaining = None


def post(address, headers, data, seconds, budget, *, peer_factory=PeerSocket):
    """One direct numeric-loopback attempt: no proxy, redirect, DNS or retry."""
    host, port, authority = endpoint(address)
    require(len(data) <= REQUEST_BYTES)
    deadline = budget.until(seconds)
    family = socket.AF_INET6 if ":" in host else socket.AF_INET
    with peer_factory(family, budget, deadline) as peer:
        peer.settimeout(min(5, budget.remaining(deadline)))
        peer.connect((host, port))
        wire = (f"POST /mcp HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n"
                f"Content-Length: {len(data)}\r\n" +
                "".join(f"{name}: {value}\r\n" for name, value in headers.items()) + "\r\n").encode("ascii") + data
        # The entire request is small; sendall's timeout covers its complete write.
        peer.settimeout(min(5, budget.remaining(deadline)))
        peer.sendall(wire)
        with Response(peer) as response:
            response.begin()
            fields = {}
            for name, value in response.getheaders():
                name = name.lower()
                require(name not in fields)
                fields[name] = value
            require(response.status == 200 and "mcp-session-id" not in fields)
            require(fields.get("content-type", "").lower() in
                    {"application/json", "application/json; charset=utf-8"})
            require("content-encoding" not in fields)
            length = fields.get("content-length")
            if length is not None:
                require(re.fullmatch(r"[0-9]{1,6}", length) is not None)
                require(0 < int(length) <= RESPONSE_BYTES and "transfer-encoding" not in fields)
            else:
                require(fields.get("transfer-encoding", "").lower() == "chunked")
            body = response.read(RESPONSE_BYTES + 1)
            require(0 < len(body) <= RESPONSE_BYTES)
            require(length is None or len(body) == int(length))
            budget.remaining(deadline)
            return body


class Client:
    def __init__(self, address, budget, *, transport=post):
        endpoint(address)
        self.address, self.budget, self.transport = address, budget, transport
        self.request_id = 0

    def rpc(self, method, params, *, name=None, seconds=10):
        self.budget.remaining(self.budget.deadline)
        self.request_id += 1
        require(self.request_id <= 3)
        params = {**params, "_meta": {"io.modelcontextprotocol/protocolVersion": VERSION,
                                     "io.modelcontextprotocol/clientCapabilities": {}}}
        data = json.dumps({"jsonrpc": "2.0", "id": self.request_id,
                           "method": method, "params": params}, separators=(",", ":")).encode("ascii")
        require(len(data) <= REQUEST_BYTES)
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                   "MCP-Protocol-Version": VERSION, "Mcp-Method": method}
        if name is not None:
            headers["Mcp-Name"] = name
        raw = self.transport(self.address, headers, data, seconds, self.budget)
        require(len(raw) <= RESPONSE_BYTES)
        body = parse_json(raw)
        require(type(body) is dict and set(body) == {"jsonrpc", "id", "result"})
        require(body["jsonrpc"] == "2.0" and type(body["id"]) is int and body["id"] == self.request_id)
        require(type(body["result"]) is dict)
        return body["result"]

    def discover(self):
        result = self.rpc("server/discover", {})
        require(result.get("supportedVersions") == [VERSION])
        require(result.get("cacheScope") == "private" and type(result.get("ttlMs")) is int and result["ttlMs"] == 0)
        capabilities = result.get("capabilities")
        require(type(capabilities) is dict and capabilities.get("tools") == {})

    def tool(self, name, arguments, *, seconds=10):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments}, name=name, seconds=seconds)
        require(result.get("isError") is False and type(result.get("structuredContent")) is dict)
        content = result.get("content")
        require(type(content) is list and len(content) == 1 and type(content[0]) is dict)
        require(content[0].get("type") == "text" and type(content[0].get("text")) is str)
        require(parse_json(content[0]["text"]) == result["structuredContent"])
        return result["structuredContent"]

    def register(self):
        result = self.tool("telegram_register", {"external_session_id": EXTERNAL_ID, "title": TITLE})
        key = result.get("session_key")
        require(type(key) is str and re.fullmatch(r"[0-9a-f]{64}", key) is not None)
        require(result.get("title") == TITLE and
                result.get("inbox_uri") == f"telegram-bridge://sessions/{key}/inbox")
        return key

    def upload(self, key, path):
        result = self.tool("telegram_send_file", {"session_key": key, "path": str(path), "caption": CAPTION},
                           seconds=UPLOAD_SECONDS)
        require(type(result.get("telegram_chat_id")) is int and result["telegram_chat_id"] == CHAT_ID)
        message_id = result.get("outbound_message_id")
        require(type(message_id) is int and 0 < message_id < (1 << 63))
        return message_id


@contextmanager
def private_root(root):
    root = Path(root)
    require(root.is_absolute() and root != Path("/") and ".." not in root.parts)
    descriptor = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for part in root.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                            dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        metadata = os.fstat(descriptor)
        require(metadata.st_uid == os.getuid() and stat.S_IMODE(metadata.st_mode) == 0o700)
        yield descriptor
    finally:
        os.close(descriptor)


def stamp(metadata):
    return (metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns, metadata.st_ctime_ns)


class Upload:
    """Own one exclusive sparse zero file; the controller removes its root after retirement."""

    def __init__(self, root, budget):
        self.root, self.budget = Path(root), budget
        budget.remaining(budget.deadline)
        with private_root(root) as directory:
            self.directory_id = stamp(os.fstat(directory))[:2]
            descriptor = os.open(FILENAME, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                                 0o600, dir_fd=directory)
            try:
                budget.remaining(budget.deadline)
                os.ftruncate(descriptor, UPLOAD_BYTES)
                self.identity = stamp(os.fstat(descriptor))
            finally:
                os.close(descriptor)
        self.path = self.root / FILENAME
        self.verify()

    def verify(self):
        self.budget.remaining(self.budget.deadline)
        with private_root(self.root) as directory:
            metadata = os.stat(FILENAME, dir_fd=directory, follow_symlinks=False)
            require(stamp(os.fstat(directory))[:2] == self.directory_id)
            require(stat.S_ISREG(metadata.st_mode) and stamp(metadata) == self.identity)
            require(metadata.st_size == UPLOAD_BYTES and stat.S_IMODE(metadata.st_mode) == 0o600)


def readable(fd, timeout):
    return bool(select.select([fd], [], [], timeout)[0])


def writable(fd, timeout):
    return bool(select.select([], [fd], [], timeout)[1])


class Controls:
    def __init__(self, budget, *, read=os.read, wait=readable):
        self.budget, self.read, self.wait = budget, read, wait
        self.buffer = bytearray()

    def command(self):
        deadline = self.budget.until(CONTROL_SECONDS)
        while True:
            remaining = self.budget.remaining(deadline)
            if b"\n" in self.buffer:
                line, _, rest = self.buffer.partition(b"\n")
                self.buffer = bytearray(rest)
                require(len(line) <= CONTROL_BYTES)
                command = parse_json(line)
                require(type(command) is dict and set(command) == {"command"})
                require(command["command"] in ("upload", "stop"))
                return command["command"]
            require(len(self.buffer) <= CONTROL_BYTES)
            if self.wait(0, min(0.25, remaining)):
                try:
                    data = self.read(0, CONTROL_BYTES + 1 - len(self.buffer))
                except BlockingIOError:
                    continue
                require(bool(data))  # EOF is failure, never implicit approval.
                self.buffer.extend(data)


class Events:
    def __init__(self, budget, *, write=os.write, wait=writable):
        self.budget, self.write, self.wait = budget, write, wait
        self.bytes = 0

    def emit(self, event, **fields):
        data = (json.dumps({"event": event, "at_monotonic_seconds": self.budget.clock(), **fields},
                           separators=(",", ":"), allow_nan=False) + "\n").encode("ascii")
        self.bytes += len(data)
        require(len(data) <= 1024 and self.bytes <= 4096)
        deadline = self.budget.until(5)
        while data:
            remaining = self.budget.remaining(deadline)
            if self.wait(1, min(0.25, remaining)):
                try:
                    count = self.write(1, data)
                except BlockingIOError:
                    continue
                require(count > 0)
                data = data[count:]
        self.budget.remaining(deadline)


def drive(client, root, budget, controls, events, *, upload_factory=Upload):
    client.discover()
    events.emit("responsive", protocol_version=VERSION)
    key = client.register()
    upload = upload_factory(root, budget)
    events.emit("idle", upload_bytes=UPLOAD_BYTES)
    require(controls.command() == "upload")
    upload.verify()
    events.emit("upload_started", upload_bytes=UPLOAD_BYTES)
    message_id = client.upload(key, upload.path)
    upload.verify()
    events.emit("uploaded", upload_bytes=UPLOAD_BYTES,
                telegram_chat_id=CHAT_ID, outbound_message_id=message_id)
    # Remain identifiable and sampleable until the controller finishes its window.
    require(controls.command() == "stop")
    events.emit("stopped")


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise DriverError("invalid synthetic driver arguments")


def main():
    parser = Parser(description=__doc__)
    parser.add_argument("--mcp-address", required=True)
    parser.add_argument("--allowed-root", required=True)
    stopped = threading.Event()
    for kind in (signal.SIGINT, signal.SIGTERM):
        signal.signal(kind, lambda *_: stopped.set())
    try:
        options = parser.parse_args()
        os.set_blocking(0, False)
        os.set_blocking(1, False)
        budget = Budget(stopped)
        client = Client(options.mcp_address, budget)
        drive(client, options.allowed_root, budget, Controls(budget), Events(budget))
        return 0
    except (DriverError, OSError, ValueError, RuntimeError, http.client.HTTPException):
        print("Synthetic MCP driver failed; delivery may be uncertain.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
