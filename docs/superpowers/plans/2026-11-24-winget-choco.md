# winget and Chocolatey — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `winget install wladiKrin.Rustroom` and `choco install rustroom` install Rustroom, and each published release
reaches both registries without a hand on it — piece 4's goal 3.

**Architecture:** both run the release's own `setup.exe` from GitHub. Chocolatey's package (`packaging/chocolatey/`) is a
template filled per version by `pack.ps1`; winget's manifests are rendered by `packaging/winget/manifest.ps1` for local
validation and the author's first submission, and later releases are updated in place by `wingetcreate update`. A new
workflow on `release: published` does both, each skipped (saying so) until the author has set its secret.

**Spec:** `docs/superpowers/specs/2026-11-23-v0.1-piece4-updates-and-stores-design.md`, goal 3;
`docs/superpowers/specs/2026-11-23-handoff-from-arch.md`, *Goal 3*. Branch `winget-choco`, from `main` — it shares no
file with goal 2's `store-msix`, so either merges first. Its maintainer notes live beside the packages
(`packaging/{chocolatey,winget}/README.md`), not in `CONTRIBUTING.md`, which `store-msix` also changes; one line there
points at them once both are merged.

## Facts checked (2026-11-24)

- Ids free: `winget search` finds no `wladiKrin.Rustroom` nor any "Rustroom"; `choco search rustroom --exact` finds
  nothing (both searches found Inno Setup, the positive control).
- Inno Setup: `/ALLUSERS` "only has an effect when" `PrivilegesRequiredOverridesAllowed` allows the command line; with
  `PrivilegesRequired=lowest` and no switch, Setup stays in non-administrative mode and asks for nothing; `{autopf}` is
  `{commonpf}` (Program Files) in administrative mode, `{userpf}` otherwise. `UsePreviousPrivileges` is "effectively
  ignored" unless the DIALOG override is allowed — so with `commandline` only, a double-clicked installer on a machine
  with a Chocolatey install installs a second, per-user copy rather than upgrading Program Files' (builder's call to
  accept: Chocolatey users upgrade with `choco upgrade`; allowing the dialog would put a choice in every installer).
- Chocolatey's validator: description ≥ 30 characters, `projectUrl`, `licenseUrl`, space-separated `tags` are
  requirements; `summary`, `iconUrl`, `packageSourceUrl` guidelines; a downloaded file must be checksummed; an icon
  from a CDN pinned to a tag (jsDelivr), never raw GitHub; no `VERIFICATION.txt` when no binary is in the package.
- winget: `winget validate --manifest <dir>` exists; the documentation's examples use manifest schema 1.12.0 (the
  repository also holds 1.28.0, "not all fields supported") — 1.12.0 here, `wingetcreate` writes its own later.

## Global constraints

- ⚠⚠ **Nothing is submitted or pushed to a registry during development** — `choco pack`, a local `choco install` from
  the packed folder, `winget validate`; the author makes the first real submissions.
- ⚠⚠ The installer's `AppId` never changes; a double-clicked installer still installs per user with no admin prompt
  and still upgrades an earlier per-user install.
- winget stays user scope (`Scope: user`); Chocolatey installs for all users (`/ALLUSERS`, it runs elevated, often as
  another account). The catalog and settings stay per user either way.
- The secrets (`WINGET_TOKEN`, `CHOCOLATEY_API_KEY`) are set by the author; a job without its secret says so and
  succeeds. Prereleases and drafts publish nothing.
- ⚠ **The first Chocolatey push waits for a release built with this branch's installer** — `v0.1.0`'s installer has no
  command-line override, so `/ALLUSERS` does nothing there and it would install into the elevated account's profile.
  winget can start at `v0.1.0`.
- No `Co-Authored-By` trailer; never push.

## Task 1 — the installer lets Chocolatey install for all users

**Files:** `packaging/windows/rustroom.iss`.

- [ ] `PrivilegesRequiredOverridesAllowed=commandline` beside `PrivilegesRequired=lowest`, with why (the comment block).
- [ ] Locally: a silent run with no switch from a non-elevated shell installs per user, no prompt, over the existing
      per-user install (one HKCU uninstall entry, its version); with `/ALLUSERS` from an elevated one, into Program
      Files (an HKLM entry); uninstall it again. (Asks the author first — it replaces his installed copy.)

## Task 2 — the Chocolatey package

**Files:** `packaging/chocolatey/rustroom.nuspec`, `tools/chocolateyinstall.ps1`, `tools/chocolateyuninstall.ps1`,
`pack.ps1`, `README.md`.

- [ ] The nuspec (`@VERSION@`): id `rustroom`, title, authors, `projectUrl`, `packageSourceUrl`, `licenseUrl`,
      `iconUrl` (jsDelivr, pinned to the tag), `docsUrl`, `bugTrackerUrl`, `releaseNotes` (the release page), tags,
      summary, a description from the README.
- [ ] Install: `Install-ChocolateyPackage` with the release's URL and SHA-256 (`@URL@`, `@CHECKSUM@`) and
      `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /ALLUSERS`. Uninstall: the HKLM entry's uninstaller, silently.
- [ ] `pack.ps1 -Version x.y.z [-Checksum <sha256>] [-Url <url>]`: without a checksum, reads it from the release's
      `SHA256SUMS`; `-Url` (a local `file:///` installer) only for a local test; writes `target\chocolatey\`.
- [ ] Locally (elevated, asks the author): `choco install rustroom --source target\chocolatey` with this branch's
      installer → Program Files, Start menu for all users; `choco uninstall rustroom` → gone; the catalog untouched.

## Task 3 — winget's manifests

**Files:** `packaging/winget/*.yaml` (templates), `packaging/winget/manifest.ps1`, `README.md`.

- [ ] The three manifests (version, installer — `inno`, `user`, x64, `ProductCode` `{AppId}_is1`, the URL and SHA-256 —
      and `en-US` locale), rendered for a version into `target\winget\<version>`.
- [ ] `winget validate --manifest target\winget\0.1.0` passes on the published `v0.1.0`.

## Task 4 — the publish workflow

**Files:** `.github/workflows/publish.yml`.

- [ ] On `release: published` (not a prerelease): the version from the tag; the installer downloaded and its SHA-256
      checked against the release's `SHA256SUMS`.
- [ ] winget: `wingetcreate update wladiKrin.Rustroom --version … --urls … --submit` with `WINGET_TOKEN`.
- [ ] Chocolatey: `pack.ps1`, then `choco push` with `CHOCOLATEY_API_KEY`.

## Task 5 — close

- [ ] The gate on the tip; the journal; hand to the author. README's Windows section gains `winget install` and
      `choco install` only once each is listed — not on this branch.
