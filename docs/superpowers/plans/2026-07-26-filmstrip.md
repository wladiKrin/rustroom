# Filmstrip in Develop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A bottom filmstrip in the Develop view that renders the loaded folder's thumbnails; click a cell or press ←/→ to switch photos (edits auto-saved), with a show/hide toggle.

**Architecture:** Pure `crates/app/src/main.rs` change reusing existing `Library` state (`entries`, `thumbs`, `selected`) and `open_entry` (which already persists the current photo's sidecar and decodes the next). A `show_filmstrip` flag + a bottom `egui::Panel` + arrow-key navigation gated on no focused widget.

**Tech Stack:** `eframe`/`egui` 0.35 (`Panel::bottom`, `ScrollArea::horizontal`, `ctx.memory(|m| m.focused())`).

---

## Spec reference

Implements `docs/superpowers/specs/2026-07-26-filmstrip-design.md`. App-only; no new crates. Deferred: vertical orientation, drag-reorder, single-file filmstrip, rating badges.

## File Structure

Only `crates/app/src/main.rs`:
- `RustroomApp`: `show_filmstrip: bool` field (default `true` → manual `Default`).
- `open_entry`: set `library.selected` so grid + strip stay in sync.
- `develop_toolbar`: a **Filmstrip** toggle (shown when a folder is loaded).
- `develop_body`: gains a `ctx` param; calls arrow-nav + the filmstrip panel.
- New methods: `filmstrip(ui, ctx)`, `handle_develop_arrows(ctx)`.

---

## Task 1: state, selection sync, toolbar toggle

**Files:** Modify `crates/app/src/main.rs`.

- [ ] **Step 1: Add the `show_filmstrip` field with a manual `Default`**

Replace the `#[derive(Default)] struct RustroomApp { … }` block:
```rust
struct RustroomApp {
    view: AppView,
    library: Option<Library>,
    session: Option<Session>,
    status: String,
    show_filmstrip: bool,
}

impl Default for RustroomApp {
    fn default() -> Self {
        Self {
            view: AppView::default(),
            library: None,
            session: None,
            status: String::new(),
            show_filmstrip: true,
        }
    }
}
```

- [ ] **Step 2: Sync `library.selected` in `open_entry`**

Replace `open_entry` so opening (grid double-click, filmstrip click, or arrow) also sets the selection:
```rust
    fn open_entry(&mut self, ctx: &egui::Context, index: usize) {
        let path = match self.library.as_mut() {
            Some(lib) => match lib.entries.get(index) {
                Some(e) => {
                    lib.selected = Some(index);
                    e.path.clone()
                }
                None => return,
            },
            None => return,
        };
        self.open_path(ctx, path);
        self.view = AppView::Develop;
    }
```

- [ ] **Step 3: Add the Filmstrip toggle to `develop_toolbar`**

At the end of `develop_toolbar` (after the `if let Some(s) = &mut self.session { … }` block), add:
```rust
        if self.library.is_some() {
            ui.toggle_value(&mut self.show_filmstrip, "Filmstrip");
        }
```

- [ ] **Step 4: Build**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build -p app`
Expected: builds (toggle has no visible effect yet — the panel comes in Task 2).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/app && git commit -m "feat(app): filmstrip state + selection sync + toolbar toggle"
```

---

## Task 2: the filmstrip bottom panel

**Files:** Modify `crates/app/src/main.rs`.

- [ ] **Step 1: Give `develop_body` a `ctx` param and call the filmstrip**

Change the `ui` dispatcher's Develop arm to pass `ctx`:
```rust
        match self.view {
            AppView::Develop => self.develop_body(ui, &ctx),
            AppView::Library => self.library_view(ui, &ctx),
        }
```
Change `develop_body`'s signature and insert the filmstrip panel **between** the right panel and the central panel:
```rust
    fn develop_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::right("panel")
            .default_size(240.0)
            .show(ui, |ui| {
                // …existing right-panel body, unchanged…
            });

        if self.show_filmstrip && self.library.is_some() {
            self.filmstrip(ui, ctx);
        }

        egui::CentralPanel::default().show(ui, |ui| match &mut self.session {
            // …existing central-panel body, unchanged…
        });
    }
```
(Only the signature and the two-line filmstrip insert change; the panel bodies are exactly as they are today. `ctx` is currently unused elsewhere in `develop_body`, so no unused-var warning arises once `filmstrip` uses it.)

- [ ] **Step 2: Add the `filmstrip` method**

```rust
    /// Bottom strip of the loaded folder's thumbnails; click a cell to open that photo.
    fn filmstrip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        const CELL: f32 = 96.0;
        let mut open_index: Option<usize> = None;
        egui::Panel::bottom("filmstrip")
            .default_size(CELL * 0.66 + 22.0)
            .show(ui, |ui| {
                let Some(lib) = &self.library else { return };
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for i in 0..lib.entries.len() {
                            let box_size = egui::vec2(CELL, CELL * 0.66);
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

- [ ] **Step 3: Build + manual check**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH" && cargo build -p app` then `cargo run --release -p app`: open a folder, double-click a photo → in Develop a bottom strip shows the folder's thumbnails with the current one outlined; click another strip cell → it opens (previous photo's edits saved); the **Filmstrip** toggle hides/shows the strip. Adjust any 0.35 API mismatch (`Panel::bottom` / `default_size`).

