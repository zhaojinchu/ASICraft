"""Block-state strings: parsing, canonical form, rotation."""

import pytest

from harness.blockstate import BlockState, Rotation


def test_canonical_sorts_properties():
    s = BlockState.parse("minecraft:redstone_wire[west=side,power=15,east=none,north=up,south=side]")
    assert str(s) == "minecraft:redstone_wire[east=none,north=up,power=15,south=side,west=side]"


def test_namespace_defaults_to_minecraft():
    assert str(BlockState.parse("stone")) == "minecraft:stone"
    assert str(BlockState.parse("repeater[delay=2]")) == "minecraft:repeater[delay=2]"


def test_no_defaulting():
    # The parser keeps exactly the properties given; the game fills in the rest.
    assert str(BlockState.parse("minecraft:repeater[delay=2]")) == "minecraft:repeater[delay=2]"


def test_from_palette_entry_both_shapes():
    assert str(BlockState.from_palette({"": "minecraft:air"})) == "minecraft:air"
    entry = {"id": "minecraft:redstone_torch", "properties": {"lit": "false"}}
    assert str(BlockState.from_palette(entry)) == "minecraft:redstone_torch[lit=false]"


def test_matches_ignores_unlisted_properties():
    full = BlockState.parse("minecraft:redstone_wire[east=side,north=none,power=15,south=none,west=side]")
    assert full.matches(BlockState.parse("minecraft:redstone_wire[power=15]"))
    assert not full.matches(BlockState.parse("minecraft:redstone_wire[power=14]"))
    assert not full.matches(BlockState.parse("minecraft:stone"))


@pytest.mark.parametrize(
    "state, rotated",
    [
        ("minecraft:repeater[delay=1,facing=north]", "minecraft:repeater[delay=1,facing=east]"),
        ("minecraft:repeater[facing=east]", "minecraft:repeater[facing=south]"),
        ("minecraft:redstone_wall_torch[facing=west,lit=true]", "minecraft:redstone_wall_torch[facing=north,lit=true]"),
        ("minecraft:lever[face=wall,facing=south,powered=false]", "minecraft:lever[face=wall,facing=west,powered=false]"),
        (
            "minecraft:redstone_wire[east=side,north=up,power=3,south=none,west=none]",
            "minecraft:redstone_wire[east=up,north=none,power=3,south=side,west=none]",
        ),
        ("minecraft:stone", "minecraft:stone"),
    ],
)
def test_rotate_clockwise_quarter(state, rotated):
    # One quarter turn clockwise when seen from above: north -> east -> south -> west.
    assert str(Rotation(1).apply_state(BlockState.parse(state))) == rotated


def test_rotate_position_and_inverse():
    rot = Rotation(1)
    for pos in [(0, 0, 0), (3, 1, 0), (0, 2, 5), (-2, 0, 7)]:
        assert rot.inverse().apply_pos(rot.apply_pos(pos)) == pos
    # Four quarter turns are the identity.
    pos = (3, 1, 7)
    for _ in range(4):
        pos = rot.apply_pos(pos)
    assert pos == (3, 1, 7)


def test_rotate_unknown_directional_property_is_an_error():
    with pytest.raises(ValueError):
        Rotation(1).apply_state(BlockState.parse("minecraft:oak_log[axis=x]"))
