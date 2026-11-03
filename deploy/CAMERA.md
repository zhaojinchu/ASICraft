# The camera: your PC's side

Stage 14 of `docs/PLAN.md`. Your own account joins the server from your PC
as a spectator, OBS streams what it sees to the relay on the box, and the
page plays it and lets visitors drive the character. The box's side is
`deploy/README.md` ("The camera"); this is what runs on the PC and how to
keep it running.

## What you need

- A PC that stays on, with a GPU or a recent CPU: the game client capped at
  30 frames a second at 720p and OBS with the GPU's encoder are a light load
  for a desktop (a laptop is the one thing not to use for this).
- Your Minecraft account, logged into the official launcher on that PC only:
  a second login anywhere else invalidates this one.
- The Fabric loader for Minecraft 26.3 (the installer from fabricmc.net,
  "client", which makes a launcher profile), and the camera mod,
  `build/mod/redfab-camera.jar` from `tools/build_mod.sh` on a checkout with
  the client jar fetched (`tools/setup.sh --client`), dropped into the
  profile's `mods` folder.
- OBS 30 or newer (it publishes WHIP natively).
- An SSH key pair for the tunnel, made on the PC (`ssh-keygen -t ed25519 -f
  %USERPROFILE%\.ssh\camera`), whose public key the box's operator puts on
  the box as the `camera` user with port forwarding only (below).
- From the box's operator: the site's name (NAME), the mod's key
  (`/etc/redfab/camera.env` on the box) and the relay's password
  (`/etc/redfab/relay.env`).

## On the box, once (the operator)

    useradd --system --create-home --shell /usr/sbin/nologin camera
    install -d -m 700 -o camera -g camera /home/camera/.ssh
    echo 'restrict,port-forwarding,permitopen="127.0.0.1:25565" ssh-ed25519 AAAA... camera@pc' > /home/camera/.ssh/authorized_keys
    chown camera:camera /home/camera/.ssh/authorized_keys && chmod 600 /home/camera/.ssh/authorized_keys

The key can open one tunnel, to the game port on loopback, and nothing else:
no shell, no other forwarding. The game's port is fixed at 25565 by the
backend's unit (`--game-port 25565`; without it the backend takes a free
port at every start). It never opens to the internet and `online-mode`
stays off, which is acceptable only because this tunnel is the one way in.
The whitelist is on, and the backend adds your player's name to it when the
mod says hello, so that nobody else can join through a leaked key.

## On the PC, once

1. **The tunnel.** `ssh -N -L 25565:127.0.0.1:25565 -i %USERPROFILE%\.ssh\camera -o ServerAliveInterval=30 -o ExitOnForwardFailure=yes camera@NAME`
   in a scheduled task that runs at logon, as your user, "restart the task if
   it fails" every minute, so it comes back after any drop. Test it by hand
   first; it prints nothing and stays open.
2. **The profile.** In the launcher's Fabric 26.3 profile, add to the JVM
   arguments: `-Dredfab.camera.url=wss://NAME/camera?key=KEY` with the mod's
   key. The mod reads it at start, connects outbound to the backend, says
   hello with your player name, and the backend puts you in spectator mode.
3. **The game.** Video settings: render distance 12, max framerate 30, VSync
   off, "pause on lost focus" off, fullscreen off at 1280 by 720 (the stream's
   size). Multiplayer, direct connect, `localhost:25565`. You appear as a
   spectator above the plots; the mod flies the camera from then on, and the
   overlay line in the corner names the plot and who drives.
4. **OBS.** Sources: Game Capture of the Minecraft window (not Display
   Capture: the screen can be off). Settings, Output: encoder NVENC, AMF or
   QuickSync, rate control CBR, bitrate 3000 kbps, keyframe interval 1 s,
   preset for low latency, 1280 by 720 at 30. Settings, Stream: service WHIP,
   server `https://NAME/camera/camera/whip`, bearer token
   `camera:PASSWORD` with the relay's password. Start Streaming. The page's
   camera panel says "camera up" within five seconds.
5. **The nightly restart.** The game's login token expires after a day and
   the client's memory grows over a long session, so a second scheduled task
   at a quiet hour (04:00) closes the game and OBS and starts them again: the
   launcher with the profile (`MinecraftLauncher.exe --workDir ... ` or the
   profile's shortcut), then OBS with `--startstreaming --minimize-to-tray`.
   OBS reconnects on its own if the relay was restarted.
6. **Power.** Never sleep, never hibernate; the display may turn off. Windows
   Update: set active hours and let it restart only in them, since a reboot
   ends the stream until the tasks bring everything back at logon (set the
   user to log in automatically).

## When it stops

- The page says "camera off: the stream is not up": OBS is not publishing.
  Check OBS's status bar; the relay's log on the box is
  `journalctl -u redfab-relay -f`.
- The page says "camera off: the camera is not connected": the mod is not
  connected to the backend. Check that the game is running with the profile
  that has the `-Dredfab.camera.url` argument, and the backend's log
  (`journalctl -u redfab-live -f`, "camera: NAME connected").
- The game says "Connection refused" on direct connect: the tunnel is down.
  Run the task again, or the ssh command by hand to see the error.
- The character does not move when a visitor drives: the backend's status
  (`/api/status`, `camera.driver`) says who drives; if it names a driver and
  nothing moves, the mod's log in the game's `logs/latest.log` says what it
  received.

The account's token refresh is yours: roughly every three months the
launcher asks you to sign in again, and the nightly restart then starts the
game without a session until you do.

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR
MICROSOFT.
