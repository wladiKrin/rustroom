# The Microsoft Store — an MSIX package — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rustroom ships a Store package (MSIX) that Windows updates, opening the SAME catalog and settings as the
installer build — piece 4's goal 2.

**Architecture:** the exe is the installer's own (`build.ps1`'s stage folder), wrapped by `packaging/windows/msix.ps1`
in a full-trust MSIX whose manifest takes Rustroom's two AppData folders out of the file-system virtualization. At run
time the package identity (`GetCurrentPackageFullName`) joins the Flatpak in `update_check::updated_elsewhere`, so the
notice never shows in the Store build. The release workflow keeps the package as an artifact only.

**Spec:** `docs/superpowers/specs/2026-11-23-v0.1-piece4-updates-and-stores-design.md`, goal 2;
`docs/superpowers/specs/2026-11-23-handoff-from-arch.md`, *Goal 2*. Branch `store-msix`.

## The data decision (author, 2026-11-24)

An MSIX app's NEW files under `%APPDATA%` / `%LOCALAPPDATA%` go to a per-package folder that an uninstall deletes
(Microsoft Learn, *Understanding how packaged desktop apps run on Windows*, 1903 and later); only the restricted
capability `unvirtualizedResources` lets a package write the real folders. Microsoft's capability list says it is meant
for certain desktop games from Microsoft and its partners, Store submission needs its approval, and developers report
refusals (policy 10.6.3). Asked whether to park the Store, move the data out of AppData, or try the capability anyway,
**the author chose to try the capability anyway.** Builder's calls inside that:

- **Flexible virtualization, not virtualization switched off:** `virtualization:FileSystemWriteVirtualization` with
  `ExcludedDirectories` for exactly `$(KnownFolder:RoamingAppData)\rustroom` and `$(KnownFolder:LocalAppData)\rustroom`,
  so the justification can say "only our own two folders". It needs **Windows 11** (Microsoft Learn, *Flexible
  virtualization*: older systems ignore the new declarations, which would give the package its own catalog), so
  `MinVersion` is `10.0.22000.0` — ⚠ **this excludes Windows 10** (out of support since 2025-10-14); the installer,
  winget and Chocolatey still serve it.
