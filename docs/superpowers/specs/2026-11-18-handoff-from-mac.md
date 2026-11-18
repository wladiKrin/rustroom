# Hand-off from the Mac (2026-11-18)

> Written at the close of the Mac part, for the next session on the Arch machine. **Read this first, then the brief of
> piece 3c** (`2026-11-17-v0.1-packaging-linux-brief.md`, beside this file — it now carries what 3b settled). The rules
> are `AGENTS.md` and that machine's own `CLAUDE.local.md`, which is STALE until step 2 below. The Windows hand-off
> (`2026-11-18-handoff-from-windows.md`) still holds for what it lists as Arch's; this one adds what the Mac changed.

## Where things stand

- **`main` is pushed** (2026-11-18, at the author's word) and CI is green on it (Linux and Windows jobs). Everything done
  on the Mac, none of it yet built on Arch:
  - **The Metal NaN fix** — the quantize shaders test a float's bits for NaN (`is_nan`); `v != v` had been folded away
    by Metal's fast math and a NaN pixel came out green (`grep -n 'NaN pixel came out green' docs/JOURNAL.md`).
  - **`v0.1` piece 3b, the Mac app** — `packaging/macos/build.sh` makes `Rustroom-0.1.0-macos-arm64.dmg`: macOS 15 and
    later, Apple Silicon, bundle id `io.github.wladikrin.Rustroom` (fixed forever — 3c uses it too), its Homebrew
    libraries and 21 of libgphoto2's drivers inside, ad hoc signed; `rustroom_app::app_bundle` points libgphoto2 and
    libheif at the bundle's folders (macOS only today). Confirmed by the author: installed from the `.dmg`, everything
    tried works, tethering with his X-T4 (the bundle's drivers in use, `lsof`), and a downloaded copy's first launch
    (the README says how: Done → Privacy & Security → Open Anyway → an administrator's password). Then `build.sh`
    hardened (four of the review's minors). `grep -n 'piece 3b' docs/JOURNAL.md`.
  - **The release workflow's first run** (manual): `windows` (53 min cold) and `macos` (10 min, the floor measured at
    15.0 on CI) green, `release` skipped as designed.
  - **Developer ID and notarisation stay piece 4** — the author's call; `docs/BACKLOG.md` says what it needs.
- **The gate on the Mac:** fmt, clippy, **4,779 passed**, 0 failed, 32 ignored, no GPU test skipped (Metal).

## On Arch, before anything else

1. `git pull`, then the RESUME check from `CLAUDE.local.md`.
2. **Bring `CLAUDE.local.md` up to date** — the Mac's copy is the newest; the author can copy it over. Otherwise its
   *Next action* gains: the Metal fix ✅, piece 3b ✅ (merged and confirmed, tethering and a downloaded first launch
   seen), `build.sh` hardened ✅, the release workflow's first run ✅; **next: 3c on Arch, then the `v0.1.0` tag.**
3. **Run the whole gate FIRST.** ⚠ The NaN fix changed WGSL that has run on Metal only — CI has no GPU — so Arch's
   Vulkan is its first real run elsewhere: the GPU suites must RUN (no `SKIPPED`). The display-colour code from Windows
   has still never been built on Linux by a person (CI only). On Linux `gpu_pipeline`'s suite can stall inside a full
   run — `AGENTS.md`'s gate section says what to do.
4. Check CI once (`gh run list --limit 5`).

## Then: piece 3c, the AppImage

From its brief. What the Mac settled is in the brief's own section: the app id, `build.sh` as the model (the library
walk, the four non-camera drivers left out, a licence map that fails on an unknown library, the outside-path check, a
measured floor — on Linux the glibc version), and `app_bundle`, which an AppImage needs too (from `$APPDIR`).
Also on Arch, only Linux can show them: `cargo run --release --example display_probe`, Settings → Display →
Automatic's caption, and Wayland (`2026-11-18-handoff-from-windows.md`).

## After 3c

Tag `v0.1.0` — the release workflow builds all three packages into a **draft** release (the tag must equal the app's
version); the author publishes it. Then M11.5 plugins; piece 4 and *Going public* after (`docs/BACKLOG.md`).

## Still unverified (with where it is recorded)

- Linux display detection and two monitors on any system (`grep -n "display's own colours" docs/JOURNAL.md`).
- Windows' colour management switched while Rustroom runs; a narrower-than-sRGB panel's gamut edge (the GPU entry).
- The Mac app on a machine without Homebrew — the floor and the bundle's drivers are measured, not run there.

## First message to paste into the Arch session

> Read `docs/superpowers/specs/2026-11-18-handoff-from-mac.md` and `AGENTS.md`. Do its "before anything else" steps —
> update `CLAUDE.local.md`, run the whole gate, check CI — then start piece 3c from its brief.
