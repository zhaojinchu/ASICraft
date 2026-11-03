# Copy this file to config.ps1 beside it (git-ignored) and fill it in. No secret
# lives here: the mod's key is in the Prism instance's Java arguments, the relay's
# password in OBS's stream settings.
$Name = 'redfab.example'                                              # the site's name
$Key = "$env:USERPROFILE\.ssh\camera"                                 # the tunnel's private key, no passphrase
$Prism = "$env:LOCALAPPDATA\Programs\PrismLauncher\prismlauncher.exe" # where Prism's installer put it
$Instance = 'redfab'                                                  # the Prism instance's name
$Obs = 'C:\Program Files\obs-studio\bin\64bit\obs64.exe'
