# Splitting the modals out of `main.rs`, and the first design foundations

**Status:** spec, 2026-08-13.

## Why now

`crates/app` is ~40% of the codebase and `main.rs` alone is ~3,400 lines, holding the egui
view, the session lifecycle, nine modal dialogs, the canvas, the Library grid and the
filmstrip. The trigger is **Phase 3**: M3.1's masking UI needs a mask list, per-mask panels
and canvas overlays, and it lands on top of this file.

⚠ **The measurement corrected the plan.** The nine modals total ~635 lines — **19%** of
`main.rs`, leaving ~2,780 behind. `develop_body` alone is 380 lines, the single largest
function, and the canvas is where masking lands hardest. So this refactor is the safest cut
but *not* the one that most helps what it is preparing for.

Hence the decision below to do two contained refactors rather than one large one.

## Decisions taken

| Question | Decision |
|---|---|
| Scope | **Modals now, canvas next.** Two specs, two refactors, each independently verifiable. |
| Visual changes | **theme scaffolding is a no-op; dialog conventions get *encoded*.** ⚠ The inconsistencies this was meant to fix turned out not to exist — see §5. One visible change remains. |
| Where the visual identity is chosen | **Not here.** A separate exercise once `theme.rs` exists, where it is a one-file change. |

### Why two refactors rather than one

Modals and canvas together would take `main.rs` to ~1,200 lines in one pass. Rejected: it is a
~1,000-line diff touching the canvas — where the render ladder, region logic and crop overlay
interact — with almost no test coverage over the egui layer. Two passes are independently
reviewable, independently bisectable, and the dialog conventions established here inform the
canvas split rather than being invented twice.

### The three layers of "design work", and their timing

This is recorded because getting the order wrong is what produces the overhaul-then-overhaul
trap:

1. **Foundations** — spacing scale, palette, typography, corner radii, and the *conventions*
   for how a dialog, a panel section or a control row is built. Cheap now, expensive later:
   everything built afterwards inherits them, everything built before needs converting.
   **This spec establishes the mechanism; it does not choose values.**
2. **Visual identity** — the actual colours, icons, symbols, button forms. Deferred, and
   cheap to defer: anything expressible in `egui::Style`/`Visuals` is one central change
   applied retroactively everywhere.
3. **Layout / information architecture** — where panels live, what the toolbar holds. Must
   wait for **after M3.1**: masking introduces a mask list, a top-level concept the current
   layout has nowhere to put. Rearranging before that means rearranging twice.

⚠ The asymmetry to remember: **colour and metric choices are cheap late; structural choices
are expensive late.** Whether buttons carry icons, whether sections have headers, how a
control row is composed — every feature added meanwhile multiplies the instances to convert.

## Design

### 1. Layout — `crates/app/src/modals/`

| file | contents | ~lines |
|---|---|---|
| `mod.rs` | the `Modal` enum, the `draw` dispatch, the `dialog()` helper, and the borrow idiom's doc comment | 90 |
| `export.rs` | export settings, save export preset, confirm delete export preset, collision | 320 |
| `presets.rs` | save preset, confirm delete preset | 120 |
| `edits.rs` | paste, sync | 80 |
| `settings.rs` | the Settings window | 60 |

Grouped by what they serve rather than split one-per-dialog: files that change together live
together. The export group is four dialogs because they form one flow — the save-preset and
confirm-delete dialogs both **return to** `ExportSettings` when closed.

### 2. Free functions taking `&mut RustroomApp`

`modals::settings(app, ctx)` replaces `self.settings_modal(ctx)`.

⚠ **No visibility changes are needed anywhere.** Rust privacy is "visible in the declaring
module *and its descendants*", and `modals` is a descendant of the crate root where
`RustroomApp` is defined — so its private fields are reachable without a single `pub`.

**Rejected: an `impl RustroomApp` block in another file.** Inherent impls only have to be in
the same *crate*, so this would be a cut-and-paste with zero call-site changes — which is
exactly what makes it a file move dressed as a refactor. Every dialog would still silently
reach anything on `self`, and a reader would learn nothing about what each one touches.

**Rejected: input/output structs per dialog.** Genuinely decoupled and unit-testable, but the
testable logic has *already been extracted* — `Settings::apply_delete_confirm` and
`batch_export::destination_ready` are pure and tested today. What remains in these functions
is glue, and wrapping glue in ceremony makes it longer without making it safer.

### 3. `app::theme`

Owns the `egui::Style`, applied once at startup via `ctx.set_style()`. **Built to reproduce
today's look exactly** — no colour or metric moves in this change — so that it becomes the
one place a later change happens.

Plus the named idioms that exist today unnamed:

```rust
/// The small grey explanatory line under a control.
pub fn hint(ui: &mut egui::Ui, text: &str);

/// The trailing confirm/dismiss pair. Returns `(confirmed, dismissed)`.
///
/// Encodes the order the dialogs already use, so it cannot drift as dialogs are added.
pub fn button_row(
    ui: &mut egui::Ui,
    confirm: &str,
    confirm_enabled: bool,
    dismiss: &str,
) -> (bool, bool);
```

