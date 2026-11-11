# The camera mod

A Fabric client mod for the user's own game client (stage 14 of
`docs/PLAN.md`, the PC's side in `deploy/CAMERA.md`): it flies the
character, a spectator, where the live backend says, so that the page can
stream the game itself and let visitors drive the camera.

Started with `-Dredfab.camera.url=wss://NAME/camera?key=KEY` in the launcher
profile's JVM arguments, it connects outbound to the backend's `/camera`
socket with the launcher's player name (`server/README.md`, "The camera");
the backend whitelists the player, and the server makes it a spectator as it
joins. Without the
property the mod loads and does nothing. Then, at the end of every client
tick:

- A driver's input (`{"drive": {"move", "look", ...}}`, ten a second while
  someone drives) turns the character by the look and moves it by the move
  in its own frame, 12 blocks a second.
- With no input for two seconds, the automatic camera glides to the framing
  pose above the plot the backend last named (`{"frame": ...}` on every
  placement and lever change while nobody drives): above and south of the
  plot's centre, facing north, looking down at 65 degrees, as near as the
  whole plot fits the view; then it orbits the plot slowly.
- The position is set outright every tick, so the keyboard at the PC does
  nothing, and it is kept over the plots' area.

The HUD is not drawn while the mod is the camera (the property is set): no
crosshair, chat or hotbar over the circuits, as F1 would do without touching
the game's own state. The page says which plot is on camera and who drives.

## Inside

- `dev.redfab.camera.CameraMath`: the arithmetic, with nothing of the game in
  it: framing, the orbit, the glide, a drive input into a velocity with the
  game's yaw convention, the look, the bounds. `test/CameraMathTest.java`.
- `dev.redfab.camera.Json`: enough JSON for the messages, no library.
  `test/JsonTest.java`.
- `dev.redfab.camera.Backend`: the JDK's WebSocket client, the hello, the
  frames and drives handed to a listener, reconnection with a growing wait.
  `test/BackendTest.java` runs it against the observer mod's WebSocket server
  as a fake backend.
- `dev.redfab.camera.mc.CameraMod` and the two Mixins (`Minecraft.tick`,
  `Hud.extractRenderState`): the game-facing part, compiled only against the
  official client jar that `tools/setup.sh --client` fetches, which stays on
  the machine that builds the mod.

`tools/build_mod.sh` compiles and tests the first three always, and builds
`build/mod/redfab-camera.jar` when the client jar is there; the jar goes into
the launcher profile's `mods` folder beside the Fabric loader for 26.3.

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR
MICROSOFT.
