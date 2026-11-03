# The tunnel to the box's game port, forever: ssh exits when the link drops or
# the local port is still taken by the last one, and the loop brings it back.
# Runs hidden as the "redfab tunnel" task at logon (install.ps1).
$ErrorActionPreference = 'Continue'
. "$PSScriptRoot\config.ps1"
while ($true) {
    & ssh -N -L 25565:127.0.0.1:25565 -i $Key -o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ExitOnForwardFailure=yes -o StrictHostKeyChecking=accept-new "camera@$Name"
    Start-Sleep -Seconds 5
}
