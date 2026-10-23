# Tile library

Every circuit redfab builds is assembled from the tiles in this directory. A tile is a 5×5 block footprint, 4 blocks tall, written as text: one grid per height level, north at the top, west on the left. Five is the smallest footprint in which one wire can bridge over another (see `cross.tile`), which is what fixed the size; the wiring of a routing column is narrower, 3×5 (the `_n` tiles), since its bridge climbs north to south and needs the five only in depth.

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

- A signal enters or leaves only at the middle of an edge, as redstone dust, on one of two wire levels: y = 1 (the lower, dust on glass) or y = 3 (the upper, dust on stone at y = 2).
- No other block touches the tile's border, except the stone an upper port's dust stands on.
- A tile is written for one orientation; the build may place it rotated by quarter turns. Gates always take inputs on W, N or S and drive E, so they are never rotated.
- A tile is one cell deep and a whole number of cells wide (`size 10 4 5` for two), or narrow, three blocks wide (`size 3 4 5`), for a routing column. Its north and south ports sit on its first cell, at the middle of a narrow one. A side-locked repeater needs its lock's input one cell in from the border, so a flip-flop takes two cells.
- Lower dust and torches stand on glass, never on stone. Powered dust weakly powers the block under it, and stone would pass that on to torches and lamps next to it; glass does not conduct. Upper dust stands on stone (it has to step onto it at a ramp), which is safe because only wiring tiles have upper ports, so the stone is never beside a torch or lamp; the adjacency check proves it for every pair.

Dust is written without shape or power. The build computes both, with every input off, and writes the finished state.

| Tile | Ports | Does |
|---|---|---|
| `input` | out E | A lever on a stone block. Tests swap the block for a redstone block. An input that nothing reads keeps the lever and has no dust. |
| `output` | in W | Dust into a lamp. |
| `wire` | io W E N S | Joins whichever of its edges are used: a straight, a turn, or a fan-out. The build may put a repeater in the arm the signal enters by (the cell next to the port, facing the port): full strength again, 2 to 8 game ticks later. |
| `wire_up` | io W E N S, upper | The wire tile on the upper level: dust at y = 3, each on a stone. A tile may carry a lower and an upper wire at once (a stack), so two nets cross as two straights, or two turns share a tile; the stone under the upper dust keeps them apart (dust only weakly powers the block it stands on, which the lower dust cannot read, and a conducting block above the lower dust stops it connecting diagonally upward). It may carry a repeater like the wire. Only wiring has upper ports, so no lamp or torch is ever beside the stone. |
| `ramp` | io W lower, E upper | From one level to the other on stone steps, the way the crossing's arm climbs; turned, it joins any edge's lower port to the opposite edge's upper one. One net's alone, with no room for a repeater. |
| `wire_n`, `wire_up_n` | io W E N S | The two wires, three blocks wide, for a routing column: the north-south arm down the middle column, the west-east arm along the middle row, a hop across costing three dust steps instead of five. A repeater entered from the north or south fits any shape; one entered from the west or east has the centre for its arm, so a straight carries it there and a turn in the arm it leaves by, next to that port and facing the centre it reads (the port must then carry two, not one); a fan-out entered from the west or east has no single place for one. |
| `cross_n` | io W E N S | The crossing three blocks wide: the flat west-east arm (three blocks, a repeater at the centre under the bridge) and the north-south arm climbing over it. It cannot be turned, so the climbing net never takes a repeater here, and loses five levels in each: after a repeater it can climb two in a row and no more. The router charges such a crossing more and counts a run of three climbed by one net as contended, which sends long vertical runs to the upper level. |
| `ramp_n`, `ramp_ns_n` | io W lower, E upper; N lower, S upper | The ramps three blocks wide: west to east in three blocks (lower port, one step, upper port), north to south in five; each turned half round for the other way. |
| `cross` | io W E N S | West to east and north to south, not touching. The flat west-east arm may carry a repeater like a wire; the build turns the tile when the other signal is the one that needs it. |
| `nor` | in W N S, out E | NOR of the used inputs; NOT with one. |
| `tie_high`, `tie_low` | out E | Constant 1 and constant 0. |
| `dff` | in W (D), in N (clock), out E (Q) | The flip-flop: two cells wide. A master/slave pair of locked repeaters acting on the **falling** clock edge. Measured in stage 8: data stable from 1 tick before the edge to 5 after, Q 8 ticks after the edge, clock high and low at least 4 ticks each, period at least 12 (`timing` line in the file). |
| `dff_alt` | the same | The other candidate, a pulse latch on the rising edge: works, but samples late (hold 11) and needs a period of 16. Kept for the record. |
