# Rustroom on winget

`winget install wladiKrin.Rustroom` runs the release's own Windows installer, per user like a double-click, from its
GitHub URL with its SHA-256 — once the package is listed in
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs).

| File | What it is |
|---|---|
| `wladiKrin.Rustroom*.yaml` | the three manifests (version, installer, `en-US` description), `@VERSION@` and `@CHECKSUM@` filled per release |
| `manifest.ps1` | renders them for one published release into `target\winget\<version>` |

`v0.1.0` works as it is: winget installs per user, which every release's installer does by default.

## Publishing

The first submission is the maintainer's, by hand:

```powershell
powershell -ExecutionPolicy Bypass -File packaging\winget\manifest.ps1 -Version 0.1.0
winget validate --manifest target\winget\0.1.0
winget install wingetcreate
wingetcreate submit target\winget\0.1.0 --token <a GitHub token with public_repo>
```

`wingetcreate submit` forks `microsoft/winget-pkgs` under the token's account and opens the pull request; Microsoft's
checks and a moderator review it. After it is merged, every published release (not a draft or prerelease) is submitted
by `.github/workflows/publish.yml` with `wingetcreate update`, once the repository secret `WINGET_TOKEN` (the same kind
of token) exists; without it the job says so and does nothing.

The update notice still shows in a winget install — it runs the same installer, so Rustroom cannot tell it from a
download — and its link is still right.
