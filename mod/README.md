# The observer mod

A Fabric server mod that watches. Every block state change in the
overworld is collected, and at the end of each server tick the batch goes
out, with the game time, to every client of a WebSocket on localhost:

    {"tick": 1234, "changes": [["x,y,z", "minecraft:redstone_wire[...]"], ...]}

A hello with the current tick comes first. The end of every tick goes out
even when nothing changed, so a client can act on a given tick; what changes
between ticks (a command, say) goes out as the next tick starts, under the
number of the tick just completed, since that is when it counts. The port
is `-Dredfab.observer.port=N` (0 disables the stream; the mod still loads).
It changes no block, no tick and runs no command. `./check --vanilla` proves
that by replaying every mechanics recording on the modded server and
comparing the stream with the disk.

## The one request: forget a template

A client may send one kind of text frame, `{"forget": "redfab:NAME"}`. The
server thread then drops that structure template from its
`StructureTemplateManager` cache and the client gets `{"forgot": "redfab:NAME"}`
back on its own connection. This is the one departure from observing only,
and it touches no block: the game keeps every template a `/place template`
command has loaded until the next resource reload, so a backend that places
a new template per submission would grow the heap by about a megabyte per
design for as long as the server runs. Only the `redfab` namespace can be
forgotten; the live backend sends the request once the stream has shown the
design placed, and deletes the `.nbt` file itself. Anything else a client
sends is answered with `{"error": ...}` and ignored.

## Inside

Two Mixins carry it: `LevelChunk.setBlockState` (which returns the old state
when a block changed) and the head and tail of `MinecraftServer.tickServer`
(the head also hands the server to the mod, for the forget request). Each
client has a writer thread with a bounded queue; a client that cannot keep
up is dropped, and the thread ends within a second of its client going
(`mod/test/WebSocketServerTest.java` holds it to that: twenty clients, half
leaving with a close frame and half by dropping the socket, must leave no
writer or client thread behind; `tools/build_mod.sh` runs it after every
build, without Minecraft). The mod logs through the server's logger as
`redfab-observer`. No Fabric API, and no Gradle: Minecraft 26.3 ships
unobfuscated, so `tools/build_mod.sh` compiles the four sources with `javac`
against the server jar, Fabric Loader and Mixin installed in `.tools/fabric`
by `tools/setup.sh` (the JDK is found through the `.tools/java` link setup.sh
makes).

The harness runs the modded server with `VanillaServer(..., modded=True)` and
reads the stream with `harness/observe.py`; the live backend (`server/`)
drives its plots from it.
