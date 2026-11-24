<#
Builds Rustroom's Store package (piece 4 goal 2): an MSIX of the very exe and licences build.ps1 staged for the
installer, so the two can never differ.

    powershell -ExecutionPolicy Bypass -File packaging\windows\msix.ps1          # the Store's, unsigned
    powershell -ExecutionPolicy Bypass -File packaging\windows\msix.ps1 -Test    # to sideload on this PC

Needs build.ps1 run first (its stage folder) and the Windows SDK (makepri.exe, makeappx.exe; signtool.exe for -Test).
The identity is what Partner Center shows under Product identity once the name is reserved, read from
RUSTROOM_MSIX_NAME, RUSTROOM_MSIX_PUBLISHER and RUSTROOM_MSIX_PUBLISHER_DISPLAY_NAME (the release workflow sets them
from repository variables); with any of them unset the script says so and exits 0 without a package. The Store signs
the package itself, so it is left unsigned.

-Test packs a fake identity (CN=Rustroom Test) and signs it with a throwaway certificate kept in target\msix (made on
the first run, never committed). Windows installs it only once that certificate is trusted on the machine - a step
for the machine's owner, undone after the test.

Writes target\msix\Rustroom-<version>-windows-x64.msix (with -Test: ...-test.msix) and prints its SHA-256. Every
step that fails stops the script. The package is never a release asset: an unsigned MSIX installs nowhere, and the
author uploads it to Partner Center by hand.
#>
param([switch]$Test)

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
Set-Location $root

function Fail($why) { Write-Error $why -ErrorAction Continue; exit 1 }

$meta = cargo metadata --format-version 1 --no-deps --manifest-path Cargo.toml | ConvertFrom-Json
if ($LASTEXITCODE) { Fail "cargo metadata failed" }
$version = ($meta.packages | Where-Object { $_.name -eq 'app' }).version
if (-not $version) { Fail "no version for the app crate" }

if ($Test) {
    $identity = @{ NAME = 'Rustroom.Test'; PUBLISHER = 'CN=Rustroom Test'; PUBLISHER_DISPLAY_NAME = 'Rustroom test build' }
} else {
    $identity = @{
        NAME                   = $env:RUSTROOM_MSIX_NAME
        PUBLISHER              = $env:RUSTROOM_MSIX_PUBLISHER
        PUBLISHER_DISPLAY_NAME = $env:RUSTROOM_MSIX_PUBLISHER_DISPLAY_NAME
    }
    $missing = $identity.Keys | Where-Object { -not $identity[$_] } | ForEach-Object { "RUSTROOM_MSIX_$_" }
    if ($missing) {
        Write-Host "No Store package: $($missing -join ', ') unset - set them from Partner Center's Product identity"
        exit 0
    }
}
# The Store's version has four parts and its last must be 0.
$identity.VERSION = "$version.0"
Write-Host "Rustroom $version - Store package as $($identity.NAME)"

# The installer's stage, never a build of its own: the same bytes in both.
$stage = "$root\target\installer\stage"
if (-not (Test-Path "$stage\Rustroom.exe")) { Fail "no $stage\Rustroom.exe - run packaging\windows\build.ps1 first" }
if ((Get-Item "$stage\Rustroom.exe").VersionInfo.ProductVersion -ne $version) {
    Fail "$stage\Rustroom.exe is not version $version - run packaging\windows\build.ps1 again"
}

# The newest SDK that has the tools.
$kits = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
$sdk = Get-ChildItem $kits -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match '^\d+(\.\d+){3}$' -and (Test-Path "$($_.FullName)\x64\makeappx.exe") } |
    Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
if (-not $sdk) { Fail "no Windows SDK with makeappx.exe under $kits" }
$tools = "$($sdk.FullName)\x64"
Write-Host "Windows SDK $($sdk.Name)"

$out = "$root\target\msix"
$layout = "$out\layout"
if (Test-Path $layout) { Remove-Item -Recurse -Force $layout }
New-Item -ItemType Directory -Force "$layout\Assets" | Out-Null
Copy-Item "$stage\*" $layout -Recurse
Copy-Item "$root\packaging\icons\msix\*.png" "$layout\Assets"

# Placeholders are @NAME@-style, so the manifest's $(KnownFolder:...) tokens pass through untouched.
$manifest = [IO.File]::ReadAllText("$root\packaging\windows\AppxManifest.xml")
foreach ($key in $identity.Keys) {
    $manifest = $manifest.Replace("@$key@", [Security.SecurityElement]::Escape($identity[$key]))
}
if ($manifest -match '@[A-Z_]+@') { Fail "a placeholder is left in the manifest: $($Matches[0])" }
[IO.File]::WriteAllText("$layout\AppxManifest.xml", $manifest, (New-Object Text.UTF8Encoding $false))

# resources.pri: how Windows finds Assets\Square44x44Logo.png among its scale and size variants. The config keeps
# no <packaging> section, which would split the index into resource packs a single package does not have.
$priconfig = "$out\priconfig.xml"
& "$tools\makepri.exe" createconfig /cf $priconfig /dq en-US /pv 10.0.0 /o | Out-Null
if ($LASTEXITCODE) { Fail "makepri createconfig failed" }
[xml]$cfg = Get-Content $priconfig
$packaging = $cfg.resources.SelectSingleNode('packaging')
if ($packaging) { $cfg.resources.RemoveChild($packaging) | Out-Null }
$cfg.Save($priconfig)
& "$tools\makepri.exe" new /pr $layout /cf $priconfig /mn "$layout\AppxManifest.xml" /of "$layout\resources.pri" /o
if ($LASTEXITCODE) { Fail "makepri new failed" }

$name = if ($Test) { "Rustroom-$version-windows-x64-test.msix" } else { "Rustroom-$version-windows-x64.msix" }
$msix = "$out\$name"
& "$tools\makeappx.exe" pack /d $layout /p $msix /o
if ($LASTEXITCODE) { Fail "makeappx pack failed - its message above names the manifest's fault" }

if ($Test) {
    # A throwaway certificate whose subject is the fake publisher; made once into target\msix, never a store's.
    $pfx = "$out\test-signing.pfx"
    $cer = "$out\test-signing.cer"
    $password = ConvertTo-SecureString 'rustroom-test' -AsPlainText -Force
    if (-not (Test-Path $pfx)) {
        $cert = New-SelfSignedCertificate -Type Custom -Subject $identity.PUBLISHER -KeyUsage DigitalSignature `
            -FriendlyName 'Rustroom test package (throwaway)' -CertStoreLocation 'Cert:\CurrentUser\My' `
            -NotAfter (Get-Date).AddMonths(3) `
            -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
        Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $password | Out-Null
        Export-Certificate -Cert $cert -FilePath $cer | Out-Null
        Remove-Item "Cert:\CurrentUser\My\$($cert.Thumbprint)"
    }
    & "$tools\signtool.exe" sign /fd SHA256 /f $pfx /p 'rustroom-test' $msix
    if ($LASTEXITCODE) { Fail "signtool failed" }
    Write-Host "Signed with $pfx - trust $cer (Local Machine, Trusted People) to install it"
}

$hash = (Get-FileHash $msix -Algorithm SHA256).Hash.ToLower()
Write-Host $msix
Write-Host "SHA-256 $hash"
