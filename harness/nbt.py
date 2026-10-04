"""Reader for Minecraft's NBT binary format (big-endian, uncompressed input).

Returns plain Python values: compounds become dicts, lists become lists, numbers
and strings become themselves. The three array tags come back as `LazyArray`,
which decodes its numbers only when indexed or iterated; chunk files are full of
large arrays (light, heightmaps, biomes) that a caller rarely needs.
"""

from __future__ import annotations

import struct

TAG_END, TAG_BYTE, TAG_SHORT, TAG_INT, TAG_LONG, TAG_FLOAT, TAG_DOUBLE = range(7)
TAG_BYTE_ARRAY, TAG_STRING, TAG_LIST, TAG_COMPOUND, TAG_INT_ARRAY, TAG_LONG_ARRAY = range(7, 13)

_SCALARS = {
    TAG_BYTE: struct.Struct(">b"), TAG_SHORT: struct.Struct(">h"), TAG_INT: struct.Struct(">i"),
    TAG_LONG: struct.Struct(">q"), TAG_FLOAT: struct.Struct(">f"), TAG_DOUBLE: struct.Struct(">d"),
}
_ARRAYS = {TAG_BYTE_ARRAY: ("b", 1), TAG_INT_ARRAY: ("i", 4), TAG_LONG_ARRAY: ("q", 8)}
_INT = struct.Struct(">i")
_USHORT = struct.Struct(">H")


class LazyArray:
    """An NBT array whose elements are decoded on demand."""

    __slots__ = ("_data", "_offset", "_count", "_code", "_size", "_values")

    def __init__(self, data: bytes, offset: int, count: int, code: str, size: int):
        self._data, self._offset, self._count, self._code, self._size = data, offset, count, code, size
        self._values: list | None = None

    def _decode(self) -> list:
        if self._values is None:
            fmt = f">{self._count}{self._code}"
            self._values = list(struct.unpack_from(fmt, self._data, self._offset))
        return self._values

    def __len__(self) -> int:
        return self._count

    def __getitem__(self, index):
        if isinstance(index, int) and self._values is None:
            if index < 0:
                index += self._count
            if not 0 <= index < self._count:
                raise IndexError(index)
            return struct.unpack_from(">" + self._code, self._data, self._offset + index * self._size)[0]
        return self._decode()[index]

    def __iter__(self):
        return iter(self._decode())

    def __eq__(self, other) -> bool:
        return list(self) == list(other)

    def __repr__(self) -> str:
        return f"LazyArray({list(self)!r})"


def read_nbt(data: bytes):
    """Parse one named root compound and return its value."""
    if data[0] != TAG_COMPOUND:
        raise ValueError(f"root tag is {data[0]}, expected a compound")
    name_len = _USHORT.unpack_from(data, 1)[0]
    value, _ = _read_payload(data, 3 + name_len, TAG_COMPOUND)
    return value


def _read_payload(data: bytes, offset: int, tag_type: int):
    """Return (value, offset after the value)."""
    scalar = _SCALARS.get(tag_type)
    if scalar is not None:
        return scalar.unpack_from(data, offset)[0], offset + scalar.size
    if tag_type == TAG_STRING:
        length = _USHORT.unpack_from(data, offset)[0]
        start = offset + 2
        return data[start: start + length].decode("utf-8", errors="replace"), start + length
    array = _ARRAYS.get(tag_type)
    if array is not None:
        code, size = array
        count = _INT.unpack_from(data, offset)[0]
        start = offset + 4
        return LazyArray(data, start, count, code, size), start + count * size
    if tag_type == TAG_LIST:
        element_type = data[offset]
        count = _INT.unpack_from(data, offset + 1)[0]
        offset += 5
        items = []
        for _ in range(count):
            value, offset = _read_payload(data, offset, element_type)
            items.append(value)
        return items, offset
    if tag_type == TAG_COMPOUND:
        result = {}
        while True:
            element_type = data[offset]
            offset += 1
            if element_type == TAG_END:
                return result, offset
            length = _USHORT.unpack_from(data, offset)[0]
            name = data[offset + 2: offset + 2 + length].decode("utf-8", errors="replace")
            value, offset = _read_payload(data, offset + 2 + length, element_type)
            result[name] = value
    raise ValueError(f"unknown NBT tag type {tag_type}")
