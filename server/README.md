# The live playground's backend

The playground page with a server behind it: the source goes to the server,
the server builds it, places it in the game, and the page shows the game's
blocks instead of its own simulation. Levers on the page reach probe blocks
in the world; nothing else from a browser ever reaches the game.

    node server/live.mjs [--port 8766] [--plots 8] [--run-dir build/mc/live]

That starts a Fabric server with the observer mod (`tools/setup.sh` and
`tools/build_mod.sh` first), then serves the page at http://127.0.0.1:8766/.
Click **Run on the server**.

## What happens to a submission

1. `POST /api/submit` with `{source, top, name}` and nothing else. Any other
   field is refused: a submission is source only.
2. The source is compiled in a child process (`sandbox.mjs`,
   `compile_child.mjs`): the same pipeline as the page and `redfab build`,
   YoWASP Yosys and the WebAssembly core, with the same limits on source size,
   cells and blocks. The child is killed past the time limit (90 s) and past the
   memory limit (2 GB; a normal compile peaks near 900 MB). Compiles run one at
   a time behind a short queue; beyond it, the answer is 429.
3. A free plot is taken (503 if none). Plots are a row of fixed cells in an
   otherwise empty world; a design larger than a cell is refused.
4. The structure file the core wrote is placed with `/place template … strict`,
   exactly as the checks place designs, in forceloaded chunks. The backend
   watches the mod's stream until it shows the design in place, and records
   whether what appeared equals the design file.
5. The reply carries the plot number, a token for its levers, the design (the
   page draws it) and the report. The page opens `ws://…/live?plot=N&token=T`.

## The stream

The mod sends every block change in the overworld with its tick. The backend
keeps each plot's blocks as the stream says, and relays changes inside the
plot to its viewers in design coordinates, with the lamp and lever states:

    {"tick": 1234, "changes": [["x,y,z", "state"], ...], "lamps": [0, 1], "inputs": [1, 0]}

A viewer holding the token sends `{"set": "a", "on": true}`; the backend sets
the probe block of input `a` with one `setblock`. For a clocked design the
backend drives the clock from the stream: an edge due at tick T goes out as
tick T's message arrives, so it lands between T and T+1, which the trace
format counts as the last thing in tick T. The mod flushes what changes
between ticks under the number of the tick just completed, so a command's
effects never mix with the next tick's.

Everything since placement is a recording in the trace format:
`GET /api/plots/N/trace` returns the fixture (the design, placed strictly, with
every lever change as an event at its tick) and the trace (every change the
stream reported, by tick). `redfab check-fixtures` replays the fixture in the
simulator, and the checks require the replay to equal the recording tick for
tick: that is how the live server is held to the same standard as everything
else.

A plot is released by its owner (`DELETE /api/plots/N?token=T`), after ten
minutes with nobody controlling it, or after an hour; the backend fills it with
air. `GET /api/status` reports free plots and the server's tick time from
`/tick query`, sampled every few seconds.

## Files

- `live.mjs`: the HTTP and WebSocket front, `createLive` (any world: the
  checks give it a fake one) and `startLive` (launches Minecraft).
- `plots.mjs`: plots, their blocks as the stream says, levers, the clock
  driver, viewers and the recording.
- `minecraft.mjs`: launches the Fabric server the way the harness does, with
  the world the backend talks to: place, setblock, fill, forceload, tick query,
  the stream.
- `sandbox.mjs`, `compile_child.mjs`: compiles in a child process with time
  and memory limits.
- `rcon.mjs`, `ws.mjs`: the RCON client and a small WebSocket server, no
  dependencies.
