# Presets & copy/paste — design

Date: 2026-08-07
Status: approved

## Goal

Move an `Edits` from one place to another: **copy/paste** between photos, and **presets**
saved to disk and reapplied. Both need the same underlying idea — *which* settings transfer —
so they are one feature, not two.

`Edits` is already `serde` and the pipeline is driven entirely from it, so this is **app-layer
only**. No stage changes, no render changes.

## Scope: sync is deliberately excluded

CLAUDE.md framed presets + copy/paste + **sync** as one feature. Two of them are; sync is
different in kind:

- Copy/paste and presets apply an `Edits` to **the current photo**.
- Sync applies one to **many selected photos** — and `Library.selected` is `Option<usize>`,
  so multi-selection does not exist yet. That is a Library-model change, not an
  `Edits`-moving change.

Multi-selection is also exactly what **batch export** (the next Phase 2 gap) needs. Cutting
there means it gets designed once, on its own terms, and serves both — rather than being
decided under time pressure from two unrelated features. **Sync gets its own spec, bundled
with multi-selection.**

## 1. The shared core: `EditGroups` in `edit-model`

```rust
/// Which groups of settings a paste or a preset carries.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditGroups {
    pub white_balance: bool,
    pub tone: bool,
    pub color: bool,
    pub detail: bool,
    pub effects: bool,
    pub geometry: bool,
}

impl Edits {
    /// Overwrite this photo's settings with `src`'s, for the selected groups only.
    pub fn merge_from(&mut self, src: &Edits, groups: EditGroups);
}
```

The partition:

| group | fields |
|---|---|
| `white_balance` | `white_balance` |
| `tone` | `exposure`, `contrast`, `highlights`, `shadows`, `whites`, `blacks`, `tone_curve` |
| `color` | `saturation`, `vibrance`, `hsl` |
| `detail` | `noise_reduction`, `sharpening` |
| `effects` | `vignette` |
| `geometry` | `geometry` |

`version` is a schema marker, not an edit, and **never transfers** — the target keeps its own.

`EditGroups::default()` is **everything except geometry**. Geometry is the one group that is
usually wrong to carry across photos: a crop framed for one composition reframes another, and
a preset built on a 3:2 crop would reshape every photo it touches. `EditGroups::ALL` and
`EditGroups::NONE` exist for tests and for a "check all" button.

### ⚠ The exhaustiveness trap

This has the same failure mode as `PrefixKey`: add a field to `Edits`, forget to assign it to
a group, and `merge_from` **silently never transfers it**. Nothing fails, and the bug surfaces
much later as "my preset doesn't include X".

Two guards, and the first is the important one:

1. **`merge_from` destructures `Edits` exhaustively** — `let Edits { white_balance, exposure,
   … } = src;` — so a new field is a **compile error** until someone decides which group owns
   it. This is the `all_edits()` tripwire pattern the pipeline tests already use, applied to
   production code rather than to a test fixture.
2. Tests pinning the partition as a partition (§5).

## 2. Presets on disk: `app::presets`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub groups: EditGroups,
    pub edits: Edits,
}

pub struct PresetStore { dir: PathBuf }

impl PresetStore {
    pub fn new(dir: PathBuf) -> Self;
    pub fn user_default() -> Self;              // ~/.config/rustroom/presets
    pub fn list(&self) -> Vec<Preset>;          // sorted by name; unreadable entries skipped
    pub fn save(&self, preset: &Preset) -> Result<(), PresetError>;
    pub fn delete(&self, name: &str) -> Result<(), PresetError>;
}
```

**One JSON file per preset**, not one file holding all of them. That makes a preset a
shareable artifact (copy the file), makes add and delete plain file operations with no
read-modify-write race, and means one corrupt preset cannot take the rest down with it.

**The preset stores its own group mask.** You choose the groups once, when creating it, and
applying it only touches those — so a "punchy tone" preset never disturbs the photo's white
balance. Copy/paste chooses groups at *paste* time instead. Same `EditGroups` type serves
both; that shared type is the whole reason these are one feature.

**The name lives inside the file as well as in the filename**, because the filename is
sanitized and the display name should survive that.

**`dir` is injected** so tests run against a temp directory and never touch the real home
directory — the same pattern `DecodeWorker` uses for its decode fn. `user_default()` is the
only thing that knows about `~/.config`, via a new `dirs` dependency (tiny, cross-platform;
CLAUDE.md's "avoid platform-specific assumptions" rules out hand-rolling `$HOME`).

**`list()` skips unreadable or malformed files rather than failing.** A hand-edited preset
with a typo must not make the preset panel disappear.

### ⚠ Filename sanitizing is load-bearing

The preset name comes from a free-text field and becomes a path. A preset named `../../evil`
must not write outside `dir`. Sanitizing replaces anything outside `[A-Za-z0-9 _-]` and
rejects a name that sanitizes to empty. This gets an explicit test — it is the one place in
this feature where user input reaches the filesystem.

## 3. Settings: `app::settings`

Rustroom has **no settings system today** — nothing is persisted between launches, not even
`show_filmstrip`. The "don't ask again" checkbox in §4 requires one, so this spec creates it,
deliberately minimal:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Ask before deleting a preset. Cleared by "Don't ask again"; restored from the
    /// Settings window.
    pub confirm_preset_delete: bool,      // default: true
}

pub struct SettingsStore { path: PathBuf }

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self;
    pub fn user_default() -> Self;        // ~/.config/rustroom/settings.json
    pub fn load(&self) -> Settings;       // infallible
    pub fn save(&self, s: &Settings) -> Result<(), SettingsError>;
}
```

