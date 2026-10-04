"""Block states as the game writes them in commands: `minecraft:name[prop=value,...]`.

Also rotation about the vertical axis, used to run the same fixture in several
orientations. Rotation only knows the properties of the blocks redfab emits and
raises on anything else, so a new block kind cannot be rotated wrongly by accident.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

_STATE_RE = re.compile(r"^(?P<name>[a-z0-9_.:/-]+)(?:\[(?P<props>[^\]]*)\])?$")

Pos = tuple[int, int, int]


@dataclass(frozen=True, order=True)
class BlockState:
    name: str
    props: tuple[tuple[str, str], ...] = ()

    @classmethod
    def parse(cls, text: str) -> BlockState:
        m = _STATE_RE.match(text.strip())
        if not m:
            raise ValueError(f"not a block state: {text!r}")
        name = m.group("name")
        if ":" not in name:
            name = "minecraft:" + name
        props: dict[str, str] = {}
        if m.group("props"):
            for item in m.group("props").split(","):
                key, _, value = item.partition("=")
                if not key or not value:
                    raise ValueError(f"bad property {item!r} in {text!r}")
                props[key.strip()] = value.strip()
        return cls(name, tuple(sorted(props.items())))

    @classmethod
    def from_palette(cls, entry) -> BlockState:
        """From a chunk palette entry. 26.3 writes a palette as a list of plain
        strings when no entry has properties; otherwise as compounds, where a
        block without properties is `{"": name}` and one with properties is
        `{"id": name, "properties": {...}}`."""
        if isinstance(entry, str):
            return cls(entry)
        if "" in entry:
            return cls(entry[""])
        return cls(entry["id"], tuple(sorted(entry.get("properties", {}).items())))

    def __str__(self) -> str:
        if not self.props:
            return self.name
        return f"{self.name}[{','.join(f'{k}={v}' for k, v in self.props)}]"

    def get(self, prop: str, default: str | None = None) -> str | None:
        return dict(self.props).get(prop, default)

    def with_props(self, **changes: str) -> BlockState:
        props = dict(self.props)
        props.update(changes)
        return BlockState(self.name, tuple(sorted(props.items())))

    def matches(self, pattern: BlockState) -> bool:
        """True if same block and every property the pattern names has that value."""
        if self.name != pattern.name:
            return False
        mine = dict(self.props)
        return all(mine.get(k) == v for k, v in pattern.props)

    @property
    def is_air(self) -> bool:
        return self.name in ("minecraft:air", "minecraft:cave_air", "minecraft:void_air")


AIR = BlockState("minecraft:air")

_HORIZONTAL = ["north", "east", "south", "west"]  # clockwise seen from above

# Properties whose values are not directions, so rotation leaves them alone.
_ROTATION_SAFE = {
    "power", "lit", "powered", "locked", "delay", "face", "mode", "waterlogged",
    "snowy", "type", "conditional", "triggered", "extended", "enabled", "inverted",
    "attached", "open", "has_book", "instrument", "note",
}
# Properties naming one of the four horizontal directions (or up/down, left as is).
_FACING_PROPS = {"facing"}
# Properties keyed by direction, as on redstone dust (north=side, ...).
_SIDE_PROPS = set(_HORIZONTAL)


@dataclass(frozen=True)
class Rotation:
    """Quarter turns clockwise about +Y, seen from above: north -> east -> south -> west."""

    quarter_turns: int

    def __post_init__(self):
        object.__setattr__(self, "quarter_turns", self.quarter_turns % 4)

    def inverse(self) -> Rotation:
        return Rotation(-self.quarter_turns)

    def apply_pos(self, pos: Pos) -> Pos:
        x, y, z = pos
        for _ in range(self.quarter_turns):
            # north (0,-1) must go to east (1,0): (dx, dz) -> (-dz, dx)
            x, z = -z, x
        return (x, y, z)

    def apply_dir(self, direction: str) -> str:
        if direction not in _HORIZONTAL:
            return direction  # up / down
        return _HORIZONTAL[(_HORIZONTAL.index(direction) + self.quarter_turns) % 4]

    def apply_state(self, state: BlockState) -> BlockState:
        if self.quarter_turns == 0:
            return state
        props = dict(state.props)
        out: dict[str, str] = {}
        for key, value in props.items():
            if key in _FACING_PROPS:
                out[key] = self.apply_dir(value)
            elif key in _SIDE_PROPS:
                out[self.apply_dir(key)] = value
            elif key in _ROTATION_SAFE:
                out[key] = value
            else:
                raise ValueError(f"don't know how to rotate property {key!r} of {state}")
        return BlockState(state.name, tuple(sorted(out.items())))


def parse_pos(text: str) -> Pos:
    parts = text.split(",")
    if len(parts) != 3:
        raise ValueError(f"not a position: {text!r}")
    x, y, z = (int(p) for p in parts)
    return (x, y, z)


def format_pos(pos: Pos) -> str:
    return f"{pos[0]},{pos[1]},{pos[2]}"
