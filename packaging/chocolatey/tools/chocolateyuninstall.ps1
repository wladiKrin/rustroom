# Uninstalls the all-users Rustroom this package installed - only the one under HKLM, never a user's own per-user copy
# (the same AppId, under HKCU). The catalog, previews and settings stay; the installer never removes them.
$ErrorActionPreference = 'Stop'

[array]$keys = Get-UninstallRegistryKey -SoftwareName 'Rustroom' |
    Where-Object { $_.PSPath -like '*HKEY_LOCAL_MACHINE*' }

if ($keys.Count -eq 1) {
    $packageArgs = @{
        packageName    = $env:ChocolateyPackageName
        softwareName   = 'Rustroom'
        fileType       = 'exe'
        silentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART'
        validExitCodes = @(0)
        file           = $keys[0].UninstallString.Trim('"')
    }
    Uninstall-ChocolateyPackage @packageArgs
} elseif ($keys.Count -eq 0) {
    Write-Warning "No all-users Rustroom is installed - it may have been uninstalled already."
} else {
    Write-Warning "$($keys.Count) all-users Rustroom installs found - uninstalling none; remove them in Settings > Apps."
}
