# Bundling an icon font

Status: **design agreed 2026-08-24**, not yet implemented.

## Why

The rail and toolbar draw their icons with Unicode characters, and egui's bundled fonts — Hack,
Ubuntu-Light, NotoEmoji and `emoji-icon-font` — contain almost none of what a photo editor needs.
Probing roughly 120 candidates across three batches during the rail and toolbar work found **about
twenty** that render:

```
☂ ★ ☆ ☰ ◑ ◔ ◕ ■ ▣ ○ ◎ ⌘ ✚ ⚙ ⟲ ⟳ ⛶ ⊞ ⊟ ± ⌖ ⊗ ▓
```

Everything else ships as a tofu box. **There is no magnifying glass at all** — 🔍, 🔎 and ⌕ are all
absent — so the zoom buttons ended up as `⊞`/`⊟` and a text `1:1`, which is a workaround rather than
a design. Bundling a font removes the constraint permanently, for these eleven icons and for
anything added later.

## The font: Phosphor

**Phosphor Icons**, regular weight, **version 2.1.2**, MIT licence, 489 KB, 1,530 icons.

Chosen over Lucide (ISC, 859 KB, ~1,600 icons) after seeing both rendered live in a mock rail and
toolbar. Both are stroke-drawn, both are GPL-3.0-compatible, and both carry genuine magnifying
glasses. Phosphor is the smaller of the two and ships six weights, so a bolder or filled variant is
available later without changing font family.

⚠ **`ab_glyph`, which epaint rasterises with, reads TTF and OTF only.** That ruled out Bootstrap
Icons, which distributes `woff`/`woff2` and no TrueType build — checked, not assumed.

⚠ **Licences matter here because Rustroom is GPL-3.0-or-later.** Phosphor's MIT terms are
compatible, and its licence text is vendored beside the font rather than merely referenced.

### Vendored files

| path | what |
|---|---|
| `crates/app/assets/Phosphor.ttf` | The font, from `@phosphor-icons/web@2.1.2`. ⚠ **Pinned, never `@latest`** — a future fetch could otherwise shift the codepoints under the eleven constants below. All eleven were re-verified against 2.1.2 specifically, not against `@latest`. |
| `crates/app/assets/Phosphor-LICENSE` | Phosphor's MIT text. |
| `LICENSE` | GPL-3.0. `Cargo.toml` has declared `license = "GPL-3.0-or-later"` for the life of the project without the repository ever shipping the text. |

## Registration: a fallback, not a named family

`theme::install_fonts(ctx)` appends Phosphor to the **fallback list** of both the `Proportional` and
`Monospace` families, and runs once at startup beside `theme::apply`.

Icon codepoints live in the Private Use Area, so no existing character can collide with one, and the
consequence is the point: **call sites stay ordinary text.** `ui.button(icons::UNDO)` works with no
`FontId` juggling, `Tool::glyph()` keeps returning a `char`, and every widget that takes a string
takes an icon.

The alternative — a named family, `FontFamily::Name("icons")` — was rejected. It buys independent
control of icon size and weight, and costs an explicit `FontId` at every call site plus a second
text style to keep in step with the theme. Nothing here needs icons sized independently of the text
beside them.

⚠ **This changes what the two glyph-coverage tests are testing, and improves them.**
`every_rail_glyph_is_renderable` and `every_toolbar_glyph_is_renderable` currently build a bare
`egui::Context::default()`, which has only egui's bundled fonts. They must install the real font
stack first — after which they assert against **what actually ships**, rather than against a default
that production no longer uses.

## `icons` lives in the lib

`rustroom_app::icons` — plain `char` constants, no egui, no runtime lookup.

⚠ It has to be the lib rather than a bin module: `Tool::glyph()` is in `tool.rs`, which is a lib
module, and a bin module is invisible to it. This mirrors `menu.rs`, which is in the lib for the same
reason.

Every constant carries its Phosphor name in a doc comment. A bare `'\u{e30e}'` at a call site is a
magic literal that cannot be checked against anything; the name is what makes it traceable back to an
icon, and what makes a font upgrade auditable.

## The eleven icons

| constant | Phosphor name | codepoint |
|---|---|---|
| `EDIT` | `sliders-horizontal` | U+E434 |
| `CROP` | `crop` | U+E1D4 |
| `MASKS` | `circle-half` | U+E18C |
| `PRESETS` | `star` | U+E46A |
| `UNDO` | `arrow-u-up-left` | U+E08A |
| `REDO` | `arrow-u-up-right` | U+E08C |
| `ZOOM_OUT` | `magnifying-glass-minus` | U+E30E |
| `ZOOM_IN` | `magnifying-glass-plus` | U+E310 |
| `ZOOM_FIT` | `frame-corners` | U+E626 |
| `ZOOM_ACTUAL` | `magnifying-glass` | U+E30C |
| `BEFORE_AFTER` | `square-split-horizontal` | U+E870 |

⚠ **`ZOOM_FIT` is a frame, not a fourth magnifying glass.** Phosphor has exactly three
magnifying-glass variants — plain, plus and minus — so "four magnifying glasses" is not available.
`frame-corners` is the conventional symbol for fit-to-window anyway, but this is a departure from the
original request and is recorded as one rather than quietly substituted.

⚠ **`UNDO` and `REDO` are `arrow-u-up-left` / `arrow-u-up-right`, not the circular
`arrow-counter-clockwise` / `arrow-clockwise`.** The u-turn arrows are the curved left/right arrows
that were asked for, and are what every editor uses for undo.

## Scope

The rail's four entries and the toolbar's seven controls. **Every other label stays text.** Converting
the mask list's `×`, the Add-mask buttons or the panel headings is a separate decision with no
current complaint behind it, and doing it in the same change would make the diff impossible to judge.

## Testing

- **`every_rail_glyph_is_renderable` and `every_toolbar_glyph_is_renderable`** keep their names and
  gain the real font stack. They stop being a check against egui's defaults and become a check
  against what ships.
- **`every_icon_is_in_the_private_use_area`** — new, over the whole `icons` table. ⚠ This is the one
  that catches the failure mode the others cannot: a mistyped constant that lands on a *real*
  character resolves in Ubuntu-Light and renders as a letter or a box outline. It looks like an icon
  problem and is actually a typo, and no coverage test would fail on it, because the glyph genuinely
  exists.
- **`the_icon_font_is_registered`** — asserts a known Phosphor codepoint resolves after
  `install_fonts`, so a font that failed to embed fails the build rather than shipping as tofu.

⚠ Each test gets a sabotage, run through a check that the sabotage **compiled** first — the process
finding from M3.1b, and the reason four earlier sabotages in this rework were trustworthy.

## Accepted caveats

1. **+489 KB** on a 16.7 MB release binary (+2.9%).
2. **No subsetting.** `fonttools` is not installed on this machine, and subsetting to eleven glyphs
   would trade ~480 KB for a regeneration step every time an icon is added — a poor deal at this
   size. Revisit only if binary size ever becomes a real constraint.
3. **A font upgrade means re-checking the eleven codepoints.** The vendored file is pinned to 2.1.2
   for that reason, and the doc comments naming each icon are what make the re-check mechanical:
   fetch that version's `style.css` and compare the eleven `.ph-<name>:before` contents.
4. **Icons inherit the surrounding text colour**, which is what makes them follow the theme and the
   accent for free — but it also means they cannot be multi-coloured. Not wanted here.
5. **The in-window menu bar still uses no icons**, matching its text-only menus. Only the rail and
   toolbar change.
