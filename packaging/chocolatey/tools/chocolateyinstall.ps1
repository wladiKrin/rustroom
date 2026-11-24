# Installs Rustroom from its GitHub release (pack.ps1 fills the URL and SHA-256). Chocolatey runs elevated, often as
# another account, so the installer is told /ALLUSERS: Program Files and the all-users Start menu, never the elevated
# account's own profile. The catalog and settings are each user's own either way.
$ErrorActionPreference = 'Stop'

$log = Join-Path $env:TEMP "$($env:ChocolateyPackageName).$($env:ChocolateyPackageVersion).install.log"
$packageArgs = @{
    packageName    = $env:ChocolateyPackageName
    softwareName   = 'Rustroom'
    fileType       = 'exe'
    url64bit       = '@URL@'
    checksum64     = '@CHECKSUM@'
    checksumType64 = 'sha256'
    silentArgs     = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /ALLUSERS /LOG=`"$log`""
    validExitCodes = @(0)
}
Install-ChocolateyPackage @packageArgs
