# Rustroom on Chocolatey

`choco install rustroom` runs the release's own Windows installer, downloaded from GitHub and checked against its
SHA-256. Nothing here holds a binary.

| File | What it is |
|---|---|
| `rustroom.nuspec` | the package's description, `@VERSION@` filled per release |
| `tools/chocolateyinstall.ps1` | downloads and runs the installer silently with `/ALLUSERS` |
| `tools/chocolateyuninstall.ps1` | runs the uninstaller of the all-users install only — never a user's own copy |
| `pack.ps1` | fills the three for one release and runs `choco pack` into `target\chocolatey` |

**For all users, on purpose.** Chocolatey runs elevated, often as another account, so a per-user install would land in
that account's profile. The installer accepts `/ALLUSERS` on its command line (`PrivilegesRequiredOverridesAllowed`) and
installs into Program Files; a double-clicked installer still installs per user with no admin prompt. Each user's
catalog, previews and settings stay in their own profile, and uninstalling leaves them.

⚠ **`v0.1.0`'s installer predates that switch** — with it, `/ALLUSERS` does nothing and Rustroom would install into the
elevated account's profile. The first package pushed is the first release built after it.

## Publishing

Every published release (not a draft or prerelease) is packed and pushed by `.github/workflows/publish.yml`, once the
repository secret `CHOCOLATEY_API_KEY` exists; without it the job says so and does nothing. Community packages are
moderated before they are listed, the first one more slowly.

The first time, by the maintainer:

1. Create an account on community.chocolatey.org and copy its API key (Account → API Keys).
2. Store it as the repository secret `CHOCOLATEY_API_KEY` (Settings → Secrets and variables → Actions).
3. Publish the next release, or run `pack.ps1 -Version <x.y.z>` and
   `choco push target\chocolatey\rustroom.<x.y.z>.nupkg --source https://push.chocolatey.org/ --api-key <key>` by hand.

## Testing locally

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\build.ps1
$sha = (Get-FileHash target\installer\Rustroom-<x.y.z>-windows-x64-setup.exe).Hash
powershell -ExecutionPolicy Bypass -File packaging\chocolatey\pack.ps1 -Version <x.y.z> -Checksum $sha `
    -Url "file:///$((Resolve-Path target\installer\Rustroom-<x.y.z>-windows-x64-setup.exe) -replace '\\','/')"
# in an elevated PowerShell:
choco install rustroom --source "$PWD\target\chocolatey" -y
choco uninstall rustroom -y
```
