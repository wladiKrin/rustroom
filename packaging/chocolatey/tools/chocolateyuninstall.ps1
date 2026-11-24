# Uninstalls the all-users Rustroom this package installed: the installer's own uninstall key (its fixed AppId + "_is1")
# under HKLM - never a user's per-user copy, which has the same key under HKCU. By the key, not the name: Inno names the
# all-users entry "Rustroom (All users)". The catalog, previews and settings stay; the installer never removes them.
$ErrorActionPreference = 'Stop'

$key = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{240EB815-FED8-4E19-84FE-EFE2069F1D00}_is1'
$entry = Get-ItemProperty $key -ErrorAction SilentlyContinue
if ($entry -and $entry.UninstallString) {
    $packageArgs = @{
        packageName    = $env:ChocolateyPackageName
        softwareName   = $entry.DisplayName
        fileType       = 'exe'
        silentArgs     = '/VERYSILENT /SUPPRESSMSGBOXES /NORESTART'
        validExitCodes = @(0)
        file           = $entry.UninstallString.Trim('"')
    }
    Uninstall-ChocolateyPackage @packageArgs
} else {
    Write-Warning "No all-users Rustroom is installed - it may have been uninstalled already."
}
