# Starts the camera, or restarts it: closes the game and OBS if they run, brings
# the tunnel task back, launches the Prism instance joining the server through
# the tunnel, then OBS streaming to the tray. Runs hidden as the "redfab camera"
# task at logon and daily at 04:00 (install.ps1); by hand, it is the restart.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\config.ps1"

# The game is the Java process started with the mod's property; nothing else is touched.
Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*redfab.camera.url*' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Get-Process -Name obs64 -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 5

# A fresh tunnel, if the tasks are registered (by hand before install.ps1, the hand-run one serves).
if (Get-ScheduledTask -TaskName 'redfab tunnel' -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName 'redfab tunnel'
    Start-ScheduledTask -TaskName 'redfab tunnel'
    Start-Sleep -Seconds 10
}

Start-Process -FilePath $Prism -ArgumentList '--launch', $Instance, '--server', 'localhost:25565'
# The game loads and joins; then OBS's Game Capture has a window to hook.
Start-Sleep -Seconds 90

Start-Process -FilePath $Obs -WorkingDirectory (Split-Path $Obs) -ArgumentList '--startstreaming', '--minimize-to-tray', '--disable-shutdown-check'
