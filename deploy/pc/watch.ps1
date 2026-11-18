# The camera's watchdog, forever. OBS's WHIP output does not reconnect: when the
# link to the relay drops, even for a moment, OBS stops streaming and the page's
# camera stays offline until something starts it again (on 2026-11-17 that was
# 11 hours, until the nightly restart). Every 30 s this asks the site whether the
# relay has the stream. With the game connected and no stream for 90 s it
# restarts OBS; if the stream is still missing 90 s after that, or the game has
# not been connected for 5 minutes, it restarts the whole camera (start.ps1,
# through its task). It leaves everything alone while the site does not answer
# and while start.ps1 runs. What it does goes to %LOCALAPPDATA%\redfab\watch.log.
# Runs hidden as the "redfab watch" task at logon (install.ps1).
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot\config.ps1"
. "$PSScriptRoot\obs.ps1"

$logDir = Join-Path $env:LOCALAPPDATA 'redfab'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
function Note($text) {
    "$(Get-Date -Format s) $text" | Add-Content -LiteralPath (Join-Path $logDir 'watch.log')
}

$dark = 0                         # checks in a row with the game connected and no stream
$gone = 0                         # checks in a row with no game connected
$obsAt = [datetime]::MinValue     # when this last restarted OBS
Note 'watching'
while ($true) {
    Start-Sleep -Seconds 30
    $starting = (Get-ScheduledTask -TaskName 'redfab camera' -ErrorAction SilentlyContinue).State -eq 'Running'
    try {
        $camera = (Invoke-RestMethod -Uri "https://$Name/api/status" -TimeoutSec 10).camera
    } catch {
        $camera = $null
    }
    if ($starting -or $null -eq $camera) { $dark = 0; $gone = 0; continue }
    if (-not $camera.connected) { $dark = 0; $gone++ }
    elseif ($camera.streaming -eq $false) { $gone = 0; $dark++ }
    else { $dark = 0; $gone = 0 }

    if ($gone -ge 10 -or ($dark -ge 3 -and ((Get-Date) - $obsAt).TotalMinutes -lt 5)) {
        Note $(if ($gone -ge 10) { 'the game has not been connected for 5 minutes: restarting the camera' }
               else { 'still no stream after restarting OBS: restarting the camera' })
        Start-ScheduledTask -TaskName 'redfab camera'
        $dark = 0; $gone = 0; $obsAt = [datetime]::MinValue
    } elseif ($dark -ge 3) {
        Note 'no stream for 90 s with the game connected: restarting OBS'
        Stop-Obs
        Start-Sleep -Seconds 3
        Start-Obs
        $dark = 0; $obsAt = Get-Date
    }
}
