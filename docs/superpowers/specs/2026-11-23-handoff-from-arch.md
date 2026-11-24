# Hand-off from Arch (2026-11-23) — piece 4's goals 2 and 3 on the Windows PC

> Written at the close of piece 4's goal 1 (the update notice), for the next session on the Windows PC. **Read this
> first, then the design** (`2026-11-23-v0.1-piece4-updates-and-stores-design.md`, beside this file — goals 2 and 3
> are its sections; this file does not repeat them). The rules are `AGENTS.md` and the PC's own `CLAUDE.local.md`.

## Where things stand

- **Since the last Windows session the repository was rewritten and moved** (going public, 2026-11-20): it is now
  `github.com/wladiKrin/rustroom`, and the old clone on the PC points at a history that no longer exists. **Re-clone**,
  and in the new clone set `git config user.email 115639331+wladiKrin@users.noreply.github.com` (the global one is the
  author's real address, which must never enter the public history).
- **`v0.1.0` is published**, with the Windows installer (`Rustroom-0.1.0-windows-x64-setup.exe`), the Mac disk image and
  the AppImage.
- **Goal 1 is built** (branch `update-notice`; merged only when the author has seen it). `rustroom_app::update_check`
  holds every decision; `update_flow` (bin) starts the check at launch and draws the status line; Settings → Behaviour
  has **Check for new versions**. A build with `RUSTROOM_NO_UPDATE_CHECK` set compiles it off.

## On the PC, goal 2 — the Store (MSIX)

1. **Package identity silences the notice.** `update_check::updated_elsewhere` knows a Flatpak only. Add the Store:
   `GetCurrentPackageFullName` returning anything but `APPMODEL_ERROR_NO_PACKAGE` (15700) means a packaged install —
   through `windows-sys` (feature `Win32_Storage_Packaging_Appx`), `cfg(windows)` at **every** reference, its `use`
   lines included (`AGENTS.md`'s macOS-module warning, the same shape). Its test: an unpackaged test process answers
   `false`. Seen for real only in the sideloaded package (step 3).
2. **`packaging/windows/msix.ps1`** and `AppxManifest.xml` as the design says; a workflow artifact only, skipped while
   the repository variables for the identity are unset.
3. ⚠⚠ **The shared-catalog proof, before anything else is built on it:** install the installer build, make a catalog
   and change a setting; install the Store build sideloaded (signed with a throwaway self-made certificate, never
   committed); it must open the SAME catalog and settings; uninstall it, and they must still be there. Find the
   mechanism the Store accepts (flexible virtualization's excluded directories, or `unvirtualizedResources`); if none
   is accepted, **stop and ask the author**.
4. **`PRIVACY.md`** drafted (what Rustroom sends where: Immich, the tile server, Nominatim, the model downloads, the
   update check; it collects nothing itself) — ⚠ shown to the author and committed only once he approves the text.
5. For the author: the first-submission checklist in `CONTRIBUTING.md`, and a draft `runFullTrust` justification.

## Goal 3 — winget and Chocolatey

- Chocolatey: `packaging/chocolatey/`; the installer gains `PrivilegesRequiredOverridesAllowed=commandline` so
  Chocolatey's `/ALLUSERS` installs into Program Files — ⚠ check that a double-clicked installer still asks no admin and
  still finds an earlier per-user install (the `AppId` never changes).
- winget: the first submission is the author's (`wingetcreate new`, id `wladiKrin.Rustroom`, `inno`, user scope).
- Both from a new workflow on `release: published`; the secrets (a winget-pkgs token, the Chocolatey API key) are set
  **by the author**. ⚠ **Nothing is submitted or pushed to a registry during development** — `choco pack` and a local
  `choco install`, `winget validate` and a local manifest install only.

## The PC's limits (unchanged)

No page file: `CARGO_BUILD_JOBS=4` (2 for test builds) and the app closed during a gate; `VCPKG_ROOT` set
(`CONTRIBUTING.md`).
