"""Minimal client for Minecraft's RCON protocol.

Packet layout (little-endian): int32 length, int32 id, int32 type, body, two NUL bytes.

Two server behaviours shape this client (both observed on 26.3):

- The server splits replies longer than 4096 bytes across several packets and
  gives no end marker. So after a command we send a second packet of an unknown
  type. The server answers packets in order, so when that second answer arrives
  the command's reply is complete.
- The server expects each socket read to contain exactly one request packet, at
  most 1460 bytes. If two requests arrive in the same read it drops the
  connection. So the second packet is only sent after the first reply packet has
  arrived, which proves the server has already consumed the command.
"""

import socket
import struct

_AUTH = 3
_COMMAND = 2
_SENTINEL = 0x7F  # not a valid request type; the server replies "Unknown request 7f"
_MAX_COMMAND_BYTES = 1446  # 1460-byte server read buffer minus the 14 bytes of framing


class RconError(Exception):
    pass


class Rcon:
    def __init__(self, host: str, port: int, password: str, timeout: float = 30.0):
        self._sock = socket.create_connection((host, port), timeout=timeout)
        self._next_id = 1
        self._send(self._take_id(), _AUTH, password)
        reply_id, _, _ = self._recv()
        if reply_id == -1:
            raise RconError("RCON authentication failed")

    def command(self, text: str) -> str:
        """Run one server command and return its full reply text."""
        if len(text.encode("utf-8")) > _MAX_COMMAND_BYTES:
            raise RconError(f"command is longer than {_MAX_COMMAND_BYTES} bytes: {text[:60]!r}...")
        cmd_id, end_id = self._take_id(), self._take_id()
        self._send(cmd_id, _COMMAND, text)
        parts = []
        sentinel_sent = False
        while True:
            reply_id, _, body = self._recv()
            if reply_id == end_id:
                return "".join(parts)
            if reply_id != cmd_id:
                raise RconError(f"unexpected reply id {reply_id} for command {text!r}")
            parts.append(body)
            if not sentinel_sent:
                self._send(end_id, _SENTINEL, "")
                sentinel_sent = True

    def close(self) -> None:
        self._sock.close()

    def _take_id(self) -> int:
        self._next_id += 1
        return self._next_id

    def _send(self, packet_id: int, packet_type: int, body: str) -> None:
        payload = struct.pack("<ii", packet_id, packet_type) + body.encode("utf-8") + b"\x00\x00"
        self._sock.sendall(struct.pack("<i", len(payload)) + payload)

    def _recv(self) -> tuple[int, int, str]:
        (length,) = struct.unpack("<i", self._read_exact(4))
        payload = self._read_exact(length)
        packet_id, packet_type = struct.unpack("<ii", payload[:8])
        return packet_id, packet_type, payload[8:-2].decode("utf-8")

    def _read_exact(self, count: int) -> bytes:
        chunks = bytearray()
        while len(chunks) < count:
            chunk = self._sock.recv(count - len(chunks))
            if not chunk:
                raise RconError("RCON connection closed by server")
            chunks += chunk
        return bytes(chunks)
