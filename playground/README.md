# The playground

A page that turns SystemVerilog into a redstone circuit and runs it, with no
server: synthesis (YoWASP Yosys) and the compiler core (this repository's Rust
crates built for WebAssembly) both run in a worker in the browser, and the
simulator that drives the 3D view is the same one the checks compare with the
game, tick for tick.

    node tools/playground_serve.mjs        # then open http://127.0.0.1:8765/

The server is a few lines of Node that serve this directory, `node_modules/`
(Yosys and three.js), `designs/` (the examples) and the core from
`target/wasm32-unknown-unknown/release/redfab_wasm.wasm`; build the core first
with `cargo build -p redfab-wasm --target wasm32-unknown-unknown --release`.

Files:

- `index.html`, `app.js` — the page: an editor, Compile, a panel of levers and
  lamps, and the view. The simulation runs at the game's 20 ticks a second; a
  clocked design's clock is driven by the page at the period its timing report
  states. Click a lever in the view or its button in the panel to flip it.
- `worker.js` — compiles and simulates off the main thread. The page stops it
  if a compile takes longer than the limit.
- `compile.js` — the pipeline, the same as `redfab build`: synthesis, the
  equivalence proof, then placement, routing and the checks in the core. Also
  the limits: source size, cells, blocks, and the compile time the page enforces.
- `core.js` — the WebAssembly core behind one JSON call.
- `view.js` — three.js: every block an instance of a cube, recoloured as it
  changes state.

What is not here: nothing is sent anywhere, so nothing needs guarding beyond
the browser's own sandbox and the limits that keep a bad design from hanging
the page. The live playground (stage 12) adds the server side.
