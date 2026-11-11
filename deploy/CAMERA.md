# The camera: your PC's side

Stage 14 of `docs/PLAN.md`. Your own account joins the server from your PC
as a spectator, OBS streams what it sees to the relay on the box, and the
page plays it and lets visitors drive the character. The box's side is
`deploy/README.md` ("The camera"); this is what runs on the PC and how to
keep it running, from a clone of this repository on the PC.

## What you need

- A PC that stays on, with a GPU or a recent CPU: the game client capped at
  30 frames a second at 720p and OBS with the GPU's encoder are a light load
  for a desktop (a laptop is the one thing not to use for this).
- Your Minecraft account, logged in on that PC only: a second login
  anywhere else invalidates this one.
- Prism Launcher (prismlauncher.org, the Windows installer, not the Store
  build). It installs the Fabric loader and the game's Java itself, and its
  command line launches an instance and joins a server, which the official
  launcher cannot do and the nightly restart needs.
- The camera mod, `build/mod/redfab-camera.jar`. It is not in the
  repository: `tools/build_mod.sh` builds it on a Mac or Linux checkout
  that fetched the client jar (`tools/setup.sh --client`), and it is copied
  to the PC.
- OBS 30 or newer (it publishes WHIP natively).
- Git for Windows, for the clone and for `ssh`.
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
The whitelist is on, and the backend adds your player to it when the mod
says hello, so that nobody else can join through a leaked key. Every player
joins as a spectator (`force-gamemode` in the backend's server settings).

## On the PC, once

In PowerShell, as your user. Steps 1 and 2 need nothing from the box.

1. **The clone and the key.**

       git clone https://github.com/zhaojinchu/ASICraft.git
       cd ASICraft
       ssh-keygen -t ed25519 -f "$env:USERPROFILE\.ssh\camera"

   Press Enter twice at the passphrase: the tunnel runs unattended, so the
   key cannot have one. Send the operator the public half only,
   `$env:USERPROFILE\.ssh\camera.pub`, and wait for the box side above and
   the three things from the operator.

2. **The config.** Copy `deploy\pc\config.example.ps1` to
   `deploy\pc\config.ps1` (git-ignored) and fill it in: the site's name,
   where Prism and OBS are installed. No secret goes in it.

3. **The tunnel, by hand first.**

       ssh -N -L 25565:127.0.0.1:25565 -i "$env:USERPROFILE\.ssh\camera" -o ExitOnForwardFailure=yes camera@NAME

   Answer yes to the host key. Then it prints nothing and stays open; leave
   it open for the next steps (Ctrl-C ends it).

4. **The instance.** In Prism: Accounts, Add Microsoft, sign in. Add
   Instance: name `redfab`, Minecraft 26.3, mod loader Fabric. Edit, Mods,
   Add file: the mod's jar. Edit, Settings, Java: tick Java arguments and
   put `-Dredfab.camera.url=wss://NAME/camera?key=KEY` with the mod's key.
   Launch once; Prism fetches the game and, if it asks, a Java for it (26.3
   runs on Java 25). The mod reads the property at start, connects outbound
   to the backend, says hello with your player name, and the backend puts
   you on the whitelist; the server makes you a spectator when you join.

5. **The game.** Video settings: render distance 16 (the server's view
   distance; more shows nothing more), max framerate 30, VSync
   off, "pause on lost focus" off, fullscreen off at 1280 by 720 (the stream's
   size), and as plain as the game allows: clouds off, particles minimal,
   entity shadows, smooth lighting, vignette, view bobbing and the
   distortion and FOV effects off, biome blend 0, chunk fade-in 0. In
   `options.txt` of the instance's `minecraft` folder, with the game closed:
   `renderClouds:"false"`, `particles:2`, `entityShadows:false`, `ao:false`,
   `vignette:false`, `bobView:false`, `screenEffectScale:0.0`,
   `fovEffectScale:0.0`, `darknessEffectScale:0.0`, `biomeBlendRadius:0`,
   `chunkSectionFadeInTime:0.0`. Multiplayer, Direct Connect, `localhost:25565`. You appear as a
   spectator above the plots; the mod flies the camera from then on and draws
   no HUD, so the stream is the game's picture alone. The backend's
   log on the box says "camera: NAME connected".

6. **OBS.** Sources: Game Capture, mode "capture specific window", the game's
   window, match priority "match title, otherwise find window of same
   executable" (not Display Capture: the screen can be off). Settings,
   Output: encoder NVENC, AMF or QuickSync, rate control CBR, bitrate 3000
   kbps, keyframe interval 1 s, the low-latency preset; Video: 1280 by 720
   at 30. Settings, Stream: service WHIP, server
   `https://NAME/camera/camera/whip`, bearer token `camera:PASSWORD` with
   the relay's password. Start Streaming. The page's camera panel says
   "camera up" within five seconds.

7. **The tasks.** Close the game, OBS and the hand-run tunnel, then

       powershell -ExecutionPolicy Bypass -File deploy\pc\install.ps1

   registers two scheduled tasks as your user and starts them: "redfab
   tunnel" at logon (`tunnel.ps1`, the ssh command in a loop that brings it
   back after any drop) and "redfab camera" at logon and daily at 04:00
   (`start.ps1`: closes the game and OBS, restarts the tunnel task, launches
   the instance joining `localhost:25565`, then OBS streaming to the tray).
   The nightly restart is there because the game's login token expires
   after a day and the client's memory grows over a long session. Within
   two minutes of either trigger the page's camera is up, with no click on
   the PC. Running `start.ps1` by hand restarts everything at any time.

8. **Power.** Never sleep, never hibernate; the display may turn off. Set
   the user to log in automatically, so that a Windows Update reboot ends
   the stream only until the tasks come back at logon, and set active hours
   so that it reboots at night.

## When it stops

- The page says "camera off: the stream is not up": OBS is not publishing.
  Check OBS's status bar; the relay's log on the box is
  `journalctl -u redfab-relay -f`.
- The page says "camera off: the camera is not connected": the mod is not
  connected to the backend. Check that the game runs from the `redfab`
  instance with the `-Dredfab.camera.url` argument (the instance's log in
  Prism says what the mod did), and the backend's log
  (`journalctl -u redfab-live -f`, "camera: NAME connected").
- The game says "Connection refused" on direct connect: the tunnel is down.
  Task Scheduler shows "redfab tunnel"'s last result; run the ssh command of
  step 3 by hand to see the error.
- The character does not move when a visitor drives: the backend's status
  (`/api/status`, `camera.driver`) says who drives; if it names a driver and
  nothing moves, the instance's log says what the mod received.
- Everything is up but the page shows nothing: run `start.ps1` by hand once
  and watch it; a path in `config.ps1` that moved (an update of Prism or
  OBS) is the usual cause, and `install.ps1` refuses a missing one.

If the account's session ever lapses, Prism asks you to sign in again, and
until you do the nightly restart starts the game without one.

NOT AN OFFICIAL MINECRAFT PRODUCT. NOT APPROVED BY OR ASSOCIATED WITH MOJANG OR
MICROSOFT.
