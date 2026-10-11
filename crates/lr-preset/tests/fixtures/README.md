# Lightroom preset fixtures — every file real, none written here

⚠ This project has paid for hand-written fixtures before (`docs/JOURNAL.md`, M9.1's `album.json`): a
fixture you write checks an assumption against itself. Every file below was written by Lightroom /
Camera Raw or by a third party, and says where it came from.

| File | Source | Licence |
|---|---|---|
| `questtion/*.xmp` (5 presets) | [Questtion/Lightroom-Presets](https://github.com/Questtion/Lightroom-Presets), `Presets/`, commit `10e36149ab94e41a702946075ae30195c23af77e` — the author's own presets, written by Camera Raw 17.5 (`crs:Version="17.5"`) | MIT, `questtion/LICENSE` |
| `questtion.zip` | GitHub's archive of that same commit (`codeload.github.com/…/zip/10e3614…`) — a real zip: folders inside, deflate and stored entries | MIT, as above |
| `yuzukumo/minimal.lrtemplate` | [yuzukumo/LRtemplate2XMPPresets](https://github.com/yuzukumo/LRtemplate2XMPPresets), `fixtures/minimal.lrtemplate` — a third party's sample of the legacy format | GPL-3.0, as Rustroom |

**Not committed:** Lightroom-written `.lrtemplate` files from
[xsynaptic/synaptic-lightroom-presets](https://github.com/xsynaptic/synaptic-lightroom-presets) — shared
"for everyone to use, adapt, and study" but with no licence file, so they are read only by the
`#[ignore]`d `real_lightroom_templates` test from a local path in `LR_PRESET_SAMPLES`.