- [ ] **Step 4: Commit**

```bash
cargo fmt --all && cargo clippy -p app --all-targets -- -D warnings
git add crates/app && git commit -m "feat(app): bottom filmstrip in Develop (click to switch photo)"
```

---

## Task 3: arrow-key photo navigation

**Files:** Modify `crates/app/src/main.rs`.

- [ ] **Step 1: Add `handle_develop_arrows` and call it in `develop_body`**

Add the call as the first line of `develop_body`:
```rust
    fn develop_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.handle_develop_arrows(ctx);
        egui::Panel::right("panel")
        // …rest unchanged…
```
Add the method:
```rust
    /// ←/→ move to the previous/next photo in the loaded folder — but only when no egui
    /// widget has keyboard focus, so arrows still fine-tune a focused slider.
    fn handle_develop_arrows(&mut self, ctx: &egui::Context) {
        let nav = {
            let Some(lib) = &self.library else { return };
            let n = lib.entries.len();
            if n == 0 || ctx.memory(|m| m.focused().is_some()) {
                return;
            }
            let cur = lib.selected.unwrap_or(0);
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    Some((cur + 1).min(n - 1))
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    Some(cur.saturating_sub(1))
                } else {
                    None
                }
            })
        };
        if let Some(i) = nav {
            if self.library.as_ref().and_then(|l| l.selected) != Some(i) {
                self.open_entry(ctx, i);
            }
        }
    }
```

- [ ] **Step 2: Build + manual check**

Run: `cargo run --release -p app`: open a folder, edit a photo, press **→** and **←** — the editor moves to the next/previous photo (edits saved per photo, strip highlight follows). With a slider focused (click into its value), arrows adjust the slider instead. Adjust any 0.35 API mismatch (`memory`/`focused`).

- [ ] **Step 3: Commit**

```bash
cargo fmt --all && cargo clippy -p app --all-targets -- -D warnings
git add crates/app && git commit -m "feat(app): arrow-key photo navigation in Develop"
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
Expected: all green (no new tests — this is view code reusing tested machinery).

- [ ] **Step 2: Manual acceptance**

Open a folder → edit a photo → click through the filmstrip and arrow between photos (edits persist per photo) → grid and strip selection stay in sync → toggle the strip on/off → single-file open (Open file…) shows no strip. Clean close.

- [ ] **Step 3: Update `CLAUDE.md`** — note the filmstrip (Develop bottom strip reusing `Library` state; click + ←/→ nav via `open_entry`; `show_filmstrip` toggle; arrow-nav gated on `ctx.memory().focused()`); mark progress "toward C". Update the "Next step" (rounded-out develop, saturation first). Commit `docs: record filmstrip`.

---

## Self-Review notes (for the executor)

- **Spec coverage:** `show_filmstrip` + toggle (Task 1/2), bottom panel reusing `Library` (Task 2), click-to-switch via `open_entry` (Task 2), ←/→ nav gated on focus (Task 3), grid/strip selection sync via `open_entry` (Task 1). No-folder → no strip (guard in Task 2). Covered.
- **Type consistency:** `open_entry(&self, ctx, index)` now sets `library.selected` and is the single open path used by grid double-click, filmstrip click, and arrows. `develop_body(ui, ctx)` signature change is applied at its one call site (Task 2 Step 1). `filmstrip`/`handle_develop_arrows` both take `ctx`.
- **Borrow notes:** `filmstrip` and `handle_develop_arrows` compute an `open_index`/`nav` under an immutable `&self.library` borrow, then call `self.open_entry` after that borrow ends — no aliasing.
- **No new tests:** reuses `ThumbLoader`, `open_entry`, sidecar persistence, `Library` — all already unit-tested; the new code is thin egui view + a trivial clamp. Verified by build + clippy + manual smoke, per prior GUI milestones.
- **Done:** `cargo test --all`/clippy/fmt green; filmstrip + arrow-nav work; selection stays in sync; toggle works; single-file unaffected; clean close.
```
