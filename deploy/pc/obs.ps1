# OBS, stopped and started the one way this setup does both. Dot-sourced by
# start.ps1 and watch.ps1, after config.ps1.

# Stops OBS outright, whatever it was doing.
function Stop-Obs {
    Get-Process -Name obs64 -ErrorAction SilentlyContinue | Stop-Process -Force
}

# Starts OBS streaming to the tray, unless one already runs (a second would stop
# on its "already running" dialog).
function Start-Obs {
    if (Get-Process -Name obs64 -ErrorAction SilentlyContinue) { return }
    # OBS keeps a sentinel per run and drops it on a clean exit. Stop-Obs leaves one, and OBS 32
    # then stops on its "Crash Detected" dialog at the next start, flag or no flag, streaming nothing.
    # This OBS is only ever stopped that way, so its sentinels are cleared first.
    Get-ChildItem -LiteralPath (Join-Path $env:APPDATA 'obs-studio\.sentinel') -Force -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    Start-Process -FilePath $Obs -WorkingDirectory (Split-Path $Obs) -ArgumentList '--startstreaming', '--minimize-to-tray', '--disable-shutdown-check'
}