**`load()` cannot fail.** A missing *or malformed* file yields `Settings::default()`. A corrupt
settings file must never brick the app — the same forgiving posture the JSON sidecars already
take, and the reason `#[serde(default)]` is on the struct.

**Saved on change, not on quit.** It is one flag in a tiny file, and quitting is not always
graceful.

Deliberately **one field**. Other settings (`show_filmstrip`, panel widths, last folder,
export defaults) move in when they are actually wanted, not speculatively — egui panel sizes
in particular are fiddly to save and restore, and that belongs in whatever spec needs them.

## 4. UI

All in `crates/app/src/main.rs`; the stores above stay in the lib so only rendering lands here.

### Preset panel

A collapsible **left** `egui::Panel` in Develop, toggled by a "Presets" button in the toolbar
beside "Filmstrip". Lightroom puts presets on the left, and the right panel already scrolls
through Hue / Saturation / Luminance / Sharpening / Noise Reduction / Vignette — presets there
would be buried.

Contents: a **"Save current as preset…"** button, then one row per preset — click the name to
apply, `×` to delete.

Unlike the filmstrip toggle, which appears only when a folder is loaded, the **Presets toggle
appears whenever a photo is open** — presets are just as useful on a single-file open, and
they do not depend on a `Library` at all.

### Two modals (`egui::Window`)

**Save preset:** name field + the six group checkboxes, geometry unchecked. Disabled until the
name is non-empty. Saving an existing name overwrites it.

**Paste settings:** the same six checkboxes, seeded from the last-used selection.

### Copy / paste

`RustroomApp.clipboard: Option<Edits>` and `paste_groups: EditGroups` (last used).

- **Ctrl+C** copies the current photo's `Edits`.
- **Ctrl+V** pastes immediately using the **last-used** group selection — no dialog.
- **"Paste…"** in the toolbar opens the checkbox modal.

Both shortcuts are gated on `ctx.memory(|m| m.focused())`, the way arrow-nav already is, so
`Ctrl+C` inside the preset-name field is not hijacked.

Applying a preset or pasting is a single `history.push(edits)` — **one undo step, for free**,
because `History<Edits>` is snapshot-based.

### Delete flow

Clicking `×`:

- if `confirm_preset_delete` — a modal naming the preset, with a **"Don't ask again"**
  checkbox and Delete / Cancel;
- otherwise — delete immediately.

The Settings window (a "Settings…" toolbar button) exposes the flag as "Confirm before
deleting a preset", so it can be turned back on. With one setting, a modal is the right
weight; a settings *panel* would be overkill.

#### ⚠ "Don't ask again" must only apply on Delete

Ticking the box and then clicking **Cancel** must leave `confirm_preset_delete` **unchanged**.
Otherwise a user can tick it, think better of the deletion, cancel — and have silently
disarmed a guard they just declined to use. The checkbox binds to a scratch variable that is
committed to `Settings` only on the Delete path. This gets its own test.

## 5. Testing

**`edit-model` — the partition, pinned as a partition.** Starting from an `Edits` with every
field non-neutral (the `all_edits()` pattern already used in the pipeline tests):

- merging with `ALL` reproduces the source exactly, except `version`;
- merging with `NONE` changes nothing;
- **for each group individually, merging that group alone changes that group's fields and no
  others** — this is the assertion that makes it a partition rather than six overlapping sets,
  and the one that catches a field assigned to two groups;
- `EditGroups` round-trips JSON, and an older file missing the field loads as `default()`.

**`app::presets`** (temp dir, no home-directory writes): save → list → delete round-trip;
overwriting an existing name; `list()` sorted by name; a malformed `.json` in the directory is
**skipped, not fatal**; a name of `../../evil` stays inside `dir`; a name that sanitizes to
empty is rejected.

**`app::settings`** (temp path): default when the file is absent; default when the file is
malformed; save → load round-trip; an unknown extra field in the JSON is ignored.

**The delete-confirm trap:** a unit-testable helper decides the outcome, so "ticked + Cancel
leaves the flag true" is asserted without driving egui.

**⚠ No pipeline stage is touched, so all 23 goldens must stay byte-identical.** The plan
asserts this explicitly — it is the cheap proof that an app-layer feature stayed in the app
layer.

## 6. Accepted caveats

1. **A preset resets, it does not merge.** A preset whose `tone` group is on but whose tone
   values are neutral will *reset* the target's tone to neutral. This is correct — the preset
   asserts "tone is this", and this is neutral — and it is what Lightroom does, but it
   surprises people the first time.
2. **No shipped/built-in presets.** The panel starts empty.
3. **No preset folders or tags.** A flat list; fine until someone has dozens.
4. **Not Lightroom-compatible.** No `.xmp` or `.lrtemplate` import/export. Presets are plain
   JSON in a known directory, so they are copied and shared by hand.
5. **No rename.** Save under the new name and delete the old one.
6. **The clipboard is in-memory and per-session** — it does not survive a restart and does not
   touch the OS clipboard.
7. **No multi-photo apply.** Applying a preset or pasting affects the current photo only;
   that is what sync will add.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The 23 golden references are byte-identical.
- `merge_from` fails to compile if a field is added to `Edits` without being grouped.
- In the GUI: copy/paste with Ctrl+C / Ctrl+V and via the dialog; save, apply and delete a
  preset; the delete confirm's "Don't ask again" works and is restorable from Settings; all of
  it participates in undo/redo and survives a restart where it should.
- `CLAUDE.md` updated: the new `edit-model` type, the two new `app` lib modules, the settings
  subsystem, and the note that sync + multi-selection is the next spec.
