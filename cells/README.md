# Tile library

Every circuit redfab builds is assembled from the tiles in this directory. A tile is a 5×5 block footprint, 4 blocks tall, written as text: one grid per height level, north at the top, west on the left. Five is the smallest footprint in which one wire can bridge over another (see `cross.tile`), which is what fixed the size.

```
tile nor                 name
size 5 4 5               x (east), y (up), z (south)
port W in  0 1 2         a signal crosses this edge as dust at this cell
function nor             what the tile computes (omit for wiring)
delay 2                  game ticks from input to output
legend
  # minecraft:stone
  w minecraft:redstone_wire if W     only present when port W is used
layer 1                  y = 1; five rows of five characters, north to south
  ..n..
```

Lines starting with `#` before `legend` are comments.

Rules every tile follows, so that neighbouring tiles cannot interfere:

- A signal enters or leaves only at the middle of an edge, at y = 1, as redstone dust.
- No other block touches the tile's border.
- A tile is written for one orientation; the build may place it rotated by quarter turns. Gates always take inputs on W, N or S and drive E, so they are never rotated.
- Dust and torches stand on glass, never on stone. Powered dust weakly powers the block under it, and stone would pass that on to torches and lamps next to it; glass does not conduct.

Dust is written without shape or power. The build computes both, with every input off, and writes the finished state.

| Tile | Ports | Does |
|---|---|---|
| `input` | out E | A lever on a stone block. Tests swap the block for a redstone block. An input that nothing reads keeps the lever and has no dust. |
| `output` | in W | Dust into a lamp. |
| `wire` | io W E N S | Joins whichever of its edges are used: a straight, a turn, or a fan-out. The build may put a repeater in the arm the signal enters by (the cell next to the port, facing the port): full strength again, 2 to 8 game ticks later. |
| `cross` | io W E N S | West to east and north to south, not touching. The flat west-east arm may carry a repeater like a wire; the build turns the tile when the other signal is the one that needs it. |
| `nor` | in W N S, out E | NOR of the used inputs; NOT with one. |
| `tie_high`, `tie_low` | out E | Constant 1 and constant 0. |
