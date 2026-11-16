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
`rust-toolchain.toml`), a C++ compiler, and four system libraries: LittleCMS (colour), libheif (HEIC
photos), libgphoto2 (tethering) and libraw.

**macOS** (with [Homebrew](https://brew.sh)):

```bash
xcode-select --install
brew install libraw little-cms2 libheif libgphoto2 pkgconf
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Put the `export` line in your shell profile, or repeat it in each new terminal.

**Linux — Debian / Ubuntu:**

```bash
sudo apt-get install -y build-essential libraw-dev liblcms2-dev libgphoto2-dev libheif-dev libheif-plugin-libde265 pkg-config
cargo run --release -p app
```

⚠ Ubuntu ships the HEVC decoder for HEIC photos as a separate package,
`libheif-plugin-libde265`; without it HEIC files fail to open.

**Linux — Arch:**

```bash
sudo pacman -S base-devel libraw lcms2 libheif libgphoto2 pkgconf
cargo run --release -p app
```

**Windows:** <!-- fill at M11.2: the Windows build steps -->

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
`{ "exposure": 1.2, "contrast": 25, "shadows": 40, "white_balance": { "temp": 20 } }`.

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

These three must pass — CI runs them on every pull request:

```bash
cargo fmt --all -- --check
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

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
