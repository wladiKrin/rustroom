# Visual identity: theme, accent, density

**Status:** spec, 2026-08-13.

## Why now

`theme.rs` exists but is a seam with nothing behind it — `tune` is empty and the app renders
egui's defaults. That was deliberate: the modal split created the seam, and this spec is what
comes through it.

The sequencing recorded in the roadmap puts **foundations before Phase 3** and **layout after
M3.1**. This is the foundations half. Nothing here moves a panel or changes what the toolbar
holds; masking introduces a mask list that the layout has nowhere to put, so that pass waits.

## Decisions taken

Reached over three visual comparisons in the brainstorming companion.

| Question | Decision |
|---|---|
| Character | **Dark and near-neutral, plus one accent** for active state. |
| Which accent | **Steel blue** by default — but **user-switchable**, including a **neutral / no-accent** option. |
| Light mode | **Survives, as a setting.** Dark is the default; Light and System are available. |
| Density | **Compact** — about 50% more slider rows visible. |
| Icons | **Not in this spec.** Everything stays text. |

### The argument that narrowed the accent

A photo editor's UI is the *surround* you judge the image against, and surround affects
perceived tone and saturation. That is why Lightroom, Capture One and darktable are all dark
and near-neutral — it is not fashion. It argues for low chroma **near the image**, and says
nothing about panels further away, which is where an accent can live safely.

It also rules out more than taste does. **Amber is deliberately absent from the accent list**:
it sits on the skin-tone and colour-temperature axis, which is precisely the judgement Temp and
Tint exist to serve, so a warm cast in the image becomes harder to see against it. It also
collides with the conventional meaning of amber as a warning. One line to add if that judgement
is overruled.

### Why light mode costs more than it looks

An accent tuned for near-black washes out on near-white: `#7fb0d8` on a light base is low
contrast and fails white text on it. So **each accent is a pair, not a value** — eight
constants rather than four. This was demonstrated rather than asserted, side by side, before
the decision.

## Design

### 1. The choice lives in the lib; the colour lives in the bin

`Settings` is in `rustroom_app`, which is **deliberately egui-free** — that separation is what
keeps `history`, `selection` and the rest testable without a windowing system. So `Settings`
cannot hold an `egui::ThemePreference` or a `Color32`. Two plain enums join it in
`settings.rs`:

```rust
/// Which theme to render in. Mirrors `egui::ThemePreference`.
pub enum ThemeChoice { Dark, Light, System }   // default: Dark

/// The colour used for active state — slider fill and handle, open section, primary button.
pub enum Accent { Neutral, SteelBlue, Teal, Violet }   // default: SteelBlue
```

Both get `serde`, `Eq`, and an `ALL` table for the dialog and the tests. `theme.rs` maps them
to egui. The lib knows *what the user picked*; the bin knows *what it looks like*.

⚠ **`ThemeChoice` is a mirror enum, which `CLAUDE.md` explicitly warns against** — it is the
stated reason `Demosaic` lives in `raw-decode` rather than being duplicated in `app`, since "a
mapping that exists twice is one that will disagree". Accepted here because the direction is
forced: the lib cannot depend on egui by design. The mitigation is that the mapping is three
exhaustive match arms, so adding a variant is a compile error rather than a silent gap.

### 2. Each accent is a pair, and the pair is checkable

```rust
impl Accent {
    /// The accent colour for this theme. `dark` selects which half of the pair.
    pub fn color(self, dark: bool) -> egui::Color32;
}
```

| accent | dark | light |
|---|---|---|
| `Neutral` | `#9a9a9a` | `#5a5a5a` |
| `SteelBlue` | `#7fb0d8` | `#2f6a94` |
| `Teal` | `#5fb3ac` | `#2c6d68` |
| `Violet` | `#9a8fd0` | `#544a8c` |

`Neutral` is not an absence — it returns a grey, so the "no accent" direction is a real choice
rather than a discarded one. ⚠ Its cost is real too: active state falls back to lightness
alone, which is genuinely harder to read.

