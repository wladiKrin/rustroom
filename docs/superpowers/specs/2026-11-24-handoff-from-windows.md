# Hand-off from the Windows PC (2026-11-24) — piece 4: the stores, from any machine

> Written at the close of piece 4's goals 2 and 3 on the Windows PC, for the next session on the Mac or the Arch
> machine. **Read this first**, then the design (`2026-11-23-v0.1-piece4-updates-and-stores-design.md`, beside this
> file) only where a step below points at it. The rules are `AGENTS.md` and that machine's own `CLAUDE.local.md` —
> STALE until step 2 below.

## Before anything else

1. **Pull `main`** — it carries everything below and this file.
2. **Copy `CLAUDE.local.md` from the Windows PC by hand** (untracked, private): its state line was updated there on
   2026-11-24. Keep the receiving machine's own lines (build environment, paths) where they differ.
3. **Check the commit address:** `git config user.email` must be the GitHub no-reply one, never the author's real
   address (it was unset in the Windows clone until this session).

## Where things stand (`main` pushed 2026-11-24 at the author's word)

- **Piece 4 goals 1–3 are merged** (`grep -n 'piece 4 goal' docs/JOURNAL.md`): the update notice; the **Microsoft
  Store package** (`packaging/windows/msix.ps1`, `AppxManifest.xml`, the logos in `packaging/icons/msix/`; the release
  workflow's `windows-store` artifact); **winget** (`packaging/winget/`) and **Chocolatey** (`packaging/chocolatey/`),
  sent each published release by `.github/workflows/publish.yml`. With them: no test writes into the user's own
  thumbnails or previews any more (`grep -n "user's own thumbnails" docs/JOURNAL.md`).
- **`PRIVACY.md` is committed** — the author approved its text on 2026-11-24. Its GitHub URL is the Store's privacy
  policy.
- ⚠⚠ **The Store package asks for `unvirtualizedResources`** — the author's call, so that it shares the installer's
  catalog and settings (proved on the PC: same catalog, new files in the real folders, data kept on uninstall). It needs
  **Windows 11**. Microsoft lists the capability for certain games and developers report refusals: **if certification
  refuses it, the question goes back to the author** — move the data out of AppData for every build (~28 `dirs::`
  call sites and a migration), or park the Store. Never fall back to a package with its own catalog.
- **The gate on Windows** at the pushed code (`0f7c7324`): fmt, clippy, **4,816 passed**, 0 failed, 32 ignored.

## What is left, who does it, and where

| Step | Who | Machine |
|---|---|---|
| Partner Center: register (identity check), reserve **Rustroom**, read *Product identity* | the author | any — web |
| Set the repository **variables** `MSIX_IDENTITY_NAME`, `MSIX_PUBLISHER`, `MSIX_PUBLISHER_DISPLAY_NAME` | the author (or the builder in the browser, with his yes) | any — web |
| The Store listing (description, features, search terms; the README's five 1920×1178 PNGs qualify), the age-rating answers | the builder drafts, the author enters | any |
| Chocolatey: an account, its API key as the **secret** `CHOCOLATEY_API_KEY` | the author | any — web |
| winget's first listing (`packaging/winget/README.md`), then the **secret** `WINGET_TOKEN` | the author | easiest on **Windows** (`wingetcreate submit`); elsewhere the three files rendered from the templates go in by a pull request to `microsoft/winget-pkgs` |
| **`v0.1.1`**: the version in `crates/app/Cargo.toml` (the tag must equal it) and a `<release>` in `packaging/linux/io.github.wladikrin.Rustroom.metainfo.xml`; release notes; the gate | the builder | any |
| The tag, then publishing the drafted release (which starts `publish.yml`) | the author | any |
| Upload `windows-store`'s `.msix` to Partner Center; the two capability reasons (drafts in `CONTRIBUTING.md` §6) | the author | any — web |
| *Optional:* the App Certification Kit (`appcert.exe`) on an `msix.ps1 -Test` package before the first submission | the builder, with admin prompts | **Windows** |

- ⚠ **Chocolatey's first package is `v0.1.1`'s** — `v0.1.0`'s installer predates `/ALLUSERS` (`publish.yml` skips it).
  winget can list `v0.1.0` now.
- ⚠ **Seen for the first time on GitHub at `v0.1.1`:** `publish.yml` (tested locally only — its checksum step dry-run,
  the packages installed by hand); `msix.ps1` under PowerShell 7 (the runner's shell; tested under 5.1); the release
  job's three named `download-artifact@v8` steps and its no-`.msix` check. Watch those runs.
- A secret (a token, an API key) is entered by the author only — never by an assistant.

## What the Windows PC holds, for whenever it is used again

- A copy of the author's Rustroom data from before the Store proof: `%USERPROFILE%\rustroom-proof-backup-2026-11-24`.
  Delete it once nothing seems missing.
- Its per-user Rustroom is a development build of 0.1.0 (the installer test), not the release.
- The side session's worktree `.claude/worktrees/vigorous-wozniak-c7f404` holds branch `fix-test-library-folder` —
  ⚠ its own journal commit carries a home path that was rewritten out of `main` before the push: **never push that
  branch**; archive the session, then `git branch -D fix-test-library-folder`.
- `VCPKG_ROOT` is not a user variable there — set it per shell (`CONTRIBUTING.md`).

## Before every push

Scan the outgoing commits **one by one** (`git show` each, `origin/main..main`) for the private strings with **fixed**
strings (`grep -F`), never only the net diff: a home path added by one commit and removed by a later one is still
published by the first (2026-11-24 — `grep -n 'The push: no home path' docs/JOURNAL.md`, the unpushed chain was rebuilt).
