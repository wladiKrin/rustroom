# Rustroom's icon

`rustroom.svg` is the master — the aperture, picked by the author on 2026-11-17 (`v0.1` piece 3a), in the default
accent's two blues (`docs/DESIGN.md` §4.2). Everything else here is made from it and committed, so no build needs a
renderer:

| File | Used by |
|---|---|
| `png/rustroom-{16…1024}.png` | the window icon (`rustroom_app::app_icon`, the 256 px one); the Mac's `.icns` and Linux's desktop icon (pieces 3b, 3c) |
| `rustroom.ico` (16–256 px, PNG images) | the Windows exe's own icon (`crates/app/build.rs`) and the installer's (`packaging/windows/rustroom.iss`) |
| `msix/*.png` (StoreLogo, Square44x44Logo, Square150x150Logo at five scales; the 44's taskbar sizes) | the Store package (`packaging/windows/msix.ps1`); the 150 px tile holds the icon at two thirds of its side |

After changing the master, render again — `resvg` on `PATH` (`cargo install resvg`), Python 3 with nothing else:

```bash
python packaging/icons/render.py
```

The Mac's `.icns` is made on the Mac from the PNGs with `iconutil` (piece 3b), not here.
