# The observer mod

A Fabric server mod that only watches. Every block state change in the
overworld is collected, and at the end of each server tick the batch goes
out, with the game time, to every client of a WebSocket on localhost:

    {"tick": 1234, "changes": [["x,y,z", "minecraft:redstone_wire[...]"], ...]}

A hello with the current tick comes first; a heartbeat with no changes comes
every twenty ticks. The port is `-Dredfab.observer.port=N` (0 disables the
stream; the mod still loads). It changes nothing: no block, no tick, no
command. `./check --vanilla` proves that by replaying every mechanics
recording on the modded server and comparing the stream with the disk.

Two Mixins carry it: `LevelChunk.setBlockState` (which returns the old state
when a block changed) and the tail of `MinecraftServer.tickServer`. No Fabric
API, and no Gradle: Minecraft 26.3 ships unobfuscated, so `tools/build_mod.sh`
compiles the four sources with `javac` against the server jar, Fabric Loader
and Mixin installed in `.tools/fabric` by `tools/setup.sh`.

The harness runs the modded server with `VanillaServer(..., modded=True)` and
reads the stream with `harness/observe.py`.
