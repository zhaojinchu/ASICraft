# The playground

A page that turns SystemVerilog into a redstone circuit and runs it, with no
server: synthesis (YoWASP Yosys) and the compiler core (this repository's Rust
crates built for WebAssembly) both run in a worker in the browser, and the
simulator that drives the 3D view is the same one the checks compare with the
game, tick for tick.

    node tools/playground_serve.mjs        # then open http://127.0.0.1:8765/

The server is a few lines of Node that serve this directory, the two packages
the page needs from `node_modules/` (YoWASP Yosys and three.js, under
`/pkg/NAME@VERSION/` so the browser can keep the 67 MB Yosys core for good;
the versions are pinned in `index.html` and `worker.js` and checked against
`package.json` by `tools/playground_test.mjs`), `designs/` (the examples) and
the core from `target/wasm32-unknown-unknown/release/redfab_wasm.wasm`, which
it builds first. Every answer carries a Content-Security-Policy that keeps
scripts, workers, styles, fetches and WebSockets to the page's own origin
(WebAssembly allowed; the inline import map and stylesheet allowed by hash),
and the headless check fails on any violation.

Using it: pick an example or paste your own module. The top module is the one
named `top`, or the only module in the source; name another one in the `top`
field. Compile with the button or Ctrl+Enter (Cmd+Enter on a Mac). Edited
source turns the dropdown to `custom` and is kept in the browser's local
storage across reloads; switching back to an example asks first. The status
line says what size works before the first compile, shows the phase and the
elapsed seconds while compiling, then the cell count against what the router
manages today (`LIMITS.measuredCells` in `compile.js`: about 100 cells compile
inside the 90 s limit, measured on 2026-10-07 and held to by a test that
compiles an 8-bit adder through the WebAssembly pipeline; 150 take 2 to 3
minutes natively, which the limit cuts off) and, when a live server is behind
the page, whether the design fits its plot. A design that does not fit is not
sent. A design over the hard limit of 400 cells is refused right after
synthesis, before the proof and the build, and one above the measured size
gets a warning in the log at the same point. The three buttons after Run
download what `redfab build` writes, byte for byte: `NAME.nbt` (the
structure, for `/place template`; Litematica should load it too, untested),
`NAME.report.txt` and `NAME.design.json`.

Files:

- `index.html`, `app.js`: the page: an editor, Compile, a panel of levers and
  lamps, and the view. The simulation runs at the game's 20 ticks a second; a
  clocked design's clock is driven by the page at the period its timing report
  states, counted in simulated ticks (not wall-clock intervals), so a slow
  machine stretches time but never shortens a period: at rest through the
  first period, then active from each period's start for `high` ticks, the
  convention the native cycle protocol uses, and Pause and Run inside a pulse
  keep the level. Click a lever in the view or its button in the panel to
  flip it; what repowers at once is recoloured at once. On a clocked design
  a click waits for the cycle's input window (`input_change_at` after the
  edge, the report's rule and what the server does in live mode): the button
  shows a dashed border until then, and a second click takes it back.
- `worker.js`: compiles and simulates off the main thread. The page stops it
  if a compile takes longer than the limit.
- `compile.js`: the pipeline, the same as `redfab build`: synthesis, the
  equivalence proof, then placement, routing and the checks in the core. Also
  the limits: source size, cells, blocks, and the compile time the page
  enforces; and the plain-words explanations of the refusals Yosys and the
  core word in their own terms (colour codes stripped, the first error line
  as the message).
- `core.js`: the WebAssembly core behind one JSON call.
- `view.js`: three.js: every block an instance of a cube, recoloured as it
  changes state.

Served this way, nothing is sent anywhere, so nothing needs guarding beyond
the browser's own sandbox and the limits that keep a bad design from hanging
the page. Served by the live backend (`server/`, stage 12) the page also
offers **Run on the server**: the source goes to the server, which builds it,
places it in the game, and streams the game's blocks back; the view then
shows those, and the levers reach the probe blocks in the world. The plot is
given back when you Stop, compile again, or leave the page. The world.nbt
button stays: it serves the local build of the same source when there is one,
and otherwise asks the server for `/api/plots/N/structure`.

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG
OR MICROSOFT.
