# Registers the three scheduled tasks as the logged-in user and starts them:
# "redfab tunnel" at logon (tunnel.ps1, which loops forever), "redfab camera"
# at logon and daily at 04:00 (start.ps1, which restarts the game and OBS) and
# "redfab watch" at logon (watch.ps1, which loops forever and brings the stream
# back when it drops). All run in the user's own desktop session, hidden, and
# come back after a failure. Running it again re-registers them and restarts all three.
# Run once from the clone, after config.ps1 is filled in and everything worked by hand:
#   powershell -ExecutionPolicy Bypass -File deploy\pc\install.ps1
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\config.ps1"
foreach ($path in $Key, $Prism, $Obs) {
    if (-not (Test-Path $path)) { throw "not found: $path (fix config.ps1)" }
}

$shell = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
function Action($script) {
    New-ScheduledTaskAction -Execute $shell -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$PSScriptRoot\$script`""
}
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited
# No time limit (the tunnel runs for days), a restart a minute after a failure, and no
# stopping for power: the PC stays plugged in anyway.
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -DontStopOnIdleEnd
$atLogon = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$nightly = New-ScheduledTaskTrigger -Daily -At '04:00'

$tasks = @(
    @{ Name = 'redfab tunnel'; Script = 'tunnel.ps1'; Triggers = @($atLogon) },
    @{ Name = 'redfab camera'; Script = 'start.ps1'; Triggers = @($atLogon, $nightly) },
    @{ Name = 'redfab watch'; Script = 'watch.ps1'; Triggers = @($atLogon) }
)
foreach ($task in $tasks) {
    # A task already running keeps its old script until stopped; the starts below run the new one.
    Stop-ScheduledTask -TaskName $task.Name -ErrorAction SilentlyContinue
    Register-ScheduledTask -TaskName $task.Name -Action (Action $task.Script) -Trigger $task.Triggers -Principal $principal -Settings $settings -Force | Out-Null
}
foreach ($task in $tasks) { Start-ScheduledTask -TaskName $task.Name }
'registered and started: "redfab tunnel" and "redfab watch" (at logon), "redfab camera" (at logon and 04:00); Task Scheduler lists them'
