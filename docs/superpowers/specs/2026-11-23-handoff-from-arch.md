# Hand-off from Arch (2026-11-23, updated 2026-11-24) — piece 4's goals 2 and 3 on the Windows PC

> Written at the close of piece 4's goal 1 (the update notice), for the next session on the Windows PC. **Read this
> first, then the design** (`2026-11-23-v0.1-piece4-updates-and-stores-design.md`, beside this file — goals 2 and 3
> are its sections; this file does not repeat them). The rules are `AGENTS.md` and the PC's own `CLAUDE.local.md`.

## Before anything else

1. **Re-clone.** Since the last Windows session the repository was rewritten and moved (going public, 2026-11-20): it is
   now `github.com/wladiKrin/rustroom`, and the old clone on the PC points at a history that no longer exists. Keep the
   old clone until the new one builds, then delete it.
2. **Set the commit address in the new clone:** `git config user.email 115639331+wladiKrin@users.noreply.github.com` —
   the global one is the author's real address, which must never enter the public history. Check with
   `git log -1 --format=%ae` after the first commit.
3. **Copy `CLAUDE.local.md` from Arch by hand** (untracked, private): the Arch copy holds the current state line. Keep
   the PC's own lines (`VCPKG_ROOT`, the page-file limits) where they differ.
4. **Run the gate once** on the fresh clone before changing anything (`CONTRIBUTING.md`; on this PC
   `CARGO_BUILD_JOBS=2` for the test build, the app closed). Expected: 0 failed (Linux ran 4,818 at `c5aec1e0`; the
   Windows count differs — tethering is `cfg(not(windows))` — so compare it with the last Windows count in the journal).

## Where things stand (`main` at `c5aec1e0`, pushed, CI green)

- **`v0.1.0` is published**, with the Windows installer (`Rustroom-0.1.0-windows-x64-setup.exe`), the Mac disk image and
  the AppImage. The repository is public.
- **Goal 1, the update notice, is merged** (2026-11-24). `rustroom_app::update_check` holds every decision; `update_flow`
  (bin) starts the check at launch and draws the status line — "Rustroom x.y.z is available", **Hide** (this run only)
  and **Don't show again** (turns Settings → Behaviour's **Check for new versions** off). A build with
  `RUSTROOM_NO_UPDATE_CHECK` set compiles it off. `grep -n 'the update notice (branch' docs/JOURNAL.md` for the story.
- **"Updated elsewhere" is looked up ONCE per run** (`update_check::updated_elsewhere`, a `OnceLock` through
  `remembered`) — the status row asks every frame. The Store's check (below) goes INSIDE that probe, never beside it.
- **The workflows' actions are on Node 24** (checkout v7, cache v6, upload-artifact v7, download-artifact v8). ⚠
  download-artifact v8 has not run yet — it runs only in the release job, at a tag; it now FAILS on a digest mismatch
  where v4 warned.
- The Mac's Developer ID is ⛔ declined (free routes only). Flathub (goal 4) waits for the author's own submission.

## Goal 2 — the Store (MSIX)

1. **Package identity silences the notice.** Add the Store to `updated_elsewhere`'s probe:
   `GetCurrentPackageFullName` returning anything but `APPMODEL_ERROR_NO_PACKAGE` (15700) means a packaged install —
   through `windows-sys` (feature `Win32_Storage_Packaging_Appx`), `cfg(windows)` at **every** reference, its `use`
   lines included (`AGENTS.md`'s macOS-module warning, the same shape). Its test: an unpackaged test process answers
   `false`. Seen for real only in the sideloaded package (step 3).
2. **`packaging/windows/msix.ps1`** and `AppxManifest.xml` as the design says; a workflow artifact only, skipped while
   the repository variables for the identity are unset. ⚠ The release workflow's artifacts are uploaded by
   `upload-artifact@v7` now — keep the same zipped default.
3. ⚠⚠ **The shared-catalog proof, before anything else is built on it:** install the installer build, make a catalog
   and change a setting; install the Store build sideloaded (signed with a throwaway self-made certificate, never
   committed); it must open the SAME catalog and settings; uninstall it, and they must still be there. Find the
   mechanism the Store accepts (flexible virtualization's excluded directories, or `unvirtualizedResources`); if none
   is accepted, **stop and ask the author**.
4. **`PRIVACY.md`** drafted (what Rustroom sends where: Immich, the tile server, Nominatim, the model downloads, the
   update check; it collects nothing itself) — ⚠ shown to the author and committed only once he approves the text.
5. For the author: the first-submission checklist in `CONTRIBUTING.md`, and a draft `runFullTrust` justification.

## Goal 3 — winget and Chocolatey

- Chocolatey: `packaging/chocolatey/`; the installer (`packaging/windows/rustroom.iss`, now `PrivilegesRequired=lowest`)
  gains `PrivilegesRequiredOverridesAllowed=commandline` so Chocolatey's `/ALLUSERS` installs into Program Files —
  ⚠ check that a double-clicked installer still asks no admin and still finds an earlier per-user install (⚠⚠ the
  `AppId` never changes).
- winget: the first submission is the author's (`wingetcreate new`, id `wladiKrin.Rustroom`, `inno`, user scope).
- Both from a new workflow on `release: published`; the secrets (a winget-pkgs token, the Chocolatey API key) are set
  **by the author**. ⚠ **Nothing is submitted or pushed to a registry during development** — `choco pack` and a local
  `choco install`, `winget validate` and a local manifest install only. A winget or Chocolatey install runs the same
  `setup.exe`, so the update notice shows there — by design (the spec's call).

## How the work runs

- Brainstorm is done (the design); each goal goes plan → executing-plans, **inline**, on its own branch. Never push
  unless the author says so; commits carry no `Co-Authored-By` trailer.
- The author closes a goal by trying it ("works, merge"): merge `--no-ff`, delete the branch, then the journal,
  `CLAUDE.local.md` and `docs/BACKLOG.md` (piece 4's line).
- Never reintroduce into the public repository any of the private strings the history was cleaned of (the list is in
  `CLAUDE.local.md`) — scan the diff for them before every push.

## The PC's limits (unchanged)

No page file: `CARGO_BUILD_JOBS=4` (2 for test builds) and the app closed during a gate; `VCPKG_ROOT` set
(`CONTRIBUTING.md`).
