# The "new version available" notice — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rustroom tells the user, once, quietly, when a newer release exists on GitHub — piece 4's goal 1.

**Architecture:** a lib module `rustroom_app::update_check` (no egui) holds every decision — the version comparison,
whether a check is due, whether the line shows, reading GitHub's reply, whether this build checks at all — and a
small background checker with the HTTP call handed in. The bin starts it at launch, pumps its answer each frame into
`Settings`, draws one line in the status bar and one switch in Settings → Behaviour.

**Tech stack:** Rust, `ureq` (already a dependency), `serde_json`, egui 0.35.

**Spec:** `docs/superpowers/specs/2026-11-23-v0.1-piece4-updates-and-stores-design.md`, goal 1. Branch
`update-notice` (the spec is its first commit).

## Global constraints

- On from the first launch; Settings → Behaviour's **"Check for new versions"** turns it off.
- Asks only `GET https://api.github.com/repos/wladiKrin/rustroom/releases/latest`, at startup, at most once a day.
- Offline, a timeout, an error status or an unparsable reply: **silent** — no dialog, no status message.
- Compares versions as numbers, never strings (`0.10.0` > `0.9.0`).
- Downloads and installs nothing; a click opens the release page in the browser.
- A dismissed version stays quiet until a newer one exists.
- Built with `RUSTROOM_NO_UPDATE_CHECK` set (any value), the check is compiled off and the switch hidden.
- Silent in a Flatpak (`/.flatpak-info` exists or `FLATPAK_ID` is set). ⚠ The Store's detection (package identity)
  is **goal 2's**, built and tested on the Windows PC — not here; the hand-off says so.
- `docs/DESIGN.md` for anything drawn: `tokens::CAPTION` text, no literal sizes or colours, one-sentence hovers with no
  full stop, no glyph outside the bundled fonts' cmaps (✕ is in none — use a word).
- New `Settings` fields appended at the end of the struct and of `Default` (`AGENTS.md`: append, never insert).
- Tests never touch the network. `cargo test --workspace <filter>` — filter by test name, never `-p`.
- Commits: no `Co-Authored-By` trailer; author email the no-reply one already set in this clone. Never push.

## Review focus

1. **A reply that is not a version** (`tag_name` "nightly", "v1.2", "", a missing field, HTML from a captive portal):
   no line, no panic, the last-check time still recorded so it is not retried every launch — Task 1's
   `an_unreadable_reply_is_no_version`, Task 3's `a_failed_check_still_counts_as_a_check`.
2. **The clock** — a last-check time in the future (clock moved back, a copied settings file): treated as due, never
   "not for years" — Task 1's `a_check_from_the_future_is_due`.
3. **Running a NEWER build than the latest release** (a development build at 0.2.0-dev against release 0.1.0): no line
   — Task 1's `an_older_release_shows_nothing`.
4. **The switch turned off while a version is already known**: the line goes at once, and no check starts at the next
   launch — Task 4's `turning_the_check_off_hides_the_line`, Task 3's `no_check_when_switched_off`.
5. **A slow network at startup**: the window never waits for the check — the request is on its own thread with a
   10 s timeout, and the pump only `try_recv`s — Task 3's `the_pump_never_blocks`.

---

### Task 1: `update_check`'s decisions

**Files:**
- Create: `crates/app/src/update_check.rs` (tests in a `#[cfg(test)] mod tests` at its foot)
- Modify: `crates/app/src/lib.rs` (append `pub mod update_check;`)

**Interfaces — produces:**
- `pub struct Version(pub u32, pub u32, pub u32)` — `Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord`;
  `impl Display` as `0.2.0`.
- `pub fn parse_version(s: &str) -> Option<Version>` — accepts `0.2.0` and `v0.2.0`, exactly three dot-separated
  integers, nothing after them (`0.2.0-dev` → `None`; the running version is read through `current()`, below, which
  strips a `-…` suffix first).
