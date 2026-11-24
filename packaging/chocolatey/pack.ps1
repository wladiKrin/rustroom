<#
Packs Rustroom's Chocolatey package for one published release (piece 4 goal 3).

    powershell -ExecutionPolicy Bypass -File packaging\chocolatey\pack.ps1 -Version 0.2.0

Fills the nuspec and install script with the version, the release's installer URL and its SHA-256 - read from the
release's own SHA256SUMS unless -Checksum is given - and runs `choco pack` into target\chocolatey. For a local test of
an installer that is not released yet, -Url takes a file:/// address and -Checksum its hash. Publishing (`choco push`)
is not this script's: the publish workflow does it, with the author's API key.
#>
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Checksum,
    [string]$Url
)

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
function Fail($why) { Write-Error $why -ErrorAction Continue; exit 1 }

if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "the version is x.y.z, not '$Version'" }
$installer = "Rustroom-$Version-windows-x64-setup.exe"
$release = "https://github.com/wladiKrin/rustroom/releases/download/v$Version"
if (-not $Url) { $Url = "$release/$installer" }
if (-not $Checksum) {
    try { $sums = (Invoke-WebRequest "$release/SHA256SUMS" -UseBasicParsing).Content }
    catch { Fail "could not read $release/SHA256SUMS - is v$Version published? ($($_.Exception.Message))" }
    if ($sums -is [byte[]]) { $sums = [Text.Encoding]::ASCII.GetString($sums) }
    $pattern = "^([0-9a-fA-F]{64})\s+\*?$([regex]::Escape($installer))\s*$"
    $line = $sums -split "`n" | Where-Object { $_ -match $pattern } | Select-Object -First 1
    if (-not ($line -and $line -match $pattern)) { Fail "no $installer in the release's SHA256SUMS" }
    $Checksum = $Matches[1]
}
$Checksum = $Checksum.ToLower()
if ($Checksum -notmatch '^[0-9a-f]{64}$') { Fail "not a SHA-256: '$Checksum'" }

$out = "$root\target\chocolatey"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force "$out\tools" | Out-Null
$values = @{ VERSION = $Version; URL = $Url; CHECKSUM = $Checksum }
$files = @('rustroom.nuspec', 'tools\chocolateyinstall.ps1', 'tools\chocolateyuninstall.ps1')
foreach ($f in $files) {
    $text = [IO.File]::ReadAllText("$root\packaging\chocolatey\$f")
    foreach ($k in $values.Keys) { $text = $text.Replace("@$k@", $values[$k]) }
    if ($text -match '@[A-Z]+@') { Fail "a placeholder is left in ${f}: $($Matches[0])" }
    # With a byte-order mark: Windows PowerShell 5.1, which runs Chocolatey's scripts, reads one without it in the
    # system code page.
    [IO.File]::WriteAllText("$out\$f", $text, (New-Object Text.UTF8Encoding $true))
}

choco pack "$out\rustroom.nuspec" --outputdirectory $out --limit-output
if ($LASTEXITCODE) { Fail "choco pack failed" }
$nupkg = "$out\rustroom.$Version.nupkg"
if (-not (Test-Path $nupkg)) { Fail "choco pack wrote no $nupkg" }
Write-Host $nupkg
Write-Host "installs $Url"
Write-Host "SHA-256 $Checksum"
