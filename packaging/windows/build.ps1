<#
Builds Rustroom's Windows installer (v0.1, piece 3a): the release exe, its licences, and Inno Setup's installer.
The release workflow runs this same script, so a local build and a release cannot differ.

    powershell -ExecutionPolicy Bypass -File packaging\windows\build.ps1

Needs: VCPKG_ROOT with libheif[core,libde265]:x64-windows-static; cargo-about (cargo install cargo-about --locked
--features cli); Inno Setup 6 (ISCC.exe on PATH or in its default folder). Writes
target\installer\Rustroom-<version>-windows-x64-setup.exe and its .sha256, and prints both. Every step that fails
stops the script.
#>

$root = (Resolve-Path "$PSScriptRoot\..\..").Path
Set-Location $root

function Fail($why) { Write-Error $why -ErrorAction Continue; exit 1 }

# The version is the app crate's: the installer's name, its AppVersion, and what a release tag must equal.
$meta = cargo metadata --format-version 1 --manifest-path Cargo.toml | ConvertFrom-Json
if ($LASTEXITCODE) { Fail "cargo metadata failed" }
$version = ($meta.packages | Where-Object { $_.name -eq 'app' }).version
if (-not $version) { Fail "no version for the app crate" }
Write-Host "Rustroom $version"

cargo build --release -p app
if ($LASTEXITCODE) { Fail "cargo build failed" }
$exe = "$root\target\release\Rustroom.exe"
# A development build may lack its resources (build.rs only warns); a release never does.
if ((Get-Item $exe).VersionInfo.ProductVersion -ne $version) {
    Fail "Rustroom.exe carries no version information (build.rs could not run rc.exe) - refusing to package it"
}

$stage = "$root\target\installer\stage"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force "$stage\licenses" | Out-Null
Copy-Item $exe $stage
Copy-Item "$root\LICENSE" "$stage\LICENSE.txt"

cargo about generate --manifest-path crates/app/Cargo.toml packaging/about.hbs -o "$stage\THIRD-PARTY-LICENSES.html"
if ($LASTEXITCODE) { Fail "cargo about failed - is it installed, and is every crate's licence in about.toml?" }

# The native libraries' and fonts' own texts, each from its source — never a copy kept here.
function CrateDir($name) {
    $p = $meta2.packages | Where-Object { $_.name -eq $name } | Select-Object -First 1
    if (-not $p) { Fail "no $name in the dependency graph" }
    Split-Path $p.manifest_path
}
$meta2 = cargo metadata --format-version 1 --manifest-path Cargo.toml --filter-platform x86_64-pc-windows-msvc | ConvertFrom-Json
if ($LASTEXITCODE) { Fail "cargo metadata failed" }
if (-not $env:VCPKG_ROOT) { Fail "VCPKG_ROOT is not set" }
$vcpkgShare = "$env:VCPKG_ROOT\installed\x64-windows-static\share"
$notices = [ordered]@{
    'LibRaw-LICENSE.LGPL.txt'  = "$root\vendor\rsraw-sys\LibRaw\LICENSE.LGPL"
    'LibRaw-LICENSE.CDDL.txt'  = "$root\vendor\rsraw-sys\LibRaw\LICENSE.CDDL"
    'LibRaw-COPYRIGHT.txt'     = "$root\vendor\rsraw-sys\LibRaw\COPYRIGHT"
    'libheif-copyright.txt'    = "$vcpkgShare\libheif\copyright"
    'libde265-copyright.txt'   = "$vcpkgShare\libde265\copyright"
    'LittleCMS-LICENSE.txt'    = "$(CrateDir 'lcms2-sys')\vendor\LICENSE"
    'LensFun-NOTICE.txt'       = "$(CrateDir 'lensfun')\NOTICE"
    'Inter-LICENSE.txt'        = "$root\crates\app\assets\Inter-LICENSE"
    'Phosphor-LICENSE.txt'     = "$root\crates\app\assets\Phosphor-LICENSE"
}
foreach ($name in $notices.Keys) {
    $from = $notices[$name]
    if (-not (Test-Path $from)) { Fail "missing licence text: $from" }
    Copy-Item $from "$stage\licenses\$name"
}

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) { Fail "Inno Setup 6 (ISCC.exe) not found" }
& $iscc /Q "/DVersion=$version" "/DStage=$stage" "/DOutput=$root\target\installer" "$root\packaging\windows\rustroom.iss"
if ($LASTEXITCODE) { Fail "Inno Setup failed" }

$out = "$root\target\installer\Rustroom-$version-windows-x64-setup.exe"
$hash = (Get-FileHash $out -Algorithm SHA256).Hash.ToLower()
"$hash  $(Split-Path $out -Leaf)" | Out-File -Encoding ascii "$out.sha256"
Write-Host $out
Write-Host "SHA-256 $hash"