`hint` earns its place immediately: `egui::RichText::new(..).small()` appears at **eight**
sites today, and that idiom is exactly what a later visual pass will want to change in one
place.

⚠ **But only four of the eight are hints**, and the distinction is worth encoding rather than
flattening. `.small()` currently serves two unrelated roles:

| role | sites | in this refactor? |
|---|---|---|
| **hint** — explanatory text under a control | 4, all in dialogs | ✅ becomes `theme::hint` |
| **caption** — a filename under a thumbnail, a camera model beside a title | 2, in the Library grid and the develop toolbar | ❌ deferred |
| test fixtures | 2 | ❌ untouched |

The captions are deliberately left alone: they live in `develop_body` and `library_view`,
which belong to the **canvas split**, and reaching into them here would spread this refactor
past its own boundary. `theme::caption` arrives with that spec, where its call sites are.

That two visually identical idioms mean different things is exactly the sort of thing a theme
module exists to disentangle — and exactly why flattening them into one helper now would be a
mistake.

⚠ **No `danger_button`.** An earlier draft of this spec proposed one; there is no visual
distinction to encode yet, and adding an alias that does nothing but call `ui.button` is the
speculative-helper trap. It arrives when the visual identity gives destructive actions a look.

### 4. The borrow idiom, written down rather than rediscovered

Every dialog already follows the same shape, and it is load-bearing rather than incidental:

```rust
let mut local = app.settings.field;        // copy out
let mut close = false;
let dismissed = dialog(ctx, id, title, |ui| { /* mutate locals only */ });
close |= dismissed;
if local != app.settings.field { /* apply */ }
```

The closure cannot touch `app`, because `dialog` already holds it. This has cost compile
errors twice in this codebase's history, so it goes in `modals/mod.rs`'s module doc.

⚠ A second rule belongs there too, learned from the view-quality setting: the dialog body runs
**every frame** the dialog is open, so any side effect must be guarded by a change comparison,
never fired unconditionally.

### 5. Enumerated visual changes — and a correction

⚠ **An earlier draft of this spec claimed two inconsistencies that do not exist.** Auditing
every dialog's buttons before writing the plan showed the conventions are *already* uniform:

| dialog | buttons, in order |
|---|---|
| save preset | Save, Cancel |
| paste | Paste, Cancel |
| sync | Sync, Cancel |
| confirm delete preset | Delete, Cancel |
| save export preset | Save, Cancel |
| confirm delete export preset | Delete, Cancel |
| export | … Export, Cancel |
| collision | *(per-choice buttons)*, Cancel export |
| settings | Close |

Confirm-first, dismiss-second, in all nine. And "Close" appears **only** in Settings — which
is the one dialog that has nothing to abandon, since its changes apply immediately. That is
exactly the Close-vs-Cancel rule the earlier draft proposed *imposing*; it is already
followed.

So the visible change list reduces to **one item**:

1. **Hint labels** route through `theme::hint`, which may alter their spacing very slightly
   where an inline `.small()` had no consistent padding. **Four sites**, all inside dialogs.

Anything else a user can see is a regression.

**This makes the refactor better, not emptier.** `theme::button_row` now *encodes* a
convention the code already follows rather than imposing a new one — so its job is to stop the
convention drifting as dialogs are added, and it can be introduced with no visible change at
all. A helper that captures existing practice is worth more than one that changes it.

## Testing

Honest about what is testable here: this is egui view code, and the pure logic behind it was
already extracted and tested before this refactor.

- **The acceptance criterion is the existing suite.** All 524 tests pass unchanged, and
  `clippy --all-targets --all-features -D warnings` stays silent. The pure functions the
  dialogs call keep their tests untouched — that they compile and pass against relocated
  callers is the evidence the move preserved behaviour.
- **`theme` gets two tests**: the style is constructed without panicking, and `hint` produces
  text smaller than the body style. Thin, but it pins the one property a later visual pass
  could silently break.
- **Manual verification** against the enumerated list above, in the running app.

## Accepted caveats

1. **`main.rs` is still ~2,780 lines afterwards.** This is a step, not the fix; the canvas
   split is the next spec.
2. **The dialogs remain coupled to `RustroomApp`** — they take `&mut` to the whole thing. That
   is deliberate, per the rejected alternatives above, and it means a dialog can still touch
   more than it should. The signature at least makes the coupling visible.
3. **No new test coverage over the egui layer.** The refactor's safety comes from the compiler
   and from the pure functions' existing tests, not from new ones.
4. **One small visible change** — hint-label spacing, at four sites.
5. **`theme` encodes today's look, which has never been designed.** It reproduces egui's
   defaults faithfully; that is the point of this step, not an endorsement of them.
