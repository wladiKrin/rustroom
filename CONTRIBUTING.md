# Contributing to Rustroom

Rustroom is made by photographers, for photographers. You do not need to be a Rust expert to improve
it: many changes here start with someone who knows exactly what a tool should do in the darkroom and
works it out with the code — on their own, or with an AI coding assistant. This guide is about the
part that matters most either way: **showing that your change works.**

## 1. Before you start

- **Have an idea or a question?** Start a thread in
  [Discussions](https://github.com/wladiKrin/rustroom/discussions) — *Ideas* for a feature you would
  like, *Q&A* for anything else. Once an idea is agreed, it becomes an issue.
- **Found a bug?** [Open an issue](https://github.com/wladiKrin/rustroom/issues/new/choose); the form
  asks for what helps: your OS, the camera or file type, what you did and what you saw.
- **Check [`docs/BACKLOG.md`](docs/BACKLOG.md) first.** It lists what is planned and, more
  importantly, what was tried and **declined with measurements** — so you can see why before
  proposing it again.
- **Read [`ARCHITECTURE.md`](ARCHITECTURE.md)** for how a photo travels through Rustroom and which
  steps a typical change takes.

## 2. Build and run

You need [Rust](https://rustup.rs) (stable — `rustup` picks the right version from
`rust-toolchain.toml`), a C++ compiler with libclang (two dependencies generate their bindings with
it), and three system libraries: LittleCMS (colour), libheif (HEIC photos) and libgphoto2
(tethering). LibRaw comes compiled in.

**macOS** (with [Homebrew](https://brew.sh)):

```bash
xcode-select --install
brew install little-cms2 libheif libgphoto2 pkgconf
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Put the `export` line in your shell profile, or repeat it in each new terminal.

**The Mac app** (`target/macos/Rustroom-<version>-macos-arm64.dmg`) is built by one script, the same one the release
workflow runs. Its minimum macOS is that of your Homebrew's libraries — Homebrew builds for the macOS it runs on — so
the release is built on macOS 15:

```bash
cargo install cargo-about --locked --features cli
packaging/macos/build.sh
```

**Linux — Debian / Ubuntu:**

```bash
sudo apt-get install -y build-essential libclang-dev liblcms2-dev libgphoto2-dev libheif-dev libheif-plugin-libde265 pkg-config
cargo run --release -p app
```

⚠ Ubuntu ships the HEVC decoder for HEIC photos as a separate package,
`libheif-plugin-libde265`; without it HEIC files fail to open.

**Linux — Arch:**

```bash
sudo pacman -S base-devel clang lcms2 libheif libgphoto2 pkgconf
cargo run --release -p app
```

**The AppImage** (`target/linux/Rustroom-<version>-linux-x86_64.AppImage` and its `.zsync`) is built by one script,
`packaging/linux/build.sh`, the same one the release workflow runs — on Ubuntu 22.04, whose glibc sets the oldest Linux
it runs on. On any other Linux, run it inside an Ubuntu 22.04 container (Docker, the packages from
`packaging/linux/deps.txt`):

```bash
packaging/linux/in-docker.sh --floor 2.35
```

The AppImage carries its own libgphoto2 and drivers, LittleCMS and a libheif compiled in with its HEVC decoder (the
`appimage` feature — Ubuntu 22.04's libheif is too old), and never the graphics stack: a library the script has not
been told to bundle or to leave to the system stops the build.

**Windows** (in PowerShell, with [winget](https://learn.microsoft.com/windows/package-manager/winget/)):

```powershell
winget install Rustlang.Rustup LLVM.LLVM Kitware.CMake
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
git clone https://github.com/microsoft/vcpkg "$env:USERPROFILE\vcpkg"
& "$env:USERPROFILE\vcpkg\bootstrap-vcpkg.bat" -disableMetrics
& "$env:USERPROFILE\vcpkg\vcpkg.exe" install "libheif[core,libde265]:x64-windows-static"
[Environment]::SetEnvironmentVariable("VCPKG_ROOT", "$env:USERPROFILE\vcpkg", "User")
```

Then, in a new terminal (so it sees `cargo` and `VCPKG_ROOT`), `cargo run --release -p app`.

- **libheif comes from vcpkg** — on Windows the build reads it nowhere else. `[core,libde265]` is the HEIC
  *decoder* only, and `x64-windows-static` matches the C++ runtime the workspace links into the exe
  (`.cargo/config.toml`), so the app needs no DLL beside it — not even Microsoft's runtime. ⚠ A `RUSTFLAGS`
  environment variable replaces that setting, and the build then asks vcpkg for a triplet you did not install. LittleCMS needs nothing:
  its crate compiles a bundled copy. LibRaw is compiled in, as everywhere, from `vendor/rsraw-sys` (patched for
  MSVC — see its `VENDORED.md`). LLVM is for bindgen, which finds it without `LIBCLANG_PATH`.
- **Tethered capture is not available on Windows** (libgphoto2 has no Windows build here), and **printing
  saves a PDF** — there is no CUPS.
- **Rustroom is a GUI program on Windows** — no console window. Its debug output (`RUSTROOM_DEBUG_*`) reaches
  the terminal `cargo run` was started from, or a file with `cargo run --release -p app 2> render.log`; a
  shell's `Rustroom.exe > log` captures nothing.
- ⚠ **A build that dies with `LNK1102`, `LNK1201`, `os error 1455` ("the paging file is too small") or `STATUS_STACK_BUFFER_OVERRUN`
  is out of memory**, not broken code: a machine with no page file can commit only its RAM. Turn on a
  system-managed page file, or build with fewer jobs (`$env:CARGO_BUILD_JOBS = "4"`).
- **The installer** (`target\installer\Rustroom-<version>-windows-x64-setup.exe`) is built by one script, the same
  one the release workflow runs:

  ```powershell
  winget install JRSoftware.InnoSetup --scope user
  cargo install cargo-about --locked --features cli
  powershell -ExecutionPolicy Bypass -File packaging\windows\build.ps1
  ```

  The Microsoft Store's package is made from that same build by `packaging\windows\msix.ps1` (see
  [Releasing](#6-releasing)); `msix.ps1 -Test` makes one with a fake identity, signed to install on your own PC.

**Always use `--release` for real use.** RAW decoding, previews and thumbnails are many times faster
than in a debug build.

### The command line

Next to the app there is `rustroom-cli`, handy for testing a change without the window:

```bash
# Decode a RAW to a quick TIFF
cargo run --release -p rustroom -- decode photo.RAF out.tiff

# Develop a RAW with an edits file and export a JPEG
cargo run --release -p rustroom -- render photo.RAF out.jpg --edits edits.json
```

An `edits.json` is the same list of settings Rustroom saves beside every photo, for example
`{ "exposure": 1.2, "contrast": 25, "shadows": 40, "white_balance": { "kelvin": 5500 } }`.

## 3. Working with an AI assistant

Rustroom is set up for it. [`AGENTS.md`](AGENTS.md) holds the rules every change must follow — where
code goes, how to run the checks, and the expensive lessons this project has learned. Claude Code
reads it automatically (through `CLAUDE.md`); point any other assistant at it first.

Then **ask for evidence, not a summary**. An assistant's report of "done, all tests pass" is a claim;
these are evidence:

- a test that **fails without** the change and passes with it — ask it to show you the failure;
- for anything you can see, a **screenshot or a before/after** from the running app;
- the checks below, run on the final commit.

The habit that has caught the most mistakes here is the *sabotage*: break the thing on purpose and
watch the test go red. A test that stays green while the feature is broken is not testing it.

## 4. The checks

These three must pass. CI runs them on every pull request, and also runs clippy with
`--all-features` (the test-only features included), so run that too before you send a change that
touches a feature-gated file:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

In PowerShell the clippy line is `$env:CARGO_TARGET_DIR = "target/clippy"; cargo clippy --workspace --all-targets -- -D warnings`
(and `Remove-Item Env:CARGO_TARGET_DIR` before the tests).

- **Run one test or a group by name**, never by package: `cargo test --workspace tone_curve`.
  Switching between `--workspace` and `-p <crate>` rebuilds everything each time.
- **Golden images** pin what the pipeline renders: `cargo test --workspace --test golden`. When a
  change is *meant* to alter the picture, regenerate the affected references with `UPDATE_GOLDEN=1`
  and look at every changed image before committing it.
- **Tests that need a real RAW file** skip unless you point them at one:
  `RAW_FIXTURE=/path/to/photo.RAF cargo test --workspace`. Camera files are not committed.
- **No source file over 1,000 lines** (code and doc comments) — a test enforces it. Split by
  responsibility, not to dodge the number.
- **Anything a user can see follows [`docs/DESIGN.md`](docs/DESIGN.md)** — text sizes, colours,
  controls, and hover text of one short sentence.

## 5. Sending it

- **Small pull requests**, one change each. A pull request that does one thing gets reviewed sooner.
- **Show it:** a screenshot or a before/after for anything visible, and the command that proves the
  rest.
- **Say what you did not check** — a platform you could not test, a camera you do not own.
- **Licence:** Rustroom is GPL-3.0-or-later, and your contribution is under the same licence. There is
  no separate agreement to sign.

## 6. Releasing

For the maintainer. A tag `v<version>`, equal to the app crate's version, runs the release workflow: it builds the
Windows installer, the Mac disk image and the AppImage, each with its own script under `packaging/`, and drafts a
GitHub release with them and `SHA256SUMS`. The draft is published by hand.

### The Microsoft Store

The Store's package is an artifact of the same run, `windows-store`, never a release asset: the Store signs it, and
only Partner Center takes it. ⚠ It declares the restricted capability `unvirtualizedResources`, so that it shares the
installer's catalog and settings (`packaging/windows/AppxManifest.xml` says why); Microsoft meant that capability for
certain games, and certification may refuse it.

The first submission, once:

1. Register a free individual developer account in Partner Center (it checks your identity).
2. Reserve the name **Rustroom** (Apps and games → New product → MSIX or PWA app).
3. From the product's **Product identity** page, set three repository variables (Settings → Secrets and variables →
   Actions → Variables): `MSIX_IDENTITY_NAME` (Package/Identity/Name), `MSIX_PUBLISHER` (Package/Identity/Publisher,
   `CN=…`) and `MSIX_PUBLISHER_DISPLAY_NAME` (Package/Properties/PublisherDisplayName). Until they are set, the
   workflow says so and makes no package.
4. Run the release workflow (the tag, or Actions → Release → Run workflow) and download its `windows-store` artifact.
5. In the submission: **Packages** — the `.msix`; **Pricing** — free; **Properties** — Photo & video, the privacy
   policy `https://github.com/wladiKrin/rustroom/blob/main/PRIVACY.md`; **Age ratings** — the questionnaire;
   **Store listing** — the description and screenshots; **Submission options** — the two restricted capabilities,
   each with its reason (drafts below, to edit).
6. Every later release: that run's `windows-store` artifact, uploaded as a new submission.

Draft reasons for the restricted capabilities:

- **runFullTrust** — Rustroom is a desktop photo editor, also distributed as an ordinary Windows installer. It opens
  RAW photos from any folder, memory card or network drive the user picks, writes each photo's edits and metadata in
  files beside it, exports to folders the user chooses and drives the GPU for its editing pipeline — the access of
  a classic desktop app. It is open source (GPL-3.0-or-later): https://github.com/wladiKrin/rustroom
- **unvirtualizedResources** — Rustroom keeps the user's catalog (a database of their photo library with every edit,
  rating, keyword and collection) and its previews in `%LOCALAPPDATA%\rustroom`, and its settings and presets in
  `%APPDATA%\rustroom`. The same app is also distributed as an ordinary installer, and users move between the
  two. Virtualized, the Store version would start with an empty catalog, and uninstalling it would delete the
  user's catalog and years of edits with it. The package excludes exactly those two folders from virtualization
  (flexible virtualization's `ExcludedDirectories`) and nothing else.
