# Browse Polish (filmstrip feedback fixes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three issues found while testing the filmstrip: `Open file…` should leave single-file mode (no stale strip); RAW + non-RAW files with the same basename must not share an edit sidecar; and the filmstrip should resize (thumbnails scale with the panel height).

**Architecture:** All changes are in `crates/app/src/main.rs`. (1) `open_dialog` clears `self.library`. (2) a `sidecar_path` helper names sidecars by *appending* `.json` to the full filename instead of replacing the extension. (3) the filmstrip cell size is derived from the panel's available height, and the panel is resizable.

**Tech Stack:** existing `app` / egui 0.35.

> **Deferred (chosen):** #2 — background-thread decode + instant thumbnail placeholder so RAW switching feels snappy (the ~1 s freeze is the half-size RAW decode on the UI thread). Separate follow-up.

---

## Task 1: `Open file…` leaves folder/single-file cleanly

**Files:** Modify `crates/app/src/main.rs`.

- [ ] **Step 1: Clear the library when opening a standalone file**

In `open_dialog`, after the successful pick + open, drop any loaded folder so no stale filmstrip/grid remains:
```rust
        self.open_path(ctx, path);
        self.library = None;
        self.view = AppView::Develop;
```
(`open_path` already persisted the outgoing photo's sidecar before this. With `library == None`, `develop_body` shows no strip, the Filmstrip toggle is hidden, and arrow-nav is inert — true single-file mode.)

- [ ] **Step 2: Build**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build -p app`
Expected: builds.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
git add crates/app && git commit -m "fix(app): Open file… clears the loaded folder (single-file mode)"
```

---

## Task 2: unique sidecar per source file (append `.json`)

**Files:** Modify `crates/app/src/main.rs`.

- [ ] **Step 1: Add a `sidecar_path` helper**

Add near the other free functions (e.g. after `decode_for_export`):
```rust
/// The edit sidecar path for a source file: the full filename with `.json` appended
/// (e.g. `photo.RAF` → `photo.RAF.json`), so a RAW and a JPEG that share a basename get
/// distinct sidecars.
fn sidecar_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".json");
    std::path::PathBuf::from(s)
}
```

- [ ] **Step 2: Use it everywhere a sidecar is read or written**

Replace the three `.with_extension("json")` call sites:
- In `open_path` (sidecar load):
```rust
        let sidecar = sidecar_path(&path);
```
- In `persist_current_session`:
```rust
                let _ = s.edits.to_json_file(&sidecar_path(&s.path));
```
- In the `export_dialog` spawned closure (after `save_jpeg`):
```rust
                edits.to_json_file(&sidecar_path(&out))?;
```

- [ ] **Step 3: Build**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build -p app`
Expected: builds. (`photo.RAF` → `photo.RAF.json`, `photo.jpg` → `photo.jpg.json` — no longer collide.)

- [ ] **Step 4: Commit**

```bash
cargo fmt --all && cargo clippy -p app --all-targets -- -D warnings
git add crates/app && git commit -m "fix(app): sidecar name appends .json so RAW+JPEG basenames don't collide"
```

---

## Task 3: resizable filmstrip with height-scaled thumbnails

**Files:** Modify `crates/app/src/main.rs`.

- [ ] **Step 1: Make the panel resizable and size cells from its height**

In `filmstrip`, replace the `const CELL` + fixed `box_size` with a panel-height-derived size, and make the panel resizable with a sensible initial height:
```rust
    /// Bottom strip of the loaded folder's thumbnails; click a cell to open that photo.
    /// The panel is resizable (drag its top border); thumbnails scale with its height.
    fn filmstrip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut open_index: Option<usize> = None;
        egui::Panel::bottom("filmstrip")
            .resizable(true)
            .default_size(96.0)
            .show(ui, |ui| {
                let Some(lib) = &self.library else { return };
                let cell_h = (ui.available_height() - 8.0).clamp(48.0, 400.0);
                let box_size = egui::vec2(cell_h * 1.5, cell_h);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for i in 0..lib.entries.len() {
                            let resp = ui
                                .allocate_ui(box_size, |ui| match &lib.thumbs[i] {
                                    Some(tex) => {
                                        ui.add(
                                            egui::Image::new(tex)
                                                .max_size(box_size)
                                                .maintain_aspect_ratio(true),
                                        );
                                    }
                                    None => {
                                        let (rect, _) =
                                            ui.allocate_exact_size(box_size, egui::Sense::hover());
                                        ui.painter().rect_filled(
                                            rect,
                                            egui::CornerRadius::same(3),
                                            egui::Color32::from_gray(60),
                                        );
                                    }
                                })
                                .response
                                .interact(egui::Sense::click());
                            if lib.selected == Some(i) {
                                ui.painter().rect_stroke(
                                    resp.rect,
                                    egui::CornerRadius::same(3),
                                    egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 160, 240)),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if resp.clicked() {
                                open_index = Some(i);
                            }
                        }
                    });
                });
            });
        if let Some(i) = open_index {
            self.open_entry(ctx, i);
        }
    }
```

- [ ] **Step 2: Build + manual check**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build -p app` then `cargo run --release -p app`: open a folder → Develop → drag the filmstrip's **top border** up/down; the thumbnails grow/shrink with the panel height. Adjust any 0.35 API mismatch (`Panel::resizable`).

- [ ] **Step 3: Commit**

```bash
cargo fmt --all && cargo clippy -p app --all-targets -- -D warnings
git add crates/app && git commit -m "feat(app): resizable filmstrip — thumbnails scale with panel height"
```

---

## Task 4: wrap-up

- [ ] **Step 1: Full workspace check**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```
Expected: all green.

- [ ] **Step 2: Manual acceptance**

(1) Open a folder, then `Open file…` a standalone image → no filmstrip, Library grid empty. (2) In a folder with `x.RAF` and `x.jpg`, edit one, open the other → edits do **not** transfer (`x.RAF.json` vs `x.jpg.json`). (3) Drag the filmstrip border → thumbnails resize. Clean close.

- [ ] **Step 3: Update `CLAUDE.md`** — note the three fixes; keep #2 (threaded snappy decode) listed as the open follow-up. Commit `docs: record browse-polish fixes`.

---

## Self-Review notes (for the executor)

- **Coverage:** clear-library-on-Open-file (Task 1), unique sidecar via append (Task 2), resizable height-scaled filmstrip (Task 3). #2 deferred by choice.
- **Type consistency:** `sidecar_path(&Path) -> PathBuf` (Task 2) replaces all three `with_extension("json")` uses (open_path load, persist save, export save). `filmstrip` keeps the same shape, only cell sizing + `resizable` change.
- **Sidecar migration note:** existing `<name>.json` sidecars (old scheme) won't be found under the new `<name.ext>.json` scheme — fine for early dev (edits re-load from the new name going forward). Worth a one-line mention if a user has old sidecars.
- **No new tests:** all three are view/wiring changes; the underlying `to_json_file`/`open_entry` are already unit-tested. Verified by build + clippy + manual smoke.
- **Done:** `cargo test --all`/clippy/fmt green; the three behaviors fixed; clean close.
```
