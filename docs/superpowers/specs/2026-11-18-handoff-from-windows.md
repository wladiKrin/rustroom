# Hand-off from the Windows machine (2026-11-18)

> Written at the close of the Windows part, for the next session on the Mac or the Arch machine. **Read this first,
> then the brief of the piece you start** (`2026-11-17-v0.1-packaging-mac-brief.md` or `…-linux-brief.md`, beside
> this file). The rules are `AGENTS.md` and that machine's own `CLAUDE.local.md` — which is STALE until step 2 below.

## Where things stand

- **`main` is pushed** (2026-11-18, by the author's word — the push that carries this file). It holds everything done
  on Windows, none of which the Mac or Arch has built yet:
  - `v0.1` **piece 2** — Rustroom builds, passes the gate and runs on Windows (`grep -n 'v0.1. piece 2' docs/JOURNAL.md`).
  - **piece 3a** — the aperture icon, the per-user Windows installer, the release workflow (`grep -n 'piece 3a'`).
  - **The display's own colours** on Windows and Linux — Settings → Display → **Automatic**, from the monitor's EDID
    or ICC profile (`grep -n "display's own colours"`), and **their GPU transform**: a monitor profile keeps the fast
    paths, a Straighten drag on the author's M27Q 149.5 → 24.7 ms (`grep -n 'colours on the GPU'`). On the way the
    colour table became **root-spaced** (a gamma-2.2 profile was 4.66 steps off near black).
  - **No test writes into the user's own cache** — `rustroom_app::user_cache::base` (`grep -n "user's own cache"`).
- **The gate on Windows:** fmt, clippy, **4,773 passed**, 0 failed, 32 ignored, no GPU test skipped (`e4b3b994`'s code;
  the commits after it are docs and one comment).
- **The Windows part is closed.** Seen by the author there: the build, the installer, the colours, the in-window menu bar
  (clicked through). Not seeable there: two monitors (no second one).

## On the other machine, before anything else

1. `git pull`, then the RESUME check from `CLAUDE.local.md`.
2. **Bring `CLAUDE.local.md` up to date** (private, never committed). The Windows copy is the newest — the author can
   copy it over; otherwise change its *Next action* block to say: `v0.1` pieces 2 and 3a ✅ (merged 2026-11-17/18,
   confirmed by the author); the display's own colours and their GPU transform ✅ (2026-11-18, confirmed; on Windows the
   GPU path itself was seen only through the probe and the tests — the author's session had Windows' own colour
   management on); no test writes into the user's cache ✅; the in-window menu bar ✅ seen on Windows; **next: 3b on the
   Mac, 3c on Arch**. Also add the Windows run line (*Run the GUI*): `VCPKG_ROOT`, no page file, `CARGO_BUILD_JOBS=2`
   for test builds, the app closed during a gate.
3. **Run the whole gate FIRST** — before starting a piece. Shared code changed in ways only Windows has compiled:
   `color::Display` replaced `OutputSpace` on the whole display path (render engine, loaders, settings, sessions);
   `rustroom_app::display_detect` (its Linux branch reads `/sys/class/drm` — compiled on Linux only, never yet);
   `gpu_pipeline::quantize`'s second plane; `user_cache`. ⚠⚠ **The macOS-only modules (`native_menu`,
   `display_colorspace`, `AGENTS.md`'s first warning) have not been compiled since 2026-11-16** — read clippy on the
   Mac before anything else. Another system counts a few tests differently (some are `cfg`'d): read the number, never
   compare it blindly with 4,773.
4. **Check CI once** (`gh run list --limit 5`): this push is the first run of CI's **Windows job** and the first CI build
   of the Linux display code. Then the **release workflow**, never run yet: start it by hand from the Actions tab — a
   manual run builds the installer without releasing (`.github/workflows/release.yml`).

## What is left, by machine

| Where | What |
|---|---|
| **The Mac** | **piece 3b**, the `.app` and `.dmg` — its brief. ⚠ Mark the **vault** there (`03-plan-of-action.md`, M11.4 and M11.2): pieces 2 and 3a ✅, Windows ✅ (builds, gate, runs, menu bar seen), the display work ✅. On macOS the display is unchanged by design: macOS matches colours itself, Settings says "handled by macOS". |
| **Arch** | **piece 3c**, the AppImage — its brief. And three things only Linux can show: `cargo run --release --example display_probe` (what Automatic finds through sysfs — never run anywhere), Settings → Display → Automatic's caption, and Wayland. ⚠ On KDE or GNOME with a colour profile applied the desktop may convert itself — then sRGB is the right choice; say so if Automatic converts twice. |
| **Any, after 3b and 3c** | tag `v0.1` (the release workflow builds the Windows installer into a draft release); piece 4 (signing, stores) after going public; M11.5 plugins; *Going public* in `docs/BACKLOG.md`. |

## Still unverified (with where it is recorded)

- Linux display detection, and two monitors on any system (`grep -n "display's own colours" docs/JOURNAL.md`).
- Windows' colour management switched while Rustroom runs (detected at startup — seen; the switch — not).
- CI's Windows job and the release workflow (step 4 above).
- A narrower-than-sRGB panel with a gamma curve stays 10–18 steps off at its gamut's edge — a known limit, recorded in
  the GPU entry, not chased.

## If you come back to the Windows PC

`VCPKG_ROOT=%USERPROFILE%\vcpkg` (the static triplet, `x64-windows-static`), LLVM and the MSVC Build Tools installed;
⚠ **no page file** — `CARGO_BUILD_JOBS=2` for test builds, 4 for others, Rustroom closed during a gate (OOM shows as
LNK1102/LNK1201 or a failed allocation). `C:\cache` (empty folders a test made before the fix) is the author's to delete.

## First message to paste into the next session

> Read `docs/superpowers/specs/2026-11-18-handoff-from-windows.md` and `AGENTS.md`. Do its four "before anything else"
> steps — update `CLAUDE.local.md`, run the whole gate, check CI — then start the piece for this machine from its brief.
