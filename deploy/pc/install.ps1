# Registers the two scheduled tasks as the logged-in user and starts them:
# "redfab tunnel" at logon (tunnel.ps1, which loops forever) and "redfab camera"
# at logon and daily at 04:00 (start.ps1, which restarts the game and OBS). Both
# run in the user's own desktop session, hidden, and come back after a failure.
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

Register-ScheduledTask -TaskName 'redfab tunnel' -Action (Action 'tunnel.ps1') -Trigger $atLogon -Principal $principal -Settings $settings -Force | Out-Null
Register-ScheduledTask -TaskName 'redfab camera' -Action (Action 'start.ps1') -Trigger @($atLogon, $nightly) -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'redfab tunnel'
Start-ScheduledTask -TaskName 'redfab camera'
'registered and started: "redfab tunnel" (at logon) and "redfab camera" (at logon and 04:00); Task Scheduler lists them'
