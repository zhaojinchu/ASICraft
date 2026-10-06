"""A client for the observer mod's stream: a WebSocket that only receives.

    with Observer(port) as stream:
        tick, changes = stream.next()   # the next tick with changes (or a heartbeat)

Each message is `{"tick": T, "changes": [["x,y,z", "state"], ...]}`; the first is
a hello with the current tick. Enough of RFC 6455 for a server that sends text
frames and expects nothing back but a close.
"""

from __future__ import annotations

import base64
import json
import os
import socket
import struct
import time


class ObserverError(Exception):
    pass


class Observer:
    def __init__(self, port: int, timeout: float = 30.0):
        self.port = port
        self.timeout = timeout
        self.sock: socket.socket | None = None
        self.buffer = b""
        self.hello: dict | None = None

    def __enter__(self) -> "Observer":
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                self.sock = socket.create_connection(("127.0.0.1", self.port), timeout=self.timeout)
                break
            except OSError:
                if time.monotonic() > deadline:
                    raise ObserverError(f"no observer stream on port {self.port}")
                time.sleep(0.2)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(
            f"GET / HTTP/1.1\r\nHost: 127.0.0.1:{self.port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n".encode()
        )
        reply = b""
        while b"\r\n\r\n" not in reply:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise ObserverError("the observer closed the connection during the handshake")
            reply += chunk
        head, _, self.buffer = reply.partition(b"\r\n\r\n")
        if b" 101 " not in head.split(b"\r\n")[0]:
            raise ObserverError(f"handshake refused: {head[:80]!r}")
        self.hello = self.next_message()
        if self.hello.get("hello") != "redfab-observer":
            raise ObserverError(f"unexpected first message {self.hello!r}")
        return self

    def __exit__(self, *_) -> None:
        if self.sock is not None:
            try:
                self.sock.sendall(b"\x88\x80\x00\x00\x00\x00")  # close, masked, empty
            except OSError:
                pass
            self.sock.close()

    def _read(self, n: int) -> bytes:
        while len(self.buffer) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ObserverError("the observer stream ended")
            self.buffer += chunk
        out, self.buffer = self.buffer[:n], self.buffer[n:]
        return out

    def next_message(self) -> dict:
        """The next text frame, decoded."""
        while True:
            first, second = self._read(2)
            opcode, length = first & 0x0F, second & 0x7F
            if length == 126:
                (length,) = struct.unpack(">H", self._read(2))
            elif length == 127:
                (length,) = struct.unpack(">Q", self._read(8))
            payload = self._read(length)
            if opcode == 0x1:
                return json.loads(payload.decode())
            if opcode == 0x8:
                raise ObserverError("the observer closed the stream")
            # pings and anything else: ignore (the server never sends them)

    def next(self) -> tuple[int, list[tuple[tuple[int, int, int], str]]]:
        """The next tick message as (tick, [((x, y, z), state), ...])."""
        message = self.next_message()
        changes = [(tuple(int(v) for v in pos.split(",")), state) for pos, state in message["changes"]]
        return int(message["tick"]), changes

    def until(self, tick: int) -> dict[tuple[int, int, int], str]:
        """Collects changes up to and including `tick`, latest state per position."""
        latest: dict[tuple[int, int, int], str] = {}
        while True:
            t, changes = self.next()
            for pos, state in changes:
                latest[pos] = state
            if t >= tick:
                return latest
