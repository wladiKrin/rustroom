# Dialog sizing and layout — one convention for all fourteen modals

**Date:** 2026-08-27
**Status:** design agreed, ready to plan.
**Reported as:** *"all pop up windows in general are very small and cramped."*

## What this is

Every dialog in Rustroom is an `egui::Modal` sized by whatever its content happens to want, laid
out against egui's default 6 pt frame margin. The result is uniformly tight and mutually
inconsistent: the rule editor's rows wrap, the export dialog's hint text crowds its controls, and
Settings' `label: control` rows sit at ragged positions.

⚠⚠ **This is a `theme`/funnel job, not fourteen per-dialog ones.** Nine dialogs each growing their
own spacing is how they got here; fixing them one at a time reproduces exactly that.

## The facts that shaped the design

Each was read out of the vendored egui 0.35 source, not assumed:

1. **`egui::Modal` has no width knob at all.** Its only sizing surface is `frame(Frame)` and
   `area(Area)`. Width comes entirely from content — which is *why* they are all different.
2. **`Modal`'s default frame is `Frame::popup`, whose inner margin is `style.spacing.menu_margin`
   = `Margin::same(6)`.** That is egui's default, not a choice this project ever made, and it is
   the bulk of the "cramped" feeling.
3. **`Modal` anchors `Align2::CENTER_CENTER`.** It stays centred as it grows.
4. **`egui::Resize` exists** with `default_size`, `min_size`, `max_size`, `resizable` and
   `id_salt` — everything needed, and nothing to write from scratch.
5. **⚠ `Resize::State::desired_size` is `pub(crate)`.** The size cannot be read back out of egui,
   which rules out "load the state on close and persist it" and drives the measurement below.
6. **`Resize::show` returns only the inner value**, not a `Response` — so there is no
   `drag_stopped()` to persist on, unlike the snapshot strip's panel.
7. **`theme::tune` does not touch `menu_margin`**, so overriding the frame per-modal affects no
   real menus.

## Architecture

`modals::dialog` already funnels **13 of the 14** dialogs. It grows one parameter:

```rust
pub fn dialog<R>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: DialogSize,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> bool
```

- **`DialogSize` lives in `theme`** with its numbers — it is a design-system fact, and `theme` is
  where `button_row`, `hint`, `caption` and `committed` already encode this app's conventions.
- **`dialog` owns the frame, the resize, the scroll and the persistence.** A call site names its
  class and nothing else.
- ⚠ **`shortcuts` and `about` convert to it.** They build their `egui::Modal` by hand today, so
  they would otherwise miss every part of this. Folding them in is what makes the convention
  universal rather than merely widespread — and Keyboard shortcuts is one of the dialogs that most
  wants to be Wide and scrollable.

### The three classes

| class | default | min width | dialogs |
|---|---|---|---|
| `Compact` | 340 × 140 | 300 | Delete preset, Delete export preset, Forget folder, About |
| `Standard` | 480 × 320 | 400 | Save preset, Save export preset, Export, Paste settings, Sync settings, Settings, File already exists |
| `Wide` | 640 × 480 | 520 | Rule editor, Keyboard shortcuts, Information |

That is 4 + 7 + 3 = **14**, which is every dialog in the app.

⚠ Two classifications are worth their reasons. **About is `Compact`** despite being a "form-ish"
dialog — it is two lines and a Close button, and `Standard` would be mostly empty. **"File already
exists" is `Standard` despite being a confirmation** — it shows a full filename, which wraps badly
at 340 pt.

⚠ The heights are **defaults, not caps** — `Resize` grows to content up to the viewport bound
below. The widths are what the class actually asserts.

⚠ Three rather than one, because a two-button "Delete preset?" at the rule editor's width reads as
padding rather than design. Three rather than two, because Information and Settings want to sit
between a confirmation and the widest thing in the app.

### Padding

`dialog` passes `Frame::popup(style).inner_margin(14)`.

⚠ **Not by raising `menu_margin` in `theme::tune`**, which is shared with every real menu and
would fatten the context menus too.

⚠ **The compact panel metrics stay exactly as they are.** `tune` tightens `item_spacing.y` and
`slider_rail_height` deliberately, *for the panel* — a panel of forty sliders is where vertical
space is scarce. A dialog is the opposite case, and this change gives dialogs room without
touching the panel's.

### Resize

An `egui::Resize` inside the frame:

```rust
egui::Resize::default()
    .id_salt(id)
    .default_size(seed)          // persisted size, else the class default
    .min_size(class.min())
    .max_size(viewport * 0.8)
    .resizable(true)
```