- If certification refuses the capability, the question comes back to the author — never a silent fallback to a
  package with its own catalog (the design's anti-goal).

## Global constraints

- The package is never a release asset nor in `SHA256SUMS`; a workflow artifact only, skipped (and saying so) while
  the identity variables are unset.
- Identity values (`Identity Name`, `Publisher`, `PublisherDisplayName`) come only from Partner Center through
  repository variables — never invented. A local test package uses an obviously fake one (`CN=Rustroom Test`).
- A throwaway signing certificate lives in `target\` only, never committed; trusting it on this PC, Developer Mode,
  installing or uninstalling a package, and touching the author's real catalog need the author's yes first.
- `cfg(windows)` at EVERY reference to the Windows probe, its `use` lines included (`AGENTS.md`'s macOS-module warning,
  the same shape); `cargo clippy` on Linux must not see `windows-sys`.
- `windows-sys` 0.61 is already in `Cargo.lock` (no new crate compiled) — confirm with `cargo tree -i windows-sys@0.61.2`.
- `PRIVACY.md` is shown to the author and committed only once he approves its text.
- Tests never touch the network. `cargo test --workspace <filter>`, never `-p`. No `Co-Authored-By` trailer; never push.

## Review focus

1. **The probe's two answers.** `APPMODEL_ERROR_NO_PACKAGE` (15700) means unpackaged; ANY other result (success,
   `ERROR_INSUFFICIENT_BUFFER` 122 for a zero-length buffer) means packaged. The test asserts the raw code is exactly
   15700 in a test process — not merely "false", which any error would also give.
2. **The positive control for the notice.** 0.1.0 is the newest release, so a silent Store build proves nothing by
   itself: the proof writes `update_latest: "9.9.9"` and `update_checked_at: <now>` into the shared settings file — the
   installer build must then show "Rustroom 9.9.9 is available" and the Store build must not.
3. **The release job's download** takes the three named artifacts only (an allowlist) and fails if any `.msix` reached
   `dist` — `download-artifact@v8` with `merge-multiple` and no pattern takes every artifact of the run today.

## Task 1 — package identity silences the notice (and hides its switch)

**Files:** `crates/app/Cargo.toml`, `crates/app/src/update_check.rs`, `crates/app/src/modals/settings.rs`.

- [ ] `[target.'cfg(windows)'.dependencies] windows-sys = { version = "0.61", features = ["Win32_Foundation",
      "Win32_Storage_Packaging_Appx"] }`.
- [ ] `update_check`: a `#[cfg(windows)] fn package_status() -> u32` calling `GetCurrentPackageFullName` with a zero
      length and a null buffer, and `#[cfg(windows)] fn packaged() -> bool` (`!= APPMODEL_ERROR_NO_PACKAGE`); the
      probe inside `updated_elsewhere`'s `remembered` closure (never beside it — the status row asks every frame), the
      `not(linux)` arm narrowed to `not(any(linux, windows))`. The doc comment says what the Store build is.
- [ ] `update_check::switch_shown()` = `COMPILED_IN && !updated_elsewhere()` (builder's call: a switch that does
      nothing in the Store build or a Flatpak is not drawn), used by Settings → Behaviour and its test.
- [ ] Test (`cfg(windows)`): `a_test_process_has_no_package_identity` — `package_status() == 15700`.
- [ ] `cargo test --workspace update_check`, `cargo test --workspace behaviour_has_the_update_check_switch`; clippy.

## Task 2 — the Store's logos

**Files:** `packaging/icons/render.py`, `packaging/icons/msix/*.png` (committed, like the other rasters).

- [ ] Check Microsoft's current list of package logos (*App icons and logos*); render the required ones from
      `rustroom.svg` at their scales (Square44x44 with its target sizes and `altform-unplated`, Square150x150,
      StoreLogo), named with MRT qualifiers so `makepri` indexes them.
- [ ] `python packaging/icons/render.py` reproduces every committed raster byte for byte except the new ones.

## Task 3 — `AppxManifest.xml` and `msix.ps1`

**Files:** `packaging/windows/AppxManifest.xml` (a template with `@NAME@`-style placeholders — `$(KnownFolder:…)` must
survive PowerShell untouched), `packaging/windows/msix.ps1`.

- [ ] The manifest: `Windows.FullTrustApplication`, `runFullTrust` + `unvirtualizedResources`, the two excluded
      directories, `TargetDeviceFamily Windows.Desktop MinVersion="10.0.22000.0"`, version `<x.y.z>.0`, x64.
- [ ] `msix.ps1`: version from `cargo metadata` (as `build.ps1`); the stage folder `build.ps1` made (stop if absent);
      the identity from `RUSTROOM_MSIX_NAME`, `RUSTROOM_MSIX_PUBLISHER`, `RUSTROOM_MSIX_PUBLISHER_DISPLAY_NAME` — unset:
      say so and exit 0; `-Test` uses the fake identity and signs with a throwaway certificate made into `target\msix`;
      `makepri` + `makeappx` from the newest Windows SDK; writes `target\msix\Rustroom-<version>-windows-x64.msix`.
- [ ] `makeappx` accepts the manifest (its schema validation) and the package opens in `Get-AppxPackageManifest`.

## Task 4 — the release workflow

**Files:** `.github/workflows/release.yml`.

- [ ] The windows job: after the installer, "Build the Store package" with the three variables as environment (from
      `vars.`); an `upload-artifact@v7` step `windows-store`, `if-no-files-found: ignore` (unset variables → none).
- [ ] The release job's download: the three named artifacts only, and a step that fails if `dist` holds an `.msix`.

## Task 5 — the shared-catalog proof, on this PC (asks the author first)

- [ ] Back up `%APPDATA%\rustroom` and `%LOCALAPPDATA%\rustroom`; the installed `0.1.0` is the installer build.
- [ ] Sideload the `-Test` package (the certificate trusted, or Developer Mode — whichever the author allows).
- [ ] The installer build: change a setting; write the 9.9.9 positive control; see the line.
- [ ] The Store build (from Start): the same catalog, the changed setting, the Immich keys; **no** update line and no
      switch; change a setting there and see it in the installer build; `%LOCALAPPDATA%\Packages\<family>\LocalCache`
      holds nothing of Rustroom's after use.
- [ ] Uninstall the package: both folders still there, unchanged; remove the certificate; restore the settings.

## Task 6 — `PRIVACY.md` (the author approves the text before it is committed)

- [ ] What Rustroom sends where, each from the code (`grep` every request): Immich (the user's own server), the map's
      tile server, Nominatim on Enter, the model downloads, the update check — and that it collects nothing itself.

## Task 7 — the first submission, for the author

**Files:** `CONTRIBUTING.md` (its release section).

- [ ] A checklist: register, reserve the name, set the three repository variables, run the release workflow, download
      `windows-store`, upload, the listing, the age rating, the privacy URL, the restricted-capability justifications.
- [ ] Drafts of the `runFullTrust` and `unvirtualizedResources` justifications for him to edit.

## Task 8 — close

- [ ] The gate on the tip (fmt, clippy, test — each exit code on its own); the journal's section; hand to the author
      to try. On "works, merge": merge `--no-ff`, delete the branch, `CLAUDE.local.md`, `docs/BACKLOG.md`.

## The shared-catalog proof — run 2026-11-24 on the Windows PC (Windows 11 26H2, build 26300)

The installer's stage exe and the package's exe were byte-identical (this branch's release build, 0.1.0 + the probe);
the package `Rustroom.Test`, signed with the throwaway certificate, sideloaded with both restricted capabilities and
no approval. The author's data was backed up first and restored after, file for file.

- **Positive control:** `update_latest: "9.9.9"` written into the shared settings — the unpackaged exe showed
  "Rustroom 9.9.9 is available" with Hide and Don't show again; **the packaged exe, the same file, showed no line**:
  the identity probe answered "packaged" for real.
- **Same data:** the packaged exe opened the same catalog (495 photos, the same folders, people and selection) and
  reached the Immich server with the keys in Credential Manager.
- **New files** (what virtualization redirects — edits of existing files pass through regardless), made from inside the
  package (`Invoke-CommandInDesktopPackage`): one in `%APPDATA%\rustroom` and one in `%LOCALAPPDATA%\rustroom` landed
  in the REAL folders; the control, one in a non-excluded `%APPDATA%\rustroom-proof-control`, went to the package's
  `LocalCache\Roaming` — virtualization was on, and only the two folders were let through.
- **Uninstall:** the package and its private folder (the control with it) went; both real folders stayed, every file,
  `catalog.db` byte-identical to the backup.
- Not seen: Settings → Behaviour without its switch in the package (the same predicate as the silent line); the
  Store's own certification of `unvirtualizedResources`.
