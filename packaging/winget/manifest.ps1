<#
Renders winget's three manifests for one published release (piece 4 goal 3) into target\winget\<version>, for
`winget validate` and for the maintainer's first submission (`wingetcreate submit target\winget\<version>`). Later
releases are not rendered here: the publish workflow's `wingetcreate update` carries the listed manifest forward.

    powershell -ExecutionPolicy Bypass -File packaging\winget\manifest.ps1 -Version 0.1.0

The installer's SHA-256 is read from the release's own SHA256SUMS unless -Checksum is given.
#>
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Checksum
)

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
function Fail($why) { Write-Error $why -ErrorAction Continue; exit 1 }

if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "the version is x.y.z, not '$Version'" }
$installer = "Rustroom-$Version-windows-x64-setup.exe"
$release = "https://github.com/wladiKrin/rustroom/releases/download/v$Version"
if (-not $Checksum) {
    try { $sums = (Invoke-WebRequest "$release/SHA256SUMS" -UseBasicParsing).Content }
    catch { Fail "could not read $release/SHA256SUMS - is v$Version published? ($($_.Exception.Message))" }
    if ($sums -is [byte[]]) { $sums = [Text.Encoding]::ASCII.GetString($sums) }
    $pattern = "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($installer))\s*$"
    $line = $sums -split "`n" | Where-Object { $_ -match $pattern } | Select-Object -First 1
    if (-not ($line -and $line -match $pattern)) { Fail "no $installer in the release's SHA256SUMS" }
    $Checksum = $Matches[1]
}
# winget's own manifests write the hash in capitals.
$Checksum = $Checksum.ToUpper()
if ($Checksum -notmatch '^[0-9A-F]{64}$') { Fail "not a SHA-256: '$Checksum'" }

$out = "$root\target\winget\$Version"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
$values = @{ VERSION = $Version; CHECKSUM = $Checksum }
foreach ($f in Get-ChildItem "$root\packaging\winget\*.yaml") {
    $text = [IO.File]::ReadAllText($f.FullName)
    foreach ($k in $values.Keys) { $text = $text.Replace("@$k@", $values[$k]) }
    if ($text -match '@[A-Z]+@') { Fail "a placeholder is left in $($f.Name): $($Matches[0])" }
    [IO.File]::WriteAllText("$out\$($f.Name)", $text, (New-Object Text.UTF8Encoding $false))
}
Write-Host $out