⚠⚠ **The dialog stays centred as it grows, so the corner handle drifts from the pointer at half
rate.** That is inherent to `Modal`'s `CENTER_CENTER` anchor and cannot be fixed without giving up
the centring, which was a deliberate choice over `egui::Window` (*"a Window opens at the top-left
and is genuinely easy to miss"*). **This is the one thing to judge in the running app rather than
on paper.** If it feels wrong, the fallback is to drop the handle and keep the classes.

### Persistence

```rust
/// Sizes the user has dragged dialogs to, keyed by dialog id.
pub dialog_sizes: std::collections::BTreeMap<String, [u32; 2]>,
```

- ⚠ **`[u32; 2]`, not floats.** `Settings` derives `Eq`, which no float satisfies — the reasoning
  `library_cell` and `snapshot_strip_height` already carry.
- ⚠ **`BTreeMap`** so the settings file has a stable order and a diff of it is readable — the
  reason `camera_profiles` is one.
- ⚠ **No field-level `#[serde(default)]`.** `Settings` carries a container-level one, so a missing
  field comes from `Settings::default()`. A field-level one would work here too (an empty map
  either way) but the container-level route is what every other field uses.
- ⚠ **Persisted at all because egui's own `Resize` state dies with the process** — this app
  implements no `eframe::App::save`. The panel-sections work established that a control whose
  state does not survive a restart is worth very little.

**Read on open, written on close.** Not per frame (a resize runs at 60 Hz — the
`apply_view_demosaic` rule) and not on drag-stop, because fact 6 says there is no settle signal.
A dialog has an exact close hook; a resize drag does not.

⚠⚠ **The size is measured as `ui.available_size()` on the first line of the content closure** —
the `content_ui` of the `Resize`, whose extent is the same quantity `default_size` sets. Feeding
back the same quantity is what makes a round trip unable to drift, exactly as the snapshot strip
feeds back the panel's outer rect.

⚠ **This is the one mechanism in the design that could not be confirmed from the source**, because
`desired_size` is private. The plan must verify it empirically: resize a dialog, close it, reopen
it, and assert the size came back — and check it does not creep across several cycles.

### Max height and scrolling

The `Resize`'s `max_size` is 80% of `ctx.screen_rect()`, and the content sits in a
`ScrollArea::vertical()`, so a long rule or a long preset list can never push the dialog past the
bottom of the screen.

⚠ **The buttons scroll with the content rather than staying pinned**, because `dialog()` does not
own the button row — each dialog draws its own `theme::button_row` at the end of its closure.
Pinning them means restructuring `dialog` to take content and buttons separately, across all
fourteen call sites. Not proposed; recorded as the follow-up if it annoys in use. The buttons
remain *reachable* by scrolling, which is the actual bug being fixed.

⚠ **Three dialogs have their own inner `ScrollArea` today** — `info` (`max_height(460)`),
`rule_edit`, and `shortcuts` (`max_height(420)`). **These must be removed**, or they nest inside
the shared one, and nested vertical scroll areas fight over the wheel — the exact bug the Library
panel's three inner scroll areas were removed for on 2026-08-27.

### Aligned label columns

`Information`, `Settings` and `Export` convert their `label: value` rows to `egui::Grid`.

⚠ **Information is already half-solved and the other two are not**, which is worth stating so the
change is not oversold: `info::row` uses `add_sized([130.0, ..])`, a fixed-width label. That
aligns, but truncates a long label and is a magic number; `Grid` sizes to the widest row instead.
`Settings` and `Export` use bare `ui.horizontal(|ui| { ui.label(..); control })` with no alignment
at all — those are the ragged ones.

## Testing

Appearance is exactly what the headless harness cannot see — its own recorded caveat — so this is
judged by looking, with the app running, one dialog at a time. What *can* be tested headlessly:

- **`DialogSize`'s numbers are ordered and sane**: `Compact.min() < Standard.min() < Wide.min()`,
  and every default is at least its own minimum. A table test over all three variants, so a fourth
  class cannot ship inconsistent.
- **Every dialog id in the app maps to a class** — a completeness test over the call sites, in the
  spirit of `every_rail_glyph_is_renderable`.
- **The persisted size round-trips through `Settings`**, including a settings file written before
  the field existed — the sibling of
  `a_settings_file_from_before_the_snapshot_strip_still_loads`.
- **A nonsense stored size is clamped** into the class's range rather than trusted; the file is
  user-editable, and a zero width would make a dialog unreachable.

### Verified by hand, with the app running

1. Each of the fourteen dialogs opens at a sensible width for its class.
2. None is cramped against its frame.
3. The corner handle resizes; **judge the centred drift**.
4. Resize, close, reopen — the size came back. Repeat three times: it must not creep.
5. A long rule and the shortcuts list scroll rather than overflowing the screen.
6. Settings, Export and Information have aligned value columns.
7. Light mode, which has barely been looked at.

## Accepted caveats

1. **The resize handle drifts from the pointer**, inherent to a centred modal.
2. **Buttons scroll with the content** rather than pinning to the bottom.
3. **The size is per dialog, not per class**, so making one Standard dialog wider does not widen
   its siblings. That is the point, and it means the settings file can accumulate up to fourteen
   entries.
4. **A dialog resized on a large monitor may exceed a smaller one.** The viewport-relative
   `max_size` clamps it on open, so this self-corrects rather than stranding a dialog off screen.
5. **No per-dialog *position*** — modals stay centred and cannot be dragged.
6. **The class is a compile-time choice**, so there is no UI for changing which class a dialog is.