**The test that earns its place:** contrast ratio between each accent and its base, swept over
`Accent::ALL` × both themes, asserted **≥ 3:1** (the WCAG threshold for non-text UI
components). That is exactly the property the light-mode washout demonstrated, and it is what
stops a fifth accent shipping unreadable.

⚠ **The eight values above were computed against that threshold before being written down**,
not eyeballed and left for the test to discover — against `#1c1c1c` and `#f2f2f2`:

| accent | dark | light |
|---|---|---|
| `Neutral` | 6.06:1 | 6.16:1 |
| `SteelBlue` | 7.39:1 | 5.20:1 |
| `Teal` | 6.92:1 | 5.37:1 |
| `Violet` | 5.85:1 | 6.86:1 |

Worst case 5.20:1, comfortably clear. So the test is a **guard for what comes next**, not a
hurdle this change has to scramble over — which is the right relationship between a spec and
its tests.

### 3. `tune` stops being empty

The compact metrics, applied to **both** themes because they are theme-independent:

- `spacing.item_spacing.y` reduced
- `spacing.interact_size.y` reduced
- slider handle diameter reduced
- body text one step smaller

⚠ These are the *only* things in `tune`. Anything colour-related belongs in that theme's own
`visuals` and must not be copied across, or light mode inherits dark colours — the rule already
documented on `apply`.

### 4. `apply` takes the settings, and is called again on change

```rust
pub fn apply(ctx: &egui::Context, settings: &Settings);
```

It sets `ctx.set_theme(..)` from `ThemeChoice`, then `style_mut_of` on both themes with `tune`
plus the accent's per-theme colour into `visuals.selection.bg_fill` and the widget
hover/active fills.

Changing either setting re-applies immediately — the same shape as `apply_view_demosaic`.
⚠ **Including the one-frame guard.** The Settings dialog body runs every frame it is open, so
an unguarded re-apply would fire sixty times a second. That rule is already written into
`modals/mod.rs`'s module doc; this is its second instance.

### 5. The Settings window gains an Appearance section

Two combo boxes — Theme and Accent — above the existing 1:1 view quality control, each with a
`theme::hint` line. No new dialog; the section is three lines of egui.

## Testing

- **`every_accent_has_usable_contrast`** — `Accent::ALL` × dark/light, contrast ≥ 3:1 against
  that theme's base. The load-bearing test.
- **`every_accent_differs_between_themes`** — a pair whose halves are equal means someone added
  a variant and filled in one value; that is the mistake this feature exists to prevent.
- **`a_settings_file_from_before_the_appearance_settings_still_loads`** — the guard every new
  `Settings` field needs, because `SettingsStore::load()` is infallible and a field that fails
  to parse silently reverts **every** setting.
- **`the_appearance_settings_round_trip`**.
- ⚠ The existing exhaustive `Settings { .. }` literal in
  `settings_round_trip_carries_the_export_options` will fail to compile until both new fields
  are added. That is deliberate — it is what forces a decision about each new field.
- **Manual**: switch through all four accents and all three themes in the running app; confirm
  the change is immediate, survives a restart, and that Neutral genuinely reads as no-accent.

## Accepted caveats

1. **Two palettes to maintain**, as chosen. Every future colour decision is made twice.
2. **`ThemeChoice` duplicates `egui::ThemePreference`**, against the codebase's own stated
   preference, for a reason that is structural rather than lazy.
3. **Compact costs trackpad precision** on fine slider drags — the trade the density comparison
   made visible.
4. **`Neutral` is harder to read** than any accent, since active state becomes lightness alone.
5. **Amber is excluded** on a functional argument, not a neutral one.
6. **No icons.** Everything stays text. ⚠ This is the *structural* half that is expensive to
   defer — every feature added meanwhile multiplies the buttons that would need converting — so
   it is recorded as a decision taken, not an omission overlooked.
7. **The accent reaches only egui's own widget colours.** Anything drawn by hand — the crop
   overlay, the histogram, the selection border — keeps its current hardcoded colours until
   someone routes them through `theme` too.
