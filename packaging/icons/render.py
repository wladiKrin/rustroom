"""Render Rustroom's icon from its master: every PNG size, the Windows .ico, and the Store package's logos.

    python packaging/icons/render.py        (needs `resvg` on PATH: `cargo install resvg`)

The rasters are committed, so a build never needs this script — run it after changing `rustroom.svg`.
macOS's .icns is made from the PNGs on the Mac (piece 3b, `iconutil`). Standard library only.
"""

import struct
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
MASTER = HERE / "rustroom.svg"
PNG = HERE / "png"
SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
# What Windows asks an .ico for: the small sizes for lists and the title bar, 256 for large icon views.
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]

# The MSIX package's logos (piece 4 goal 2), named with the resource qualifiers `makepri` indexes, so Windows picks
# the one nearest the display's scale. Each logo's size at 100 %, and the scales rendered.
MSIX = HERE / "msix"
MSIX_SCALES = [100, 125, 150, 200, 400]
MSIX_LOGOS = {"StoreLogo": 50, "Square44x44Logo": 44, "Square150x150Logo": 150}
# The taskbar's, Start's and Explorer's sizes, drawn at exactly these pixels; "unplated" is the same picture, since
# the icon carries its own plate.
MSIX_TARGET_SIZES = [16, 24, 32, 48, 256]
# The 150 px tile holds the icon at two thirds of its side, as Windows' own tiles do; the others fill their square.
TILE_SHARE = 2 / 3


def resvg(svg: Path, n: int, out: Path) -> None:
    subprocess.run(
        ["resvg", "-w", str(n), "-h", str(n), "--resources-dir", str(HERE), str(svg), str(out)], check=True
    )


def render() -> None:
    PNG.mkdir(exist_ok=True)
    for n in SIZES:
        resvg(MASTER, n, PNG / f"rustroom-{n}.png")


def msix() -> None:
    MSIX.mkdir(exist_ok=True)
    side, inset = 150, 150 * (1 - TILE_SHARE) / 2
    tile = (
        '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" '
        f'viewBox="0 0 {side} {side}" width="{side}" height="{side}">'
        f'<image x="{inset:g}" y="{inset:g}" width="{side - 2 * inset:g}" height="{side - 2 * inset:g}" '
        f'xlink:href="{MASTER.name}"/></svg>'
    )
    with tempfile.TemporaryDirectory() as tmp:
        padded = Path(tmp) / "tile.svg"
        padded.write_text(tile, encoding="utf-8")
        for name, base in MSIX_LOGOS.items():
            svg = padded if name == "Square150x150Logo" else MASTER
            for scale in MSIX_SCALES:
                resvg(svg, (base * scale + 50) // 100, MSIX / f"{name}.scale-{scale}.png")
    for n in MSIX_TARGET_SIZES:
        resvg(MASTER, n, MSIX / f"Square44x44Logo.targetsize-{n}.png")
        resvg(MASTER, n, MSIX / f"Square44x44Logo.altform-unplated_targetsize-{n}.png")


def ico() -> None:
    """An .ico whose images are PNGs (Windows Vista and later read them at every size)."""
    images = [(n, (PNG / f"rustroom-{n}.png").read_bytes()) for n in ICO_SIZES]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries, data = b"", b""
    for n, png in images:
        side = 0 if n == 256 else n  # 0 means 256 in an ICONDIRENTRY
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(png), offset + len(data))
        data += png
    (HERE / "rustroom.ico").write_bytes(header + entries + data)


if __name__ == "__main__":
    render()
    ico()
    msix()
    logos = len(MSIX_LOGOS) * len(MSIX_SCALES) + 2 * len(MSIX_TARGET_SIZES)
    print(f"wrote {len(SIZES)} PNGs, rustroom.ico and {logos} Store logos", file=sys.stderr)
