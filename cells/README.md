# Tile library

Every circuit redfab builds is assembled from the tiles in this directory. A tile is a 5×5 block footprint, up to 4 blocks tall, written as text: one grid per height level, north at the top, west on the left.

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
- Dust and torches stand on glass, never on stone. Powered dust weakly powers the block under it, and stone would pass that on to torches and lamps next to it; glass does not conduct.

Dust is written without shape or power. The build computes both, with every input off, and writes the finished state.