- `pub fn current() -> Version` — `CARGO_PKG_VERSION` with any `-suffix` removed; panics only if Cargo.toml's
  version is malformed (a test pins that it parses).
- `pub const DAY_SECS: u64 = 86_400;`
- `pub fn is_due(last_checked: u64, now: u64) -> bool` — `last_checked == 0` (never), or `now < last_checked`
  (future), or `now - last_checked >= DAY_SECS`.
- `pub fn shown(current: Version, latest: Option<&str>, dismissed: Option<&str>) -> Option<Version>` — the latest
  parsed, if `> current` and not `<=` the dismissed one parsed.
- `pub fn latest_from_reply(json: &str) -> Option<Version>` — `serde_json` to a struct with only `tag_name: String`,
  then `parse_version`.
- `pub fn release_page(v: Version) -> String` — `https://github.com/wladiKrin/rustroom/releases/tag/v{v}` (derived,
  not the reply's `html_url`: no URL from the network is ever opened).
- `pub const COMPILED_IN: bool = option_env!("RUSTROOM_NO_UPDATE_CHECK").is_none();`
- `pub fn updated_elsewhere() -> bool` — a Flatpak: `Path::new("/.flatpak-info").exists() ||
  env::var_os("FLATPAK_ID").is_some()`, `cfg(target_os = "linux")` only, `false` elsewhere (goal 2 adds the Store).
- `pub fn may_check(switch_on: bool) -> bool` — `COMPILED_IN && switch_on && !updated_elsewhere()`.

- [ ] **Step 1: Write the failing tests** — in `update_check.rs`'s `tests`:
  - `versions_compare_as_numbers`: `parse_version("0.10.0") > parse_version("0.9.0")`; `parse_version("v1.2.3") ==
    Some(Version(1, 2, 3))`.
  - `an_unreadable_reply_is_no_version`: `latest_from_reply` is `None` for `{"tag_name":"nightly"}`,
    `{"tag_name":"v1.2"}`, `{"tag_name":""}`, `{}`, `<html>`, and `Some(Version(0, 2, 0))` for
    `{"tag_name":"v0.2.0","html_url":"x"}` (assert the `Some` first — a parser that finds nothing passes the rest).
  - `the_running_version_parses`: `current()` equals `parse_version(env!("CARGO_PKG_VERSION").split('-').next())`.
  - `a_check_is_due_once_a_day`: `is_due(0, 5)`; `!is_due(1000, 1000 + DAY_SECS - 1)`; `is_due(1000, 1000 + DAY_SECS)`.
  - `a_check_from_the_future_is_due`: `is_due(2_000, 1_000)`.
  - `a_newer_release_shows`: `shown(Version(0,1,0), Some("v0.2.0"), None) == Some(Version(0,2,0))`.
  - `an_older_release_shows_nothing`: `shown(Version(0,2,0), Some("v0.1.0"), None)` and `Some("v0.2.0")` → `None`.
  - `a_dismissed_version_stays_quiet_until_a_newer_one`: `shown(v010, Some("v0.2.0"), Some("v0.2.0")) == None`;
    `shown(v010, Some("v0.3.0"), Some("v0.2.0")) == Some(Version(0,3,0))`.
  - `the_release_page_is_derived`: `release_page(Version(0,2,0)) ==
    "https://github.com/wladiKrin/rustroom/releases/tag/v0.2.0"`.
- [ ] **Step 2:** `cargo test --workspace update_check` — fails to compile (module missing).
- [ ] **Step 3:** implement the interfaces above; module doc in the house style (what it decides, the ⚠ that nothing
  from the reply but a version is ever used).
- [ ] **Step 4:** `cargo test --workspace update_check` — all pass; the count is 9 (a filter matching nothing also
  exits 0).
- [ ] **Step 5:** commit `update_check: the decisions behind the new-version notice`.

### Task 2: the settings

**Files:**
- Modify: `crates/app/src/settings/mod.rs` — four fields appended to `Settings` and its `Default`
- Test: `crates/app/src/settings/compat_tests.rs`

**Interfaces — produces** (all under the container-level `#[serde(default)]`, no field-level one — the file's rule):
- `pub check_updates: bool` — default `true`.
- `pub update_checked_at: u64` — Unix seconds of the last finished check, `0` = never.
- `pub update_latest: Option<String>` — the newest version a check found, as `0.2.0`, so a second launch within the
  day still shows it.
- `pub update_dismissed: Option<String>` — the version the user hid.

- [ ] **Step 1: Write the failing test** `a_settings_file_from_before_the_update_check_still_loads`: parse
  `{"pan_drag":"Secondary","erase_drag":"Middle"}`; assert `s.erase_drag == EraseDrag::Middle` (it parsed, the
  existing tests' guard), `s.check_updates`, `s.update_checked_at == 0`, `s.update_latest.is_none()`,
  `s.update_dismissed.is_none()`.
- [ ] **Step 2:** `cargo test --workspace update_check_still_loads` — fails to compile.
- [ ] **Step 3:** add the fields, each with a doc comment (the `0` = never rule; why the latest is kept).
- [ ] **Step 4:** `cargo test --workspace settings` — passes, including the exhaustive round-trip tests.
- [ ] **Step 5:** commit `settings: the update check's switch, last time, latest and dismissed version`.

### Task 3: the background check and its pump

**Files:**
- Modify: `crates/app/src/update_check.rs` — the checker (lib)
- Create: `crates/app/src/update_flow.rs` (bin; declare `mod update_flow;` in `main.rs`, alphabetical)
- Modify: `crates/app/src/app.rs` (one field), `crates/app/src/bootstrap.rs` (start it),
  `crates/app/src/shell/mod.rs` (`self.pump_update_check();` in `pump_jobs`, last)

**Interfaces:**
- Consumes: Task 1's `is_due`, `may_check`, `latest_from_reply`; Task 2's fields.
- Produces (lib):
  - `pub type Fetch = Box<dyn FnOnce() -> Result<String, String> + Send>;`
  - `pub fn live_fetch() -> Fetch` — ureq, `place_search::USER_AGENT`, header `Accept: application/vnd.github+json`,
    `timeout_global` 10 s, body limit 1 MiB, to the URL in *Global constraints* (`tile_fetch::live_fetch` is the shape).
  - `pub enum Outcome { Found(Version), Nothing }` — `Nothing` for any failure.
  - `pub struct Checker { rx: Receiver<Outcome> }`; `pub fn Checker::start(fetch: Fetch, on_done: Box<dyn FnOnce()
    + Send>) -> Checker` (spawns a named thread `update-check`; `on_done` repaints); `pub fn Checker::poll(&self) ->
    Option<Outcome>` (`try_recv`).
  - `pub fn start_if_due(s: &Settings, now: u64, fetch: impl FnOnce() -> Fetch, on_done: …) -> Option<Checker>` —
    `None` unless `may_check(s.check_updates) && is_due(s.update_checked_at, now)`; `fetch` is a factory so no agent
    is built when no check runs.
  - `pub fn record(s: &mut Settings, outcome: &Outcome, now: u64)` — sets `update_checked_at = now` for BOTH outcomes;
    `Found(v)` also sets `update_latest = Some(v.to_string())`; `Nothing` leaves the previous latest.
- Produces (bin): `RustroomApp.update_checker: Option<update_check::Checker>`;
  `fn pump_update_check(&mut self)` — on `Some(outcome)`: `record`, `save_settings()`, drop the checker.

- [ ] **Step 1: Write the failing tests** (lib, a fake `Fetch` — never the network):
  - `no_check_when_switched_off`: `start_if_due` with `check_updates: false` → `None`, and the factory was not called
    (a `Cell<bool>` it sets).
  - `no_check_within_a_day`: `update_checked_at = now - 60` → `None`.
  - `a_check_finds_the_release`: a fetch returning `{"tag_name":"v9.0.0"}`; poll until `Some` under the 120 s ceiling
    (`AGENTS.md`: wait on the event, never a fixed budget); `record` sets `update_latest == Some("9.0.0")` and the time.
  - `a_failed_check_still_counts_as_a_check`: a fetch returning `Err("offline")` → `Outcome::Nothing`; after `record`,
    `update_checked_at == now` and a previous `update_latest` is kept.
  - `the_pump_never_blocks`: a fetch that blocks on a channel the test holds; `poll()` returns `None` at once; release
    it, then the outcome arrives.
- [ ] **Step 2:** `cargo test --workspace update_check` — the new tests fail to compile.
- [ ] **Step 3:** implement the lib half; then the bin half — in `bootstrap.rs` after the settings load,
  `update_checker: update_check::start_if_due(&settings, now, update_check::live_fetch, repaint)` with `now` from
  `SystemTime::now().duration_since(UNIX_EPOCH)` (a clock before 1970 → `0`, which is "due"); `repaint` clones
  `cc.egui_ctx` and calls `request_repaint`.
- [ ] **Step 4:** `cargo test --workspace update_check` — 15 pass (Task 2's compat test matches the filter too). ⚠ Then confirm the test app never starts a real
  check: `test_app`'s construction goes through bootstrap or not — read it; if it does, it must use
  `SettingsStore::new(path)` settings with `check_updates: false` or an injected fetch, and say which in the commit.
- [ ] **Step 5:** commit `update_check: ask GitHub once a day in the background, record the answer`.

### Task 4: the status line and the switch

**Files:**
- Modify: `crates/app/src/status_bar.rs` — `job_or_status`'s idle branch
- Modify: `crates/app/src/modals/settings.rs` — the draft field, `apply`, the Behaviour checkbox
- Tests: `status_bar.rs`'s and `modals/settings.rs`'s `tests`

⚠ Measure both files with `code_and_doc_lines` first (set `test-dir`'s `CEILING` to 600, run
`cargo test --workspace no_file_is_over`, read the list, set it back). If `modals/settings.rs` would pass 1,000, move
the Behaviour tab's body into `crates/app/src/modals/settings_behaviour.rs` first, in its own commit, then add.

**Interfaces — consumes:** `update_check::{shown, current, release_page, COMPILED_IN, may_check}`; Task 2's fields.

- [ ] **Step 1: Write the failing tests:**
  - status bar `a_newer_version_shows_in_the_status_line`: settings with `update_latest: Some("99.0.0")`,
    `check_updates: true`; draw the Library row; the label "Rustroom 99.0.0 is available" is on screen (assert it is
    found before anything else).
  - `clicking_it_opens_the_release_page`: click the label; the frame's output carries an open-URL command whose URL is
    `release_page(Version(99, 0, 0))` (egui 0.35's `PlatformOutput::commands` / `OutputCommand::OpenUrl` — check the
    exact name in its source).
  - `hide_dismisses_this_version`: click "Hide"; `settings.update_dismissed == Some("99.0.0")`, saved
    (`settings_store.load()`), and the label is gone on the next frame.
  - `turning_the_check_off_hides_the_line`: `check_updates: false` with the same latest → no label.
  - settings `behaviour_has_the_update_check_switch`: open Settings, click "Check for new versions", Apply →
    `!app.settings.check_updates`, saved. (Skip, with the reason in its doc, when `!COMPILED_IN` — a test that cannot
    fail is not a test, so assert `COMPILED_IN` in a separate `the_test_build_has_the_check` first.)
- [ ] **Step 2:** `cargo test --workspace update_check status_bar behaviour_has` — the new ones fail.
- [ ] **Step 3:** implement:
  - In `job_or_status`, only in the final `else` (no job running): when `may_check(check_updates)` and
    `shown(current(), latest, dismissed)` is `Some(v)`, draw before the status text a link-styled label
    "Rustroom {v} is available" (`tokens::CAPTION`, the theme's accent — `theme::tokens`, never a literal colour),
    hover "Opens the release page in your browser"; a click calls `ui.ctx().open_url(OpenUrl::new_tab(release_page(v)))`.
    Then a small "Hide" button, hover "Hides this version until a newer one comes out"; a click sets
    `update_dismissed` and calls `save_settings()`. Running jobs win, as every line in that function does.
  - Settings: `check_updates: bool` in `SettingsDraft`, `from_settings`, `apply` (the `reopen_last` shape, `plain =
    true`); in tab 0, beside "Map tiles" (the other choice about going online), drawn only when `COMPILED_IN`:
    `ui.checkbox(&mut d.check_updates, "Check for new versions").hover("Asks GitHub once a day whether a newer Rustroom
    is out")`.
- [ ] **Step 4:** `cargo test --workspace update_check status_bar behaviour_has every_hover nothing_in_either` — pass,
  including the hover sweep and the overlap test.
- [ ] **Step 5:** commit `status bar: a newer version's line with Hide; Settings: Check for new versions`.

### Task 5: documents, the gate, the author's look, the hand-off

**Files:**
- Modify: `README.md` (one sentence in the privacy/online part, or beside the downloads: Rustroom checks GitHub once a
  day for a newer version, switch in Settings → Behaviour), `AGENTS.md` (one ⚠ line under *Warnings*: the update check
  is the one thing Rustroom contacts unasked, only through `update_check`, compiled off by `RUSTROOM_NO_UPDATE_CHECK`;
  keep the file under ~400 lines), `docs/BACKLOG.md` (piece 4's goal 1 ✅ when merged), `docs/JOURNAL.md` (a section,
  newest first, at the close)
- Create: `docs/superpowers/specs/2026-11-23-handoff-from-arch.md`

- [ ] **Step 1:** the gate — `cargo fmt --all -- --check`, `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`; each exit code read from its own command (the Linux stalls
  in `AGENTS.md`'s gate section apply).
- [ ] **Step 2:** the sabotage — temporarily make `shown` compare with `>=` and run `cargo test --workspace
  update_check`: `an_older_release_shows_nothing` must fail; restore.
- [ ] **Step 3:** launch for the author. First the real request: with `update_checked_at` set to 0 in his settings,
  `cargo run --release -p app` — the live release is 0.1.0, equal to the build, so no line shows, but
  `update_checked_at` becomes now (proof the request ran and was parsed). Then the line: back up his `settings.json`,
  set `"update_latest": "9.9.9"`, launch; he looks at the status line, clicks it (the browser opens a 404 for
  v9.9.9 — expected), presses Hide, restarts (gone), turns the switch off. Restore the backup afterwards. Finally
  `RUSTROOM_NO_UPDATE_CHECK=1 cargo build --release -p app`, launch, and confirm the switch is absent.
- [ ] **Step 4:** write the hand-off for goals 2 and 3 on the Windows PC: what this branch built; goal 2 must add the
  Store's detection to `update_check::updated_elsewhere` (`GetCurrentPackageFullName` ≠ `APPMODEL_ERROR_NO_PACKAGE`,
  through `windows-sys`, gated `cfg(windows)` at every reference) and its test; the shared-catalog proof; the
  `PRIVACY.md` draft for the author's approval; Chocolatey's `/ALLUSERS`; the secrets and variables the author sets;
  nothing published without him.
- [ ] **Step 5:** commit the documents; wait for the author's "works, merge" — then `--no-ff` merge, delete the branch,
  the journal, `CLAUDE.local.md`'s state line, the backlog. Never push unasked.
