# The canvas split: view code out of `main.rs`

**Status:** spec, 2026-08-13.

## Why now

The modal split took `main.rs` from 3,419 to 2,762 lines — but the modals were only 19% of
it, and the *canvas* is where **M3.1's masking UI lands**: overlays and hit-testing on the
image, a mask list in the right panel. This is the split that actually clears the way for
Phase 3, and it should happen before masking rather than during it.

`develop_body` alone is **380 lines**, the largest function in the crate, and it does two
unrelated jobs.

## Decisions taken

| Question | Decision |
|---|---|
| `develop_body` | **Split into panel + canvas**, not moved whole. |
| Module layout | **Flat siblings of `modals/`**, no `view/` wrapper. |
| Scope | View code only. The orchestration layer stays. |

### Why split rather than move

Moving `develop_body` intact would be the safest possible diff — a pure relocation the
compiler fully verifies. It was rejected because it relocates the problem: masking adds a mask
list to the panel *and* overlays plus hit-testing to the canvas, so the next person to touch it
inherits a 380-line function with masks bolted on.

Breaking the panel down *further* — into its collapsible tone/colour/detail/effects sections —
was also rejected. Those section shapes are about to change when masks arrive, and the
layout/IA pass is deliberately sequenced **after** M3.1. Restructuring them now means
restructuring them twice.

### Why no `view/` wrapper

`modals/` is already a flat sibling at `crates/app/src/`. Introducing `view/` would either
leave the tree inconsistent or require moving the modals inside it too — churn for no gain.
A single `view.rs` is out at ~1,100 lines.

## Design

### 1. Module layout

```
crates/app/src/
├── develop/
│   ├── mod.rs      body() orchestration, toolbar(), preset_panel()
│   ├── panel.rs    the right adjustments panel + the histogram
│   └── canvas.rs   the image canvas, pending placeholder, crop overlay
├── library.rs      the grid + grid_columns + its layout tests
├── filmstrip.rs    the strip
├── thumb.rs        thumb_cell, paint_selection_border + the 8 cell/layout tests
├── shell.rs        impl eframe::App — the frame loop and view dispatch
├── modals/         (unchanged)
└── theme.rs        gains caption()
```

Same conventions as the modal split, for the same reasons: **free functions taking
`&mut RustroomApp`** rather than an `impl` block in another file, and **binary-crate modules**
declared in `main.rs` rather than lib modules, because `lib.rs` is deliberately egui-free.

⚠ Both privacy directions apply again, and both bit during the modal split: descendants of the
crate root reach `RustroomApp`'s **private fields** with no `pub` anywhere, but `main.rs` is the
**ancestor** of these modules, so each submodule needs `pub mod`.

### 2. `thumb.rs` is the unit the tests already found

`thumb_cell` is shared by the Library grid *and* the filmstrip, and eight of `main.rs`'s eleven
tests cluster around it and grid layout — including the reproduction of the portrait-thumbnail
cell collapse and the two headless click tests. They move with it.

⚠ Those tests **reimplement the layout loop rather than driving the real one**, which is
recorded in `CLAUDE.md` and matters here: they pin the *rule*, not the call site. A pure move
cannot break them, so their passing is weak evidence about the move itself. Their value is that
they keep documenting the rules after the code that follows those rules has relocated.

The other three tests — `every_demosaic_tier_has_a_cost_hint`,
`the_default_tier_is_described_as_the_default`, `a_full_size_decode_is_twice_the_half_size_one`
— cover `view_quality_hint` and `decode_full_res`, both of which stay, so they stay too.

### 3. `photo_edited` becomes a return value

Today it is a local threaded through 380 lines, with a comment explaining that both edit sites
sit inside closures already holding `&mut self` through `self.session` and so cannot touch
another field of `self`.

Split, each half reports for itself:

```rust
/// Returns whether a photo edit settled into a new history entry.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui) -> bool;
```

`develop::body` ORs them and records one action-log entry. This is the shape
`draw_crop_overlay` already uses, so it is the codebase's existing answer to this problem
rather than a new one.

### 4. `theme::caption` arrives, with both call sites

Promised by the modal-split spec and deferred to here, because this is where its callers live:

```rust
/// A caption: a filename under a thumbnail, a camera model beside a title.
///
/// ⚠ Renders identically to [`hint`] today and means something different — hint is
/// explanatory text *under a control*. Kept separate so the visual identity pass can move one
/// without the other.
pub fn caption(ui: &mut egui::Ui, text: &str);
```

Two sites: the camera model in the develop panel, and the filename under a Library grid
thumbnail. Both land in this change, so there is no dead-code window — the trap the CLI's
`format_arg` and the modal split's `button_row` both hit.

⚠ The two `.small()` uses inside the test module are fixtures and stay as they are.

### 5. What does *not* move

The orchestration layer stays in `main.rs`: the state types (`RustroomApp`, `Session`,
`Library`, `PendingOpen`), the decode functions and workers, `open_path` / `finalize_session`,
the `pump_*` methods, undo/redo, sync, export, and the keyboard handlers.

`main.rs` becomes a **state machine rather than a view**, at roughly 1,400 lines.

## Testing

Identical posture to the modal split, and honest about its limits:

- **The acceptance criterion is the existing suite**: all 527 tests pass with assertions
  unchanged, and `clippy --all-targets --all-features -D warnings` stays silent.
- The eight relocated layout tests are the strongest signal available, because they exercise
  real egui layout headlessly — but see the caveat in §2 about what they actually pin.
- **`theme` gains no new test.** `caption` is one line and identical in behaviour to `hint`;
  a test asserting it calls `ui.label` would be testing the mock, not the code.
- **Manual verification**: Develop and Library both render, sliders still commit one undo step
  per drag, the crop overlay still drags, and the filmstrip and grid still open photos on
  click.

## Accepted caveats

1. **`main.rs` is still ~1,400 lines afterwards** and remains the largest file. The
   orchestration layer — decode pumping, export, undo/redo, sync — is a plausible *third*
   split, deliberately not proposed now because unlike the canvas it is **not** where M3.1
   lands. Do it when it hurts, not speculatively.
2. **`preset_panel` moves into `develop/mod.rs`** with its only caller, though it is arguably a
   fourth panel rather than part of Develop. Reconsider during the layout pass after M3.1.
3. **No new test coverage over the egui layer.** The refactor's safety comes from the compiler
   and from tests that were already there.
4. **No visible change at all.** Unlike the modal split, which moved four hint labels, this one
   should be pixel-identical — `caption` reproduces exactly what its two call sites do today.
   Anything a user can see is a regression.
