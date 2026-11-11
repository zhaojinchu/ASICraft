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
`package.json` by `tools/playground_test.mjs`; a package file of 1 MB or more
goes out precompressed once its variants are made, brotli 11, zstd 19 or gzip 9
by what the browser takes, kept under `build/pkg-compressed/`: the Yosys core
and its resources are about 10 MB on the wire instead of 25), `designs/` (the examples) and
the core from `target/wasm32-unknown-unknown/release/redfab_wasm.wasm`, which
it builds first. Every answer carries a Content-Security-Policy that keeps
scripts, workers, styles, fetches and WebSockets to the page's own origin
(WebAssembly allowed; the inline import map and stylesheet allowed by hash),
and the headless check fails on any violation.

What a visitor meets: the editor on the left with a terminal under it, the
circuit on the right, and the counter example already built and counting
(the server built it once from `designs/counter4` with the page's own
pipeline, `prepareOpening` in `tools/playground_serve.mjs`, and serves its
design, report and structure under `/designs/counter4/`; the page
simulates it at once, switches its enable on, and fetches the Yosys core only
when the visitor shows they may compile: a click into the editor, another
example, the pointer on Compile). Above the view, the levers and lamps: a bus
(`name[i]` pins) sits in one box, most significant bit first, with its value
beside it; a clocked design's clock is a lever the page drives. A badge on the
view says what drives it (simulated here, or live in the game at a plot).
With a camera behind the page, a Circuit and a Camera tab share the stage:
Run in the game opens the camera, Compile the circuit. Under the editor: the
three files as exports, then the terminal, whose first line is the status as
a build tool prints it ("built not1 in 3.2 s", "live on plot 0"; red for an
error, amber for a warning), with the report open and the raw log closed.
The report on the page is a timing report written from the design: cells,
flip-flops, repeaters, blocks, area, then `T_clk`, `t_co` (when the outputs
are valid after the edge), `t_in` (the input window) and the setup and hold
slack for a clocked design, or `t_pd` (the propagation delay, min and max)
and `t_in,min` (the least spacing between input changes) for a combinational
one, in game ticks of 50 ms; the pins; and `LEC`, the equivalence proof. The
core's full report, with the placement, the lever and lamp positions and the
`/forceload` line for a world of your own, is the report.txt export.
White, with a redstone block's red as the accent (the title, the main action,
a lever that is on, an error), the system font for the page and the code font
for code and signals, Monocraft (`fonts/`, OFL 1.1) for the title; on a phone
the circuit comes first and the rest stacks under it.

Using it: pick an example or paste your own module. The top module is the one
named `top`, or the only module in the source; name another one in the `top`
field. Compile with the button or Ctrl+Enter (Cmd+Enter on a Mac). Edited
source turns the dropdown to `custom` and is kept in the browser's local
storage across reloads (the page still opens with the counter, and says the
draft is in the editor); switching back to an example asks first. The
status shows the phase and the elapsed seconds while compiling, then "built
NAME in N s" or why not; about 100 cells compile inside the 90 s limit
(`LIMITS.measuredCells` in `compile.js`, measured on 2026-10-07 and held to
by a test that compiles an 8-bit adder through the WebAssembly pipeline; 150
take 2 to 3 minutes natively, which the limit cuts off). A design too large
for the server's plot is not sent. A design over the hard limit of 400 cells is refused right after
synthesis, before the proof and the build, and one above the measured size
gets a warning in the log at the same point. The three buttons in the result
card download what `redfab build` writes, byte for byte: `NAME.nbt` (the
structure, for `/place template`; Litematica should load it too, untested),
`NAME.report.txt` and `NAME.design.json`; for the opening circuit they come
from the server.

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
- `worker.js`: compiles and simulates off the main thread, and loads Yosys
  when the page asks it to warm up or at the first compile, never when the
  page opens. The page stops it if a compile takes longer than the limit.
- `fonts/`: Monocraft, the heading font, with its license (OFL 1.1).
- `camera.js`: the camera tab (stage 14), shown when the backend has a
  camera: the game itself, the user's own client streamed from the relay on
  the box, played over WebRTC (WHEP at `/camera/camera/whep`) while the tab is
  open and the page visible, and tried again a few seconds after a refusal or
  a dropped stream (WebRTC's "disconnected" is given five seconds to pass on
  its own first). Two buttons: Auto view, where the backend frames the plots,
  and Manual control, offered while a plot is yours, where the keys (W A S D,
  space, shift, a drag on the video to look) drive the character through the
  plot's live socket, as `server/README.md` describes; the camera holds where
  you leave it until you press Auto view. The bar says only how to drive
  while driving, and what is wrong when something is.
- `compile.js`: the pipeline, the same as `redfab build`: synthesis, the
  equivalence proof, then placement, routing and the checks in the core. Also
  the limits: source size, cells, blocks, and the compile time the page
  enforces; and the plain-words explanations of the refusals Yosys and the
  core word in their own terms (colour codes stripped, the first error line
  as the message).
- `core.js`: the WebAssembly core behind one JSON call.
- `view.js`: three.js: every block an instance of a cube, recoloured as it
  changes state; a frame is drawn only when something changed, and none while
  the camera tab hides the view.

Served this way, nothing is sent anywhere, so nothing needs guarding beyond
the browser's own sandbox and the limits that keep a bad design from hanging
the page. Served by the live backend (`server/`, stage 12) the page also
offers **Run in the game**, as its main action: the source goes to the server, which builds it,
places it in the game, and streams the game's blocks back; the levers then
reach the probe blocks in the world, and the stage shows the camera (or the
game's blocks in the view, on the Circuit tab or with no camera). The live
backend compiles the examples into its cache when it starts, so running one
unedited waits for no compile. The plot is
given back when you Stop, compile again, or leave the page. The world.nbt
button stays: it serves the local build of the same source when there is one,
and otherwise asks the server for `/api/plots/N/structure`.

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG
OR MICROSOFT.
