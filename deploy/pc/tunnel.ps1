# The tunnel to the box's game port, forever: ssh exits when the link drops or
# the local port is still taken by the last one, and the loop brings it back.
# Runs hidden as the "redfab tunnel" task at logon (install.ps1).
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot\config.ps1"

# Stopping the task stops this script but not its ssh, which then outlives it: every restart
# (start.ps1 does one a day) left one more tunnel running. Any still here belongs to an earlier run.
Get-CimInstance Win32_Process -Filter "Name='ssh.exe'" |
    Where-Object { $_.CommandLine -like '*-L 25565:127.0.0.1:25565*camera@*' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

while ($true) {
    & ssh -N -L 25565:127.0.0.1:25565 -i $Key -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ExitOnForwardFailure=yes -o StrictHostKeyChecking=accept-new "camera@$Name"
    Start-Sleep -Seconds 5
}
